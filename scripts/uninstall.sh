#!/bin/bash
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run with sudo: sudo $0 [--with-driver]"
    exit 1
fi

WITH_DRIVER=0
if [ "${1:-}" = "--with-driver" ]; then
    WITH_DRIVER=1
fi

echo "==> Stopping and removing LaunchDaemons..."
for plist in \
    /Library/LaunchDaemons/com.controlesque.daemon.plist \
    /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist; do
    if [ -f "$plist" ]; then
        launchctl bootout system "$plist" 2>/dev/null || true
        rm -f "$plist"
    fi
done

killall controlesque 2>/dev/null || true
killall Karabiner-VirtualHIDDevice-Daemon 2>/dev/null || true

echo "==> Removing /usr/local/bin/controlesque and logs..."
rm -f /usr/local/bin/controlesque /var/log/controlesque.log

if [ "$WITH_DRIVER" -eq 1 ]; then
    echo "==> Deactivating and removing Karabiner-DriverKit-VirtualHIDDevice..."
    if [ -f "/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/scripts/uninstall/deactivate_driver.sh" ]; then
        bash "/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/scripts/uninstall/deactivate_driver.sh" || true
    fi
    rm -rf "/Library/Application Support/org.pqrs" "/Applications/.Karabiner-VirtualHIDDevice-Manager.app"
fi

echo "Done."
