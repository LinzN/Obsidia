<p align="center">
  <img src="assets/logo-512.png" width="128" alt="Obsidia logo">
</p>

<h1 align="center">Obsidia</h1>

<p align="center">
  A <a href="https://mirranet.de">MirraNET</a> project ·
  <a href="https://github.com/LinzN/obsidia">GitHub</a> ·
  <a href="https://github.com/LinzN/obsidia/releases">Releases</a>
</p>

A firmware flashing tool for **iPhone and iPad**, written in Rust.
Runs on **Windows**, **Linux** and **macOS** - with a modern graphical interface and a full CLI.

![Obsidia](docs/screenshot.png)

Obsidia talks to Apple devices through the open-source
[libimobiledevice](https://libimobiledevice.org) stack and fetches firmware metadata
from [ipsw.me](https://ipsw.me).

## Features

- Clear layout: Device, Flash and Toolbox tabs, firmware list in the middle,
  device and flash options on the right
- Detect iPhones / iPads in **Normal**, **Recovery** and **DFU** mode, with model, OS version,
  capacity, battery, serial, ECID and UDID
- Browse firmwares for the connected device or **any** iPhone / iPad model
- See at a glance which versions Apple **currently signs**
- **Download** IPSW files with progress, resume and SHA-1 verification
- **Flash** with one click: clean restore (erase) or update (keep data)
- Drag & drop a local `.ipsw` onto the window to flash it
- Downloaded firmware is reused while Apple signs it; optional auto-clean removes expired files
- Step-by-step guides for recovery and DFU mode for every iPhone and iPad generation
- Apple drivers (Windows): installs the classic USB drivers from Apple's iTunes installer
  (signature checked, without iTunes itself). They fix aborted uploads in recovery mode that
  are common with the Microsoft Store drivers on iPhone 14 and newer
- Battery & diagnostics: battery health (maximum capacity, charge cycles, temperature),
  IMEI, model number, activation state, storage; export as a text report, restart or shut
  down the device
- DFU assistant: a timer walks you through the button sequence and detects DFU mode live
- Backup, enter / exit recovery mode
- Live log with progress of every operation

> Apple only allows installing firmwares that are **currently signed**.
> Downgrades to unsigned versions are not possible (unsigned versions can still be downloaded).

## Installation

Installers for every platform are attached to each
[release](https://github.com/LinzN/obsidia/releases).

### Windows

1. Install Apple's USB drivers: the **Apple Devices** app (Microsoft Store) or iTunes.
2. Run `obsidia-<version>-x64.msi`.

The installer ships the libimobiledevice tools, so nothing else is needed. Obsidia shows
up in the start menu and can be removed via *Apps & features*.

> **Smart App Control:** the MSI and the bundled tools are not code-signed.
> If Windows' Smart App Control is turned on, it blocks them
> (`os error 4551`). Turn it off under *Windows Security > App & browser control*.

### Microsoft Store

Obsidia is available in the [Microsoft Store](https://apps.microsoft.com/detail/9nsg9p945vbz).
The Store version is signed by Microsoft, so Smart App Control and SmartScreen don't block it,
and it is updated through the Store (the built-in update check is turned off there). The
MSIX package is built by the *Build* workflow (`msix-store` artifact).

### Linux (Debian / Ubuntu)

```sh
sudo apt install ./obsidia_<version>-1_amd64.deb
```

This pulls in `libimobiledevice-utils`, `irecovery`, `idevicerestore` and `usbmuxd`.
Obsidia appears in the application menu.

### macOS

```sh
brew install libimobiledevice libirecovery idevicerestore
```

Open `obsidia-<version>-macos-arm64.dmg` and drag Obsidia into *Applications*. The app is
not notarized yet: on first launch right-click it and choose *Open*.

## Files

| Folder | Content |
|---|---|
| `C:\Obsidia\Firmware` (Windows), `Documents/Obsidia/Firmware` (Linux, macOS) | Downloaded IPSW files (reused while signed) |
| `Documents/Obsidia/Backups` | Device backups |
| `Documents/Obsidia/Logs` | Log of every restore |

On Windows the firmware folder is shared by all users of the computer, so a firmware file
is downloaded only once. The installer creates it with write access for all users; it is
kept when Obsidia is uninstalled. Firmware from the per-user folder of earlier versions is
moved there on start-up. The folder can be changed in the settings.

## Updates

Obsidia checks GitHub for a new release on start-up. If one is available, an info bar
offers to download and install it (the installer is verified against the release's
`SHA256SUMS`). You can also check manually under *Toolbox > Updates* or with
`obsidia update`.

The check can be turned off in the settings. In managed environments administrators can
disable it for everyone, e.g. via Group Policy / registry:

```
HKEY_LOCAL_MACHINE\Software\Policies\MirraNET\Obsidia
    DisableUpdates (DWORD) = 1
```

or with the environment variable `OBSIDIA_NO_UPDATE_CHECK=1`. Users without administrator
rights are told about new versions but are not offered the installer.
`OBSIDIA_UPDATE_URL` points the updater to a different release endpoint.

## Tool lookup

Obsidia looks for the libimobiledevice tools in this order:

1. the directory in `OBSIDIA_TOOLS`
2. a `tools` folder next to the executable
3. the directory of the executable
4. `PATH`
5. Windows: `C:\msys64\ucrt64\bin`, `C:\msys64\mingw64\bin`; macOS: `/opt/homebrew/bin`, `/usr/local/bin`

## Command line

Without arguments Obsidia opens the GUI. All functions are also available on the CLI:

```sh
obsidia list                          # show connected devices
obsidia firmwares                     # signed firmwares for the connected device
obsidia firmwares -p iPad13,18 --all  # any model, incl. unsigned
obsidia download -v 18.1              # download into the cache
obsidia backup                        # backup into Documents/Obsidia/Backups
obsidia flash                         # erase-restore to the latest signed firmware
obsidia flash -v 18.1 --update        # update to a specific version, keep data
obsidia flash -f ~/Downloads/x.ipsw   # local IPSW
obsidia recovery | exit-recovery
obsidia update                           # check for and install a new version
```

With several devices connected, select one with `-u <UDID|ECID>`.

Set `OBSIDIA_API` to use an ipsw.me-compatible mirror.

## Build from source

```sh
cargo build --release
```

On Linux the GUI needs the usual X11/Wayland development headers:
`libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev`.

Packaging lives in `packaging/` (WiX MSI, `cargo deb`, `.app`/`.dmg`). GitHub Actions
builds all installers on every push (`build.yml`). Releases: run *Actions > Release > Run workflow*
and enter the version (e.g. `0.5.0`; empty = next patch version). The workflow sets the
version, commits and tags it and starts the build, which publishes the GitHub release.
Locally the same works with `scripts/release.sh 0.5.0` and `git push origin main v0.5.0`.

## Troubleshooting

- **Device not found (Windows):** make sure the Apple Devices app / iTunes is installed and
  the device shows up there.
- **Restore fails:** put the device into **DFU mode** and try again.
- **Activation Lock** still applies after a restore - you need the Apple ID password.

## Privacy

See [docs/PRIVACY.md](docs/PRIVACY.md).

## Security

A security assessment for companies evaluating Obsidia is available in
[docs/SECURITY_ASSESSMENT.md](docs/SECURITY_ASSESSMENT.md).

## About

Obsidia is developed by [MirraNET](https://mirranet.de). Source code, issues and releases:
[github.com/LinzN/obsidia](https://github.com/LinzN/obsidia).

## License

MIT. The bundled libimobiledevice tools are licensed under the LGPL-2.1 / LGPL-3.0
by their respective authors. Icons: [Phosphor Icons](https://phosphoricons.com) (MIT). Font: [Inter](https://rsms.me/inter/) (SIL Open Font License, see `assets/fonts/Inter-LICENSE.txt`).
