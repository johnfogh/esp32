//! BLE announce protocol shared by firmware ([`crate::ble`]) and desktop tooling (`host` feature):
//! UUIDs, manager token, advertised name format and parsers. Pure `core`, available with every feature.
//!
//! Advertising packet (31 bytes, full): flags, complete 128-bit service UUID [`SERVICE_UUID`],
//! manufacturer data `[COMPANY_ID LE] ++ MAC(6)`.
//! Scan response: Complete Local Name `<project>-xxyyzz` (project name, `-`, last three MAC bytes in
//! lowercase hex). The project is truncated to [`PROJECT_MAX`] bytes so the name fits the 31-byte
//! scan response (2 bytes AD header + 29 bytes name). Parse with [`parse_name`].

/// Custom 128-bit primary service UUID.
pub const SERVICE_UUID: &str = "c0de5e00-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`SERVICE_UUID`] as a number.
pub const SERVICE_UUID_U128: u128 = 0xc0de5e00_6d67_4e2a_9b1f_3a5d7e9c0b01;
/// Write characteristic on [`SERVICE_UUID`] that receives [`MANAGER_TOKEN`].
pub const AUTH_CHAR_UUID: &str = "c0de5e01-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`AUTH_CHAR_UUID`] as a number.
pub const AUTH_CHAR_UUID_U128: u128 = 0xc0de5e01_6d67_4e2a_9b1f_3a5d7e9c0b01;
/// Notify characteristic on [`SERVICE_UUID`]: board log output, UTF-8 lines terminated by `\n`,
/// split into chunks of at most [`LOG_CHUNK`] bytes (reassemble on the central). Sent only to the
/// authenticated manager after it subscribes.
pub const LOG_CHAR_UUID: &str = "c0de5e02-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`LOG_CHAR_UUID`] as a number.
pub const LOG_CHAR_UUID_U128: u128 = 0xc0de5e02_6d67_4e2a_9b1f_3a5d7e9c0b01;
/// Log notification payload size (default ATT MTU 23 − 3).
pub const LOG_CHUNK: usize = 20;
/// Read characteristic on [`SERVICE_UUID`]: board status, [`STATUS_LEN`] bytes =
/// `uptime_ms` (u64 LE, read time) ++ connection RSSI as measured by the board (i8, [`RSSI_UNKNOWN`]
/// if unavailable). Used by the manager for the time offset and the link signal.
pub const STATUS_CHAR_UUID: &str = "c0de5e03-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`STATUS_CHAR_UUID`] as a number.
pub const STATUS_CHAR_UUID_U128: u128 = 0xc0de5e03_6d67_4e2a_9b1f_3a5d7e9c0b01;
/// Status payload length.
pub const STATUS_LEN: usize = 9;
/// RSSI byte meaning "not available".
pub const RSSI_UNKNOWN: i8 = 127;

/// Encode the status payload.
pub fn encode_status(uptime_ms: u64, rssi: Option<i8>) -> [u8; STATUS_LEN] {
    let mut b = [0u8; STATUS_LEN];
    b[..8].copy_from_slice(&uptime_ms.to_le_bytes());
    b[8] = rssi.unwrap_or(RSSI_UNKNOWN) as u8;
    b
}

/// Decode the status payload: `(uptime_ms, rssi)`.
pub fn parse_status(d: &[u8]) -> Option<(u64, Option<i8>)> {
    let up = u64::from_le_bytes(d.get(..8)?.try_into().ok()?);
    let r = *d.get(8)? as i8;
    Some((up, (r != RSSI_UNKNOWN).then_some(r)))
}

/// Write characteristic on [`SERVICE_UUID`]: JSON command lines from the authenticated manager,
/// `{"action":"…","params":[…]}` terminated by `\n`, in chunks of at most [`LOG_CHUNK`] bytes.
/// Results come back on [`LOG_CHAR_UUID`] as a line `<action>: <result>`.
pub const CMD_CHAR_UUID: &str = "c0de5e04-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`CMD_CHAR_UUID`] as a number.
pub const CMD_CHAR_UUID_U128: u128 = 0xc0de5e04_6d67_4e2a_9b1f_3a5d7e9c0b01;
/// Status buffer frames (notify): `[seq, flags, payload…]`, flags bit 7 = last frame, bit 6 = first
/// frame, bits 0–5 = frame index mod 64 (order check); the payload concatenates to
/// `{"seq":N,"buffer":{"<ms>":{…}}}` (see `status`, ≤ ~3 KB per snapshot).
/// Advertising interval bounds (ms): fast advertising so managers find/reconnect quickly.
pub const ADV_INTERVAL_MIN_MS: u64 = 20;
pub const ADV_INTERVAL_MAX_MS: u64 = 30;
pub const BUF_CHAR_UUID: &str = "c0de5e05-6d67-4e2a-9b1f-3a5d7e9c0b01";
/// [`BUF_CHAR_UUID`] as a number.
/// Buffer frame flag: last frame.
pub const BUF_FRAME_LAST: u8 = 0x80;
/// Buffer frame flag: first frame.
pub const BUF_FRAME_FIRST: u8 = 0x40;
/// Buffer frame index mask.
pub const BUF_FRAME_INDEX: u8 = 0x3f;
pub const BUF_CHAR_UUID_U128: u128 = 0xc0de5e05_6d67_4e2a_9b1f_3a5d7e9c0b01;

/// Exact bytes a manager must write to [`AUTH_CHAR_UUID`].
pub const MANAGER_TOKEN: &[u8] = b"manager-device";
/// Time a connected unbonded central has to write [`MANAGER_TOKEN`]. Bonded/encrypted links wait.
pub const AUTH_TIMEOUT_MS: u64 = 5_000;
/// Company identifier used for the manufacturer data (0xFFFF = "no company / testing").
pub const COMPANY_ID: u16 = 0xFFFF;
/// Maximum advertised name length (31-byte scan response minus the 2-byte AD header).
pub const NAME_MAX: usize = 29;
/// Maximum project-name bytes in the advertised name (`NAME_MAX` minus `-xxyyzz`).
pub const PROJECT_MAX: usize = NAME_MAX - 7;
/// Project used when an empty project name is passed.
pub const DEFAULT_PROJECT: &str = "esp32";

/// Advertised name buffer: `buf[..len]` is `<project>-xxyyzz` (ASCII/UTF-8).
#[derive(Clone, Copy)]
pub struct DeviceName {
    buf: [u8; NAME_MAX],
    len: usize,
}

impl DeviceName {
    pub fn as_bytes(&self) -> &[u8] { &self.buf[..self.len] }
    pub fn as_str(&self) -> &str { core::str::from_utf8(self.as_bytes()).unwrap_or(DEFAULT_PROJECT) }
}

/// Build the advertised name `<project>-xxyyzz` for `mac`. `project` is truncated to [`PROJECT_MAX`]
/// bytes (at a char boundary); an empty project becomes [`DEFAULT_PROJECT`].
pub fn device_name(project: &str, mac: [u8; 6]) -> DeviceName {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let project = if project.is_empty() { DEFAULT_PROJECT } else { project };
    let mut end = project.len().min(PROJECT_MAX);
    while !project.is_char_boundary(end) { end -= 1; }
    let p = &project.as_bytes()[..end];
    let mut buf = [0u8; NAME_MAX];
    buf[..p.len()].copy_from_slice(p);
    buf[p.len()] = b'-';
    for (i, b) in mac[3..].iter().enumerate() {
        buf[p.len() + 1 + 2 * i] = HEX[(b >> 4) as usize];
        buf[p.len() + 2 + 2 * i] = HEX[(b & 0xf) as usize];
    }
    DeviceName { buf, len: p.len() + 7 }
}

/// Split an advertised name into `(project, mac_suffix)`: `"laser-a1b2c3"` → `("laser", "a1b2c3")`.
/// `None` if it doesn't end in `-` + 6 lowercase/uppercase hex digits.
pub fn parse_name(name: &str) -> Option<(&str, &str)> {
    let (project, suffix) = name.rsplit_once('-')?;
    (!project.is_empty() && suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_hexdigit())).then_some((project, suffix))
}

/// Manufacturer-specific data including company id: `[COMPANY_ID LE] ++ mac`.
pub fn manufacturer_payload(mac: [u8; 6]) -> [u8; 8] {
    let c = COMPANY_ID.to_le_bytes();
    [c[0], c[1], mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]]
}

/// Extract the MAC from manufacturer data *after* the company id (as host BLE stacks such as
/// btleplug report it, keyed by company id). Returns `None` if shorter than 6 bytes.
pub fn parse_manufacturer_mac(data: &[u8]) -> Option<[u8; 6]> {
    data.get(..6)?.try_into().ok()
}
