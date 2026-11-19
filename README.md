<p align="center">
  <img src="assets/logo.svg" width="112" height="112" alt="controlesque logo" />
</p>

<h1 align="center">controlesque</h1>

<p align="center">
  <strong>macOS Caps Lock → Escape on tap, CTRL on hold.</strong><br/>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-macOS-000000?style=flat-square" alt="macOS" />
  <img src="https://img.shields.io/badge/rust-0%2btw-f74c00?style=flat-square" alt="Rust btw" />
  <img src="https://img.shields.io/badge/license-MIT-38bdf8?style=flat-square" alt="MIT License" />
</p>

---

if you're on Mac and use vim or require tap to esc and hold to ctrl, then this is for you.
I got tired of karabiner elements auto updating and showing popups, showing up in my tray, taking up ~100 MB of background services all the time, when all I needed was this seemingly very simple functionality, so I MacGyver'd this to use the same 0-latency method as karabiner and their headless driver. 


- **Input:** Exclusively seizes physical keyboards via IOKit (`IOServiceAddMatchingNotification` → `IOHIDDeviceCreate` → `kIOHIDOptionsTypeSeizeDevice`).
- **Output:** Talks directly to the standalone [Karabiner-DriverKit-VirtualHIDDevice](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice) daemon over its Unix socket (auto-detects Protocol 5 / v6.x and Protocol 7 / v8.x).

---

## Install & Auto-Start

Give these instructions to your agent. It does require `sudo` and manually adding the program in Privacy settings -> Input Monitoring -> + -> path to controlesque

### 1. Install the standalone DriverKit driver

Download and install the latest `.pkg` from [Karabiner-DriverKit-VirtualHIDDevice Releases](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases), then activate it:

```bash
sudo /Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager activate
```

Allow `pqrs.org` in **System Settings → Privacy & Security**.

### 2. Build, install & enable boot auto-start

```bash
sudo ./scripts/install.sh
```


> **Migrating from Karabiner-Elements?** Run `sudo ./scripts/migrate-from-karabiner.sh` first to remove Karabiner-Elements while keeping the DriverKit driver, then run `sudo ./scripts/install.sh`.

### 3. Grant Input Monitoring

`install.sh` automatically opens **Input Monitoring** and reveals `controlesque.app` in Finder, or run:

```bash
open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
open -R "/Library/Application Support/controlesque/controlesque.app"
```

1. *(If upgrading/rebuilding)* Remove any old `controlesque` entry with **`-`**
2. **Drag and drop `controlesque.app`** from Finder into the Input Monitoring list *(drag the `.app` bundle itself, not the inner binary)*
3. Ensure its toggle is **ON**, then restart the daemon:
   ```bash
   sudo launchctl kickstart -k system/com.controlesque.daemon
   ```

---

## CLI

```bash
sudo controlesque [--timeout-ms 500] [--protocol 5|7] [--verbose]
```

Logs (when running via `launchd`): `/var/log/controlesque.log`

---

## Uninstall

```bash
sudo ./scripts/uninstall.sh                # remove controlesque & LaunchDaemons
sudo ./scripts/uninstall.sh --with-driver  # also remove DriverKit VirtualHIDDevice
```

## License

[MIT](LICENSE)
