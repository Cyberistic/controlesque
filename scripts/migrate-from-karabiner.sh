#!/bin/bash
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run with sudo: sudo $0"
    exit 1
fi

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> 1. Stopping & uninstalling Karabiner-Elements (keeping DriverKit VirtualHIDDevice)..."
if [ -f "/Library/Application Support/org.pqrs/Karabiner-Elements/uninstall_core.sh" ]; then
    /bin/bash "/Library/Application Support/org.pqrs/Karabiner-Elements/uninstall_core.sh"
fi

for proc in \
    Karabiner-Core-Service \
    karabiner_grabber \
    karabiner_console_user_server \
    karabiner_session_monitor \
    Karabiner-NotificationWindow \
    Karabiner-Updater \
    Karabiner-Menu \
    Karabiner-Elements \
    Karabiner-EventViewer; do
    killall "$proc" 2>/dev/null || true
done

echo "==> 2. Installing standalone LaunchDaemon for Karabiner-VirtualHIDDevice-Daemon..."
cp "$REPO_DIR/launchd/org.pqrs.karabiner-virtualhiddevice-daemon.plist" \
   /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist
chown root:wheel /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist
chmod 644 /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist

# Restart vhidd under our standalone LaunchDaemon so it survives without Karabiner-Elements.
killall Karabiner-VirtualHIDDevice-Daemon 2>/dev/null || true
launchctl bootout system /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist 2>/dev/null || true
launchctl bootstrap system /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist

echo "==> 3. Installing /usr/local/bin/controlesque..."
mkdir -p /usr/local/bin
cp "$REPO_DIR/target/release/controlesque" /usr/local/bin/controlesque
chmod 755 /usr/local/bin/controlesque

echo ""
echo "Done! Remaining Karabiner processes (should only be VirtualHIDDevice-Daemon + DriverKit dext):"
pgrep -fl -i karabiner || true
echo ""
echo "==> Starting controlesque in test mode (Ctrl+C to stop)..."
echo "    (If seizing fails, grant Input Monitoring to your Terminal and /usr/local/bin/controlesque"
echo "     in System Settings -> Privacy & Security -> Input Monitoring.)"
echo ""
exec /usr/local/bin/controlesque --verbose
