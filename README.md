# controlesque

A minimal, zero-dependency Rust daemon for macOS that does one thing and one thing only:

- **Tap Caps Lock** → **Escape**
- **Hold Caps Lock** (with another key, or for `Ctrl+Click`) → **Left Control**

No menu bar tray icon, no auto-updater, no GUI, no JSON rule engine, no Electron/SwiftUI overhead — a single ~350 KB native binary using ~1–2 MB of RAM.

## How It Works (Same OS/Kernel-Level Method as Karabiner-Elements)

`controlesque` does **not** use `CGEventTap` or WindowServer-level event interception (which break in Secure Input / password prompts). It uses the exact same two-layer OS/HID architecture as [Karabiner-Elements](https://github.com/pqrs-org/Karabiner-Elements):

1. **Input (IOKit HID Seize):** Uses macOS `IOKit` (`IOHIDManager` + `IOHIDDeviceOpen(..., kIOHIDOptionsTypeSeizeDevice)`) to claim physical keyboards exclusively at the HID layer while skipping the virtual keyboard.
2. **Output (DriverKit Virtual HID Keyboard):** Speaks directly to the standalone [Karabiner-DriverKit-VirtualHIDDevice](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice) daemon over its local Unix domain socket (auto-detects both Protocol 5 / v6.x datagram and Protocol 7 / v8.x stream). Because Apple requires a signed DriverKit entitlement to publish a virtual HID keyboard on SIP-enabled macOS, the standalone DriverKit extension + daemon is the smallest possible OS-level output primitive.

All other keys — including standard modifiers, media/consumer keys, Apple `Fn`/Globe, brightness, Mission Control, and Spotlight — are forwarded transparently.

### Behavior Details

- Pressing Caps Lock immediately asserts `Left Control`, so fast chords (`Caps+C`, `Caps+[`, `Caps+R`) and `Caps+Click` with a mouse/trackpad work with **zero latency**.
- Releasing Caps Lock within `--timeout-ms` (default `500ms`) without pressing any other key releases `Left Control` and emits `Escape`.
- Holding Caps Lock longer than `--timeout-ms` or pressing any other key while it is held suppresses `Escape`.
- Physically holding real `Left Control` at the same time is tracked independently so releasing Caps Lock never drops a held physical Control key.

---

## Installation

### 1. Install the standalone DriverKit virtual HID driver

> If you are migrating away from Karabiner-Elements, uninstall Karabiner-Elements first (via **Karabiner-Elements Settings → Uninstall**), then install the standalone driver package below so you no longer have the tray app, updater, or background services.

1. Download and install the latest `.pkg` from [pqrs-org/Karabiner-DriverKit-VirtualHIDDevice Releases](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases).
2. Activate the DriverKit system extension:
   ```bash
   sudo /Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager activate
   ```
3. Open **System Settings → Privacy & Security** and click **Allow** for the system extension from `pqrs.org`.
4. Verify it is active:
   ```bash
   systemextensionsctl list
   ```
   You should see `org.pqrs.Karabiner-DriverKit-VirtualHIDDevice` as `[activated enabled]`.

### 2. Build and install `controlesque`

Requires Rust (`cargo`). No external crates are fetched or compiled.

```bash
cargo build --release
sudo cp target/release/controlesque /usr/local/bin/controlesque
```

### 3. Grant Input Monitoring permission

Open **System Settings → Privacy & Security → Input Monitoring** and add `/usr/local/bin/controlesque` (and your terminal app if testing interactively).

---

## Usage

### Run interactively (for testing)

Make sure `Karabiner-VirtualHIDDevice-Daemon` is running, then run:

```bash
sudo controlesque --verbose
```

Press `Ctrl+C` to stop cleanly (all keys are released and physical keyboards are unseized automatically).

### CLI Options

```text
USAGE:
    sudo controlesque [OPTIONS]

OPTIONS:
    --timeout-ms <MS>    Max tap duration in ms to emit Escape (default: 500)
    --protocol <5|7>     Force Karabiner VirtualHIDDevice protocol version (default: auto)
    -v, --verbose        Print virtual driver & device diagnostics
    -h, --help           Show this help
    -V, --version        Show version
```

### Run automatically at boot (`launchd`)

Install the two included LaunchDaemons (`Karabiner-VirtualHIDDevice-Daemon` + `controlesque`):

```bash
sudo cp launchd/org.pqrs.karabiner-virtualhiddevice-daemon.plist /Library/LaunchDaemons/
sudo cp launchd/com.controlesque.daemon.plist /Library/LaunchDaemons/

sudo chown root:wheel /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist \
                      /Library/LaunchDaemons/com.controlesque.daemon.plist
sudo chmod 644 /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist \
               /Library/LaunchDaemons/com.controlesque.daemon.plist

sudo launchctl bootstrap system /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.controlesque.daemon.plist
```

Check logs:

```bash
tail -f /var/log/controlesque.log
```

---

## Uninstall

```bash
sudo launchctl bootout system /Library/LaunchDaemons/com.controlesque.daemon.plist
sudo launchctl bootout system /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist
sudo rm -f /Library/LaunchDaemons/com.controlesque.daemon.plist \
           /Library/LaunchDaemons/org.pqrs.karabiner-virtualhiddevice-daemon.plist \
           /usr/local/bin/controlesque \
           /var/log/controlesque.log
```

To also remove the DriverKit extension:

```bash
sudo bash '/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/scripts/uninstall/deactivate_driver.sh'
sudo rm -rf "/Library/Application Support/org.pqrs" "/Applications/.Karabiner-VirtualHIDDevice-Manager.app"
```

## License

MIT — see [LICENSE](LICENSE).
