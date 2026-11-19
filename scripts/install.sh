#!/bin/bash
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run with sudo: sudo $0"
    exit 1
fi

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VHIDD_BIN="/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon"
APP_DIR="/Library/Application Support/controlesque/controlesque.app"

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

echo "==> 2. Installing headless app bundle ($APP_DIR)..."
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$REPO_DIR/target/release/controlesque" "$APP_DIR/Contents/MacOS/controlesque"
cp "$REPO_DIR/assets/Info.plist" "$APP_DIR/Contents/Info.plist"
cp "$REPO_DIR/assets/AppIcon.icns" "$APP_DIR/Contents/Resources/AppIcon.icns"
chmod 755 "$APP_DIR/Contents/MacOS/controlesque"
xattr -cr "$APP_DIR"
codesign --force --sign - --identifier com.controlesque.daemon "$APP_DIR"
chown -R root:wheel "/Library/Application Support/controlesque"

# Symlink into /usr/local/bin so `sudo controlesque` works from the CLI too.
mkdir -p /usr/local/bin
ln -sf "$APP_DIR/Contents/MacOS/controlesque" /usr/local/bin/controlesque

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

# Truncate log so we check fresh startup status below.
: > /var/log/controlesque.log

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

# Check if Input Monitoring permission is needed (first install, or after rebuilding the binary).
if grep -q "waiting for Input Monitoring permission" /var/log/controlesque.log 2>/dev/null; then
    echo ""
    echo "==> macOS Input Monitoring permission required:"
    echo "    Opening Input Monitoring and revealing controlesque.app in Finder..."
    if [ -n "${SUDO_USER:-}" ]; then
        su - "$SUDO_USER" -c 'open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"' 2>/dev/null || true
        su - "$SUDO_USER" -c "open -R '$APP_DIR'" 2>/dev/null || true
    else
        open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent" 2>/dev/null || true
        open -R "$APP_DIR" 2>/dev/null || true
    fi
    echo "    1. If an older 'controlesque' entry exists, select it and click '-' to remove it"
    echo "    2. Drag and drop 'controlesque.app' from Finder into the Input Monitoring list"
    echo "       (do NOT select Contents/MacOS/controlesque — drag the .app bundle itself)"
    echo "    3. Ensure its toggle is ON, then restart the daemon:"
    echo "       sudo launchctl kickstart -k system/com.controlesque.daemon"
fi
