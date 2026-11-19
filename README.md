# controlesque

A minimal, zero-dependency Rust daemon for macOS that does one thing and one thing only:

- **Tap Caps Lock** → **Escape**
- **Hold Caps Lock** (with another key, or for `Ctrl+Click`) → **Left Control**

No menu bar tray icon, no auto-updater, no GUI, no JSON rule engine, no Electron/SwiftUI overhead — a single ~350 KB native binary using ~1–2 MB of RAM.

## How It Works (Same OS/Kernel-Level Method as Karabiner-Elements)

`controlesque` does **not** use `CGEventTap` or WindowServer-level event interception (which break in Secure Input / password prompts). It uses the exact same two-layer OS/HID architecture as [Karabiner-Elements](https://github.com/pqrs-org/Karabiner-Elements):

1. **Input (IOKit Exclusive HID Seize):** Discovers `IOHIDDevice` keyboards via `IOServiceAddMatchingNotification`, creates standalone `IOHIDDeviceRef` handles via `IOHIDDeviceCreate`, and opens each physical keyboard exclusively with `IOHIDDeviceOpen(..., kIOHIDOptionsTypeSeizeDevice)` (skipping the virtual keyboard).
2. **Output (DriverKit Virtual HID Keyboard):** Speaks directly to the standalone [Karabiner-DriverKit-VirtualHIDDevice](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice) daemon over its local Unix domain socket (auto-detects both Protocol 5 / v6.x datagram and Protocol 7 / v8.x stream). Because Apple requires a signed DriverKit entitlement to publish a virtual HID keyboard on SIP-enabled macOS, the standalone DriverKit extension + daemon is the smallest possible OS-level output primitive.

All other keys — including standard modifiers, media/consumer keys, Apple `Fn`/Globe, brightness, Mission Control, and Spotlight — are forwarded transparently.

### Behavior Details

- Pressing Caps Lock immediately asserts `Left Control`, so fast chords (`Caps+C`, `Caps+[`, `Caps+R`) and `Caps+Click` with a mouse/trackpad work with **zero latency**.
- Releasing Caps Lock within `--timeout-ms` (default `500ms`) without pressing any other key releases `Left Control` and emits `Escape`.
- Holding Caps Lock longer than `--timeout-ms` or pressing any other key while it is held suppresses `Escape`.
- Physically holding real `Left Control` at the same time is tracked independently so releasing Caps Lock never drops a held physical Control key.

---

## Quick Install & Auto-Start

### Step 1: Ensure the standalone DriverKit virtual HID driver is installed

*(Skip this step if you already have `Karabiner-DriverKit-VirtualHIDDevice` installed and activated.)*

1. Download and install the latest `.pkg` from [pqrs-org/Karabiner-DriverKit-VirtualHIDDevice Releases](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases).
2. Activate the DriverKit system extension:
   ```bash
   sudo /Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager activate
   ```
3. Open **System Settings → Privacy & Security** and click **Allow** for the system extension from `pqrs.org`.

### Step 2: Build, install, and enable auto-start on boot

```bash
sudo ./scripts/install.sh
```

This single script:
- Compiles the zero-dependency release binary (`cargo build --release`)
- Installs `/usr/local/bin/controlesque`
- Disables any conflicting legacy Karabiner-Elements daemons
- Installs and starts LaunchDaemons for both `Karabiner-VirtualHIDDevice-Daemon` and `controlesque` so they start immediately and automatically on every boot

### Step 3: Grant Input Monitoring permission (first time only)

Open **System Settings → Privacy & Security → Input Monitoring**, add `/usr/local/bin/controlesque`, and restart the daemon:

```bash
sudo launchctl kickstart -k system/com.controlesque.daemon
```

Check logs anytime with:

```bash
tail -f /var/log/controlesque.log
```

---

## Migrating from Karabiner-Elements

If you currently have Karabiner-Elements installed and want to uninstall it while keeping the DriverKit virtual HID driver and testing `controlesque` interactively:

```bash
sudo ./scripts/migrate-from-karabiner.sh
```

Once verified, press `Ctrl+C` and run `sudo ./scripts/install.sh` to enable auto-start at boot.

---

## Usage (Interactive / CLI)

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

---

## Uninstall

Remove `controlesque` and its LaunchDaemons:

```bash
sudo ./scripts/uninstall.sh
```

To also deactivate and remove `Karabiner-DriverKit-VirtualHIDDevice`:

```bash
sudo ./scripts/uninstall.sh --with-driver
```

## License

MIT — see [LICENSE](LICENSE).
