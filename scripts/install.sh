#!/bin/bash
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run with sudo: sudo $0"
    exit 1
fi

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VHIDD_BIN="/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon"

if [ ! -x "$VHIDD_BIN" ]; then
    echo "error: Karabiner-DriverKit-VirtualHIDDevice is not installed."
    echo ""
    echo "Install the standalone driver first:"
    echo "  1. Download and install the latest .pkg from:"
    echo "     https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases"
    echo "  2. Activate the DriverKit system extension:"
    echo "     sudo /Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager activate"
    echo "  3. Allow it in System Settings -> Privacy & Security, then re-run this script."
    exit 1
fi

echo "==> 1. Building release binary..."
if [ -n "${SUDO_USER:-}" ]; then
    su - "$SUDO_USER" -c "cd '$REPO_DIR' && cargo build --release"
else
    (cd "$REPO_DIR" && cargo build --release)
fi

echo "==> 2. Installing /usr/local/bin/controlesque..."
mkdir -p /usr/local/bin
cp "$REPO_DIR/target/release/controlesque" /usr/local/bin/controlesque
chmod 755 /usr/local/bin/controlesque

echo "==> 3. Disabling conflicting Karabiner-Elements daemons (if any)..."
for svc in \
    org.pqrs.service.daemon.Karabiner-Core-Service \
    org.pqrs.service.daemon.karabiner_grabber \
    org.pqrs.service.daemon.Karabiner-VirtualHIDDevice-Daemon; do
    launchctl bootout "system/$svc" 2>/dev/null || true
    launchctl disable "system/$svc" 2>/dev/null || true
done

echo "==> 4. Installing & starting LaunchDaemons (auto-start on boot)..."
install_plist() {
    local src="$1"
    local name
    name="$(basename "$src")"
    local dst="/Library/LaunchDaemons/$name"

    launchctl bootout system "$dst" 2>/dev/null || true
    cp "$src" "$dst"
    chown root:wheel "$dst"
    chmod 644 "$dst"
    launchctl bootstrap system "$dst"
}

killall Karabiner-VirtualHIDDevice-Daemon 2>/dev/null || true
killall controlesque 2>/dev/null || true

install_plist "$REPO_DIR/launchd/org.pqrs.karabiner-virtualhiddevice-daemon.plist"
sleep 1
install_plist "$REPO_DIR/launchd/com.controlesque.daemon.plist"
sleep 1

echo ""
echo "controlesque is installed and running (auto-starts at boot)."
echo "Logs: /var/log/controlesque.log"
echo ""
echo "Active processes:"
pgrep -fl "Karabiner-VirtualHIDDevice-Daemon|Karabiner-DriverKit-VirtualHIDDevice|controlesque" || true

# Check if Input Monitoring permission is still needed and guide the user directly.
if tail -n 5 /var/log/controlesque.log 2>/dev/null | grep -q "waiting for Input Monitoring permission"; then
    echo ""
    echo "==> One-time macOS permission needed (Input Monitoring):"
    echo "    Opening System Settings -> Privacy & Security -> Input Monitoring..."
    if [ -n "${SUDO_USER:-}" ]; then
        su - "$SUDO_USER" -c 'open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"' 2>/dev/null || true
    else
        open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent" 2>/dev/null || true
    fi
    echo "    1. Click '+' at the bottom of the list"
    echo "    2. Press Cmd+Shift+G and enter: /usr/local/bin/controlesque"
    echo "    3. Add it and make sure its toggle is ON"
    echo "    (controlesque will automatically seize your keyboard within 2 seconds — no restart needed!)"
fi
