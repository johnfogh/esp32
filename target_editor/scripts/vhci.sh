#!/usr/bin/env bash
# Start six virtual HCI adapters (one for esp32-controller, four sensors, one hub).
# Dual-mode (default btvirt type), not -L: LE-only hangs BlueZ RegisterAdvertisement.
set -euo pipefail

COUNT="${1:-6}"
BTVIRT="${BTVIRT:-}"
if [[ -z "$BTVIRT" ]]; then
  if command -v btvirt >/dev/null 2>&1; then
    BTVIRT="$(command -v btvirt)"
  elif [[ -x "$HOME/.local/src/bluez/emulator/btvirt" ]]; then
    BTVIRT="$HOME/.local/src/bluez/emulator/btvirt"
  fi
fi
if [[ -z "$BTVIRT" ]]; then
  cat >&2 <<'EOF'
btvirt not found. Arch extra/bluez-utils does not ship it. Build the BlueZ
emulator locally (do not `make install` over the system stack):

  git clone https://git.kernel.org/pub/scm/bluetooth/bluez.git ~/.local/src/bluez
  cd ~/.local/src/bluez
  ./bootstrap
  ./configure --enable-testing --disable-manpages
  make emulator/btvirt

Then re-run this script. Override the binary with BTVIRT=/path/to/btvirt.
EOF
  exit 1
fi

if [[ ! -e /dev/vhci ]]; then
  sudo modprobe hci_vhci
fi
sudo chmod a+rw /dev/vhci 2>/dev/null || true

if pgrep -x btvirt >/dev/null 2>&1; then
  echo "stopping existing btvirt"
  sudo pkill -x btvirt || true
  sleep 1
fi
echo "starting: sudo $BTVIRT -l$COUNT"
sudo "$BTVIRT" -l"$COUNT" >/tmp/btvirt.log 2>&1 &
sleep 1

echo "virtual adapters:"
for d in /sys/class/bluetooth/hci*; do
  [[ -e "$d" ]] || continue
  name="$(basename "$d")"
  canon="$(readlink -f "$d" 2>/dev/null || echo "$d")"
  if [[ "$name" == hci0 ]]; then
    echo "  $name  (skipped: real radio)"
    continue
  fi
  if [[ "$canon" == *"/virtual/"* ]]; then
    idx="${name#hci}"
    sudo btmgmt --index "$idx" power on >/dev/null 2>&1 || true
    sudo btmgmt --index "$idx" le on >/dev/null 2>&1 || true
    sudo btmgmt --index "$idx" connectable on >/dev/null 2>&1 || true
    sudo btmgmt --index "$idx" discov on >/dev/null 2>&1 || true
    sudo btmgmt --index "$idx" bondable off >/dev/null 2>&1 || true
    addr="$(cat "$d/address" 2>/dev/null || echo "?")"
    echo "  $name  $addr"
  fi
done

echo
echo "Reserve the first virtual adapter for the controller, e.g.:"
echo "  VIRTUAL_TARGET_CENTRAL=hci1 esp32-controller --adapter hci1"
echo "target-editor --virtual uses the next adapters for S1–S4 then the hub."
