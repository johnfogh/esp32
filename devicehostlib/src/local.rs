//! Same-host manager link: Unix socket, BLE protocol over framed JSON.
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::unbounded_channel;
use utils::ble_proto::{self, AUTH_TIMEOUT_MS, MANAGER_TOKEN};

use crate::protocol::{self, Buffer};
use crate::roster;
use crate::sensor::{Board, Hub, Link};

#[derive(Serialize, Deserialize)]
#[serde(tag = "t")]
enum Msg {
    #[serde(rename = "open")]
    Open { addr: String },
    #[serde(rename = "auth")]
    Auth { #[serde(default)] data: String },
    #[serde(rename = "cmd")]
    Cmd { #[serde(default)] data: String },
    #[serde(rename = "status")]
    Status { #[serde(default)] data: Option<Vec<u8>> },
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "err")]
    Err { msg: String },
    #[serde(rename = "log")]
    Log { data: String },
    #[serde(rename = "buf")]
    Buf { data: Vec<u8> },
}

pub async fn listen(hub: Arc<Hub>) {
    let path = roster::sock_path();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            for b in hub.all_boards() {
                b.set_error(format!("local socket: {e}"));
            }
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let hub = hub.clone();
                tokio::spawn(async move {
                    if let Err(e) = session(hub, stream).await {
                        let _ = e;
                    }
                });
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

async fn session(hub: Arc<Hub>, mut stream: UnixStream) -> Result<(), String> {
    let open = read_msg(&mut stream).await.map_err(|e| e.to_string())?;
    let Msg::Open { addr } = open else {
        let _ = write_msg(&mut stream, &Msg::Err { msg: "need open".into() }).await;
        return Ok(());
    };
    let Some(board) = find(&hub, &addr) else {
        let _ = write_msg(&mut stream, &Msg::Err { msg: format!("unknown {addr}") }).await;
        return Ok(());
    };
    if !board.try_local() {
        let _ = write_msg(&mut stream, &Msg::Err { msg: "busy".into() }).await;
        return Ok(());
    }
    board.set_link(Link::Connected);
    let _ = write_msg(&mut stream, &Msg::Ok).await;
    let result = run_authed(board.clone(), stream).await;
    board.end_local();
    result
}

async fn run_authed(board: Arc<Board>, mut stream: UnixStream) -> Result<(), String> {
    let deadline = tokio::time::sleep(Duration::from_millis(AUTH_TIMEOUT_MS));
    tokio::pin!(deadline);
    let authed = loop {
        tokio::select! {
            _ = &mut deadline => {
                let _ = write_msg(&mut stream, &Msg::Err { msg: "auth timeout".into() }).await;
                return Ok(());
            }
            msg = read_msg(&mut stream) => {
                let msg = msg.map_err(|e| e.to_string())?;
                match msg {
                    Msg::Auth { data } => {
                        if data.as_bytes() == MANAGER_TOKEN {
                            board.set_link(Link::Authenticated);
                            board.set_error("");
                            let _ = write_msg(&mut stream, &Msg::Ok).await;
                            break true;
                        }
                        let _ = write_msg(&mut stream, &Msg::Err { msg: "bad token".into() }).await;
                        return Ok(());
                    }
                    _ => {
                        let _ = write_msg(&mut stream, &Msg::Err { msg: "need auth".into() }).await;
                        return Ok(());
                    }
                }
            }
        }
    };
    if !authed {
        return Ok(());
    }
    let _ = write_msg(
        &mut stream,
        &Msg::Log { data: format!("{} up", board.name()) },
    )
    .await;

    let mut buffer = Buffer::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let (done_tx, mut done_rx) = unbounded_channel::<(String, String)>();
    loop {
        tokio::select! {
            _ = tick.tick() => {
                buffer.tick(&board, true);
                while let Some(frame) = buffer.next_frame() {
                    if write_msg(&mut stream, &Msg::Buf { data: frame }).await.is_err() {
                        return Ok(());
                    }
                    buffer.advance();
                }
            }
            done = done_rx.recv() => {
                let Some((action, line)) = done else { return Ok(()) };
                if !protocol::is_silent(&action) {
                    let err = line.contains(": error:");
                    let result = line.split_once(": ").map(|(_, x)| x).unwrap_or(&line);
                    buffer.add_method(&board, &action, result, err);
                }
                if protocol::logs_result(&action)
                    && write_msg(&mut stream, &Msg::Log { data: line }).await.is_err()
                {
                    return Ok(());
                }
            }
            msg = read_msg(&mut stream) => {
                let msg = match msg {
                    Ok(m) => m,
                    Err(_) => return Ok(()),
                };
                match msg {
                    Msg::Cmd { data } => {
                        let line = data.trim_end_matches(['\n', '\r']);
                        if !line.is_empty()
                            && handle_cmd(&board, &mut buffer, &done_tx, line, &mut stream).await.is_err()
                        {
                            return Ok(());
                        }
                    }
                    Msg::Status { .. } => {
                        let tel = board.telemetry();
                        let bytes = ble_proto::encode_status(tel.uptime_ms, None).to_vec();
                        if write_msg(&mut stream, &Msg::Status { data: Some(bytes) }).await.is_err() {
                            return Ok(());
                        }
                    }
                    Msg::Auth { .. } | Msg::Open { .. } | Msg::Ok | Msg::Err { .. } | Msg::Log { .. } | Msg::Buf { .. } => {}
                }
            }
        }
    }
}

async fn handle_cmd(
    board: &Arc<Board>,
    buffer: &mut Buffer,
    done: &tokio::sync::mpsc::UnboundedSender<(String, String)>,
    line: &str,
    stream: &mut UnixStream,
) -> std::io::Result<()> {
    let action = protocol::action_name(line).unwrap_or_default();
    if action == "buffer_ack" {
        let r = protocol::execute(board, line);
        if let Some(rest) = r.strip_prefix("buffer_ack: ")
            && !rest.starts_with("error:")
            && let Ok(seqs) = protocol::parse_ack_seqs(rest)
        {
            buffer.ack(&seqs);
        }
        return Ok(());
    }
    if action == "get_accelerometers" {
        let board = board.clone();
        let done = done.clone();
        let line = line.to_string();
        tokio::task::spawn_blocking(move || {
            let r = protocol::execute(&board, &line);
            let _ = done.send(("get_accelerometers".into(), r));
        });
        return Ok(());
    }
    let r = protocol::execute(board, line);
    if !protocol::is_silent(&action) {
        let err = r.contains(": error:");
        let result = r.split_once(": ").map(|(_, x)| x).unwrap_or(&r);
        buffer.add_method(board, &action, result, err);
    }
    if protocol::logs_result(&action) {
        write_msg(stream, &Msg::Log { data: r }).await?;
    }
    Ok(())
}

pub enum ClientEv {
    Log(String),
    Buf(Vec<u8>),
    Status(Vec<u8>),
}

/// Same-process manager link (no Unix socket).
pub fn attach(
    board: Arc<Board>,
) -> Result<
    (
        tokio::sync::mpsc::UnboundedSender<String>,
        tokio::sync::mpsc::UnboundedReceiver<ClientEv>,
    ),
    String,
> {
    if !board.try_local() {
        return Err("busy".into());
    }
    board.set_link(Link::Connected);
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = unbounded_channel();
    tokio::spawn(async move {
        run_inproc(board.clone(), cmd_rx, ev_tx).await;
        board.end_local();
    });
    Ok((cmd_tx, ev_rx))
}

async fn run_inproc(
    board: Arc<Board>,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<String>,
    ev: tokio::sync::mpsc::UnboundedSender<ClientEv>,
) {
    board.set_link(Link::Authenticated);
    board.set_error("");
    let _ = ev.send(ClientEv::Log(format!("{} up", board.name())));
    let mut buffer = Buffer::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let (done_tx, mut done_rx) = unbounded_channel::<(String, String)>();
    loop {
        tokio::select! {
            _ = tick.tick() => {
                buffer.tick(&board, true);
                while let Some(frame) = buffer.next_frame() {
                    if ev.send(ClientEv::Buf(frame)).is_err() {
                        return;
                    }
                    buffer.advance();
                }
            }
            done = done_rx.recv() => {
                let Some((action, line)) = done else { return };
                if !protocol::is_silent(&action) {
                    let err = line.contains(": error:");
                    let result = line.split_once(": ").map(|(_, x)| x).unwrap_or(&line);
                    buffer.add_method(&board, &action, result, err);
                }
                if protocol::logs_result(&action) && ev.send(ClientEv::Log(line)).is_err() {
                    return;
                }
            }
            cmd = cmds.recv() => {
                let Some(line) = cmd else { return };
                let line = line.trim_end_matches(['\n', '\r']);
                if line == "__status__" {
                    let tel = board.telemetry();
                    let bytes = ble_proto::encode_status(tel.uptime_ms, None).to_vec();
                    if ev.send(ClientEv::Status(bytes)).is_err() {
                        return;
                    }
                    continue;
                }
                if line.is_empty() {
                    continue;
                }
                let action = protocol::action_name(line).unwrap_or_default();
                if action == "buffer_ack" {
                    let r = protocol::execute(&board, line);
                    if let Some(rest) = r.strip_prefix("buffer_ack: ")
                        && !rest.starts_with("error:")
                        && let Ok(seqs) = protocol::parse_ack_seqs(rest)
                    {
                        buffer.ack(&seqs);
                    }
                    continue;
                }
                if action == "get_accelerometers" {
                    let board = board.clone();
                    let done = done_tx.clone();
                    let line = line.to_string();
                    tokio::task::spawn_blocking(move || {
                        let r = protocol::execute(&board, &line);
                        let _ = done.send(("get_accelerometers".into(), r));
                    });
                    continue;
                }
                let r = protocol::execute(&board, line);
                if !protocol::is_silent(&action) {
                    let err = r.contains(": error:");
                    let result = r.split_once(": ").map(|(_, x)| x).unwrap_or(&r);
                    buffer.add_method(&board, &action, result, err);
                }
                if protocol::logs_result(&action) && ev.send(ClientEv::Log(r)).is_err() {
                    return;
                }
            }
        }
    }
}

pub fn find_board(hub: &Hub, addr: &str) -> Option<Arc<Board>> {
    find(hub, addr)
}

fn find(hub: &Hub, addr: &str) -> Option<Arc<Board>> {
    let hit = |b: &Arc<Board>| {
        let s = b.snapshot();
        s.adapter_addr.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(addr))
            || s.name.eq_ignore_ascii_case(addr)
            || s.mac.eq_ignore_ascii_case(addr)
    };
    hub.manager_boards()
        .into_iter()
        .find(hit)
        .or_else(|| hub.all_boards().into_iter().find(hit))
}

async fn write_msg(stream: &mut UnixStream, msg: &Msg) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let n: u16 = u16::try_from(bytes.len()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "msg too long"))?;
    stream.write_all(&n.to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await
}

async fn read_msg(stream: &mut UnixStream) -> std::io::Result<Msg> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;
    let n = u16::from_le_bytes(hdr) as usize;
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
