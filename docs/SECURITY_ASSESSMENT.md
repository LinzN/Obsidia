# Obsidia – Security Assessment

| | |
|---|---|
| **Document type** | Vendor self-assessment for software onboarding |
| **Product** | Obsidia – iPhone / iPad firmware tool |
| **Vendor** | MirraNET ([mirranet.de](https://mirranet.de)) |
| **Version covered** | 0.4.x |
| **Platforms** | Windows 10/11 (x64, MSI or Microsoft Store), Debian/Ubuntu (amd64), macOS 11+ (Apple Silicon) |
| **Date** | 2026-11-25 |
| **Status** | Self-assessment, not independently audited |

This document gives security, IT and compliance teams the information they need to
evaluate Obsidia before introducing it in a company. It describes how the software works,
which data it touches, what it talks to over the network, which privileges it needs, known
risks and the controls a company should put in place. Section 10 contains a checklist that
can be used directly in an approval process.

---

## 1. Summary

Obsidia is a desktop application (with an additional command line interface) that
installs, updates and restores the firmware of iPhones and iPads connected via USB. It
also manages device backups (create, restore, delete, encryption), lists, installs and
removes apps, copies photos and crash reports to the computer, shows the live system log
and battery / hardware diagnostics, and switches devices into and out of recovery mode.

| Area | Assessment |
|---|---|
| Data processed | Device identifiers, firmware files, battery and hardware details; on user request device backups, photos, app lists, system logs and crash reports (may contain personal data) |
| Data sent to third parties | Device model to ipsw.me; device identifiers to Apple during a restore (required by Apple); update check to GitHub (no device data) |
| Telemetry / analytics | None (update check only, can be disabled) |
| User accounts / credentials | None |
| Network listeners | None |
| Required privileges | Administrator for installation; standard user for operation (see section 6) |
| Code signing | Microsoft Store version: signed by Microsoft. MSI, `.deb` and `.dmg`: **not signed** (see risk R1) |
| Overall risk rating | **Medium** – mainly driven by the destructive nature of firmware restores and unsigned binaries |

---

## 2. Intended use and scope

**Intended use:** IT service desks, device management teams and repair technicians who
need to reinstall, update or recover company-owned iPhones and iPads.

**Out of scope:** jailbreaking, bypassing Activation Lock, downgrading to firmware that
Apple no longer signs, or modifying firmware. Obsidia only installs original Apple
firmware that Apple currently authorizes; the device itself verifies Apple's signatures.

---

## 3. Architecture

```
+-------------------- workstation --------------------+
|                                                     |
|  Obsidia (GUI / CLI, Rust)                          |
|    |  firmware metadata + downloads (HTTPS)  -------+----> api.ipsw.me
|    |                                         -------+----> Apple CDN (firmware files)
|    |                                                |
|    +-- starts helper tools (child processes)        |
|          idevice_id, ideviceinfo, irecovery,        |
|          idevicerestore, idevicebackup2,            |
|          ideviceenterrecovery, idevicediagnostics,  |
|          ideviceinstaller, idevicesyslog,           |
|          idevicecrashreport, idevicescreenshot,     |
|          afcclient                                  |
|            |   restore authorization (HTTPS) -------+----> Apple signing server
|            |                                        |
|            +-- USB via usbmuxd / Apple Mobile Device Service
|                                                     |
+---------------------------+-------------------------+
                            | USB
                     iPhone / iPad
```

| Component | Description | Origin |
|---|---|---|
| `obsidia` | Main program (GUI and CLI) | This project, Rust |
| libimobiledevice tools | Talk to the device over USB | libimobiledevice project (LGPL) |
| `idevicerestore` | Performs the firmware restore / update | libimobiledevice project (LGPL) |
| `ideviceinstaller`, `afcclient`, `idevicesyslog`, `idevicecrashreport`, `idevicediagnostics` | App management, photo export, system log, crash reports, battery data | libimobiledevice project (LGPL) |
| Apple Mobile Device Service (Windows) / `usbmuxd` (Linux) | USB multiplexing to the device | Apple / libimobiledevice |

On Windows the helper tools are shipped inside the MSI / MSIX package (`<install dir>\tools`).
On Linux and macOS they come from the system package manager.

---

## 4. Data inventory

| Data | Where it is stored | Leaves the workstation? | Classification |
|---|---|---|---|
| Device model (product type, e.g. `iPhone14,5`) | Memory only | Yes – to ipsw.me to list firmware | Public |
| UDID, ECID, serial number, IMEI, device name, OS version, battery health, capacity, "Find My" state | Memory, shown in the UI and log window | ECID and related identifiers are sent to Apple during a restore (required by Apple's restore process) | Internal / personal data if the device is assigned to a person |
| Firmware files (`.ipsw`) and their metadata (`.ipsw.json`) | `C:\Obsidia\Firmware` on Windows (shared by all users), `Documents/Obsidia/Firmware` on Linux and macOS | No | Public (Apple software) |
| Device backups | `Documents/Obsidia/Backups` | No | **Confidential** – may contain personal and business data |
| Restore logs | `Documents/Obsidia/Logs` | No | Internal – contain device identifiers (ECID, serial number) |
| Saved device logs (console) | `Documents/Obsidia/Logs` | No | **Confidential** – may contain app and user activity |
| Device reports (battery, IMEI, serial number) | `Documents/Obsidia/Reports` | No | Internal – device identifiers |
| Exported photos, screenshots, crash reports | `Documents/Obsidia/Photos`, `Screenshots`, `CrashReports` | No | **Confidential** – personal data from the device |
| List of installed apps | Memory only | No | Internal / personal data if the device is assigned to a person |
| Apple driver package (Microsoft Store version) | `Documents/Obsidia/Drivers` | No | Public (Apple software) |
| Activity log | Memory only, can be copied by the user | No | Internal |
| Settings (auto-clean, automatic download, update check, language) | `obsidia/settings.json` in the user config folder | No | Internal |

Storage locations: firmware is kept in `C:\Obsidia\Firmware` on Windows, a folder the installer
creates with modify rights for the local *Users* group so that all users share the downloads
(firmware files are public Apple software). All other user data lives in `Documents/Obsidia` (`%USERPROFILE%\Documents\Obsidia`
on Windows, `~/Documents/Obsidia` on Linux and macOS). Update installers are kept in the
user cache folder (`%LOCALAPPDATA%\obsidia\updates`, `~/.cache/obsidia/updates`,
`~/Library/Caches/obsidia/updates`). Settings are stored in `%APPDATA%\obsidia`,
`~/.config/obsidia` or `~/Library/Application Support/obsidia`.

Downloaded firmware is reused as long as Apple still signs it. With **auto-clean** enabled,
files that Apple no longer signs are deleted automatically.

**Backups:** Backups are created and restored with `idevicebackup2`. They are only
encrypted if backup encryption is enabled on the device, which Obsidia can turn on and off
(*Device > Backups*). Without it, backups contain readable personal data (messages,
photos, contacts, app data) on the workstation's disk. Backup passwords are entered in a
dialog, passed to `idevicebackup2` through an environment variable (not on the command
line) and are never stored.

**Photos, system log, crash reports:** only copied or shown when a user starts it on the
*Device* tab. They stay on the workstation in the folders listed above.

Obsidia does not store passwords, Apple ID credentials or any other secrets.

---

## 5. Network communication

| Destination | Purpose | Protocol | Data sent | Initiated by |
|---|---|---|---|---|
| `api.ipsw.me` | Firmware list and signing status | HTTPS | Device model identifier | Obsidia |
| `updates.cdn-apple.com` and other Apple CDN hosts | Firmware download | HTTPS | None beyond the request | Obsidia |
| `gs.apple.com` | Restore authorization (Apple signing server) | HTTPS | Device identifiers such as ECID, firmware build | `idevicerestore` |
| `www.apple.com`, `secure-appldnld.apple.com` | Windows only, on demand: download of the official iTunes installer for the classic Apple USB drivers (*Toolbox > Apple Drivers* or the "Get Apple drivers" banner) | HTTPS | None beyond the request | Obsidia |
| `api.github.com` | Update check (latest release) on start-up and on demand | HTTPS | None beyond the request (IP address, user agent with version) | Obsidia |
| `apt.mirranet.de` | Linux only, if the MirraNET APT repository is configured: package index and package downloads by `apt` (signed with the MirraNET key) | HTTPS | None beyond the request | `apt` |
| `github.com`, `objects.githubusercontent.com` / `release-assets.githubusercontent.com` | Download of an update installer and `SHA256SUMS`, only after the user clicks *Install update* | HTTPS | None beyond the request | Obsidia |

- No inbound connections, no listening ports.
- No telemetry, crash reporting or analytics. The only background request is the update
  check against GitHub (see section 5.1).
- The Apple driver installer is only used after the user starts *Toolbox > Apple Drivers*.
  Obsidia checks that the installer and the extracted `AppleMobileDeviceSupport64.msi`
  carry a valid Authenticode signature issued to Apple Inc. before installing, and the
  installation runs through Windows Installer with a UAC prompt. Optionally the Microsoft
  Store packages `AppleInc.AppleDevices` / `AppleInc.iTunes` are removed for the current
  user. The Microsoft Store version installs nothing itself: it only places the verified
  driver package in `Documents/Obsidia/Drivers` and opens the folder, the user runs it.
  Organisations that distribute Apple drivers centrally can skip this feature.
- On Windows Obsidia checks with `sc query` whether the Apple Mobile Device Service is
  installed and running and shows a notice if it is not. This is a local check.
- Firmware downloads are verified against the SHA-1 checksum published by ipsw.me.
  Independently of that, the device only accepts firmware that is signed by Apple and
  personalized for that device by Apple's signing server.

**Proxies and TLS inspection:**
- Obsidia honours the standard `HTTPS_PROXY` / `HTTP_PROXY` environment variables.
- Obsidia trusts the operating system certificate store in addition to a built-in Mozilla
  CA list, so corporate TLS inspection works if the inspection root CA is deployed to the
  workstations. The helper tools (`idevicerestore`) use their own CA bundle on Windows;
  exempt `gs.apple.com` from inspection if restores fail.
- The ipsw.me endpoint can be replaced with an internal, compatible mirror via the
  `OBSIDIA_API` environment variable.

Firewall allow list: `api.ipsw.me`, `*.cdn-apple.com`, `gs.apple.com` (TCP 443), `www.apple.com` and
`secure-appldnld.apple.com` for the optional driver installer, plus
`api.github.com`, `github.com` and `*.githubusercontent.com` if the updater is used.

### 5.1 Updater

- On start-up Obsidia asks GitHub for the latest published release of `LinzN/obsidia`.
  Drafts and pre-releases are ignored. Nothing is downloaded automatically.
- If a newer version exists, an info bar offers *Install update*. Only then the installer
  for the platform (MSI / `.deb` / `.dmg`) is downloaded and verified against the release's
  `SHA256SUMS` file. A mismatch aborts the update.
- Windows: the MSI is started with `msiexec /passive` and asks for elevation (UAC); Obsidia
  closes itself. Linux: `pkexec apt-get install`. macOS: the disk image is opened.
- The checksum file comes from the same release as the installer. It protects against
  corrupted downloads, but not against a compromised GitHub account (see R11).
- **Company control:** set the policy value
  `HKLM\Software\Policies\MirraNET\Obsidia\DisableUpdates = 1` (DWORD, deployable via Group
  Policy) or the system environment variable `OBSIDIA_NO_UPDATE_CHECK=1` to disable the
  check and roll out updates through software distribution instead. Users can also turn
  the check off in the settings. Users without administrator rights are informed about
  new versions but are not offered the installer.
  `OBSIDIA_UPDATE_URL` can point to an internal, GitHub-compatible release endpoint.
  Standard users cannot install the per-machine MSI without administrator rights.

---

## 6. Privileges and system changes

| Platform | Installation | Operation | System changes |
|---|---|---|---|
| Windows (MSI) | Per-machine, requires administrator | Standard user | Files in `Program Files\Obsidia`, start menu and desktop shortcut, uninstall entry, registry key `HKLM\Software\MirraNET\Obsidia`, folder `C:\Obsidia\Firmware` with modify rights for *Users* (kept on uninstall) |
| Windows (Microsoft Store) | MSIX, per user, no administrator needed | Standard user (full trust desktop app) | Package managed by Windows; firmware folder `C:\Obsidia\Firmware` created on first start; no drivers or services installed |
| Linux | `.deb`, requires root | Standard user with USB access via `usbmuxd` | Binary in `/usr/bin`, desktop entry, icons; depends on `libimobiledevice-utils`, `irecovery`, `idevicerestore`, `ideviceinstaller`, `usbmuxd` |
| macOS | Drag & drop `.app` | Standard user | App in `/Applications`; tools installed via Homebrew |

Obsidia does not install services, scheduled tasks or autostart entries, and installs drivers only when a user starts *Toolbox > Apple Drivers* (section 5). Updates run only when a user starts them (section 5.1). On
Windows the Apple USB drivers come from Apple (Apple Devices app or iTunes) and must be
approved separately.

Silent installation and removal on Windows:

```
msiexec /i obsidia-<version>-x64.msi /qn
msiexec /x obsidia-<version>-x64.msi /qn
```

---

## 7. Supply chain

| Item | Status |
|---|---|
| Source code | Public on GitHub (`LinzN/obsidia`), MIT license |
| Language | Rust (memory-safe); no `unsafe` code in the application except one Windows console API call |
| Third-party Rust crates | ~450 crates in the dependency tree (GUI, HTTP, TLS); versions pinned via `Cargo.lock` |
| Bundled native tools (Windows) | libplist, libimobiledevice-glue, libusbmuxd, libtatsu, libimobiledevice, libirecovery, idevicerestore and ideviceinstaller, built from the current upstream sources on every build (versions in `tools/VERSIONS.txt`); OpenSSL, curl, libzip and other runtime DLLs from MSYS2 packages |
| Build | GitHub Actions on GitHub-hosted runners; release artifacts are built from the tagged commit |
| Code signing | Microsoft Store package signed by Microsoft; MSI, executables and macOS app are unsigned / not notarized |
| SBOM | Not yet published |
| Vulnerability scanning | Not yet automated |

Licenses of bundled components: MIT / Apache-2.0 (Rust crates), LGPL-2.1 / LGPL-3.0
(libimobiledevice tools, shipped as separate executables and libraries).

---

## 8. Risk register

| ID | Risk | Likelihood | Impact | Mitigation | Status |
|---|---|---|---|---|---|
| R1 | Unsigned binaries (MSI, `.deb`, `.dmg`) cannot be verified for origin and integrity; blocked by Smart App Control / application control | High | Medium | Use the Microsoft Store version (signed by Microsoft) where possible; otherwise allow by file hash and download only from the official release page | Partially mitigated |
| R2 | Erase restore deletes all data on a device by mistake (also several devices at once) | Medium | High | Confirmation dialog listing every device and the firmware, additional devices must be ticked one by one; warning when "Find My" is on; backup function; restrict usage to trained staff | Partially mitigated |
| R3 | Unencrypted device backups on workstation disks | Medium | High | Enable backup encryption on devices, store backups on encrypted volumes, delete when no longer needed | Organizational |
| R4 | Restored device loses MDM supervision / enrollment | Medium | Medium | Use Automated Device Enrollment so devices re-enroll after restore | Organizational |
| R5 | Activation Lock blocks a restored device | Medium | Medium | Remove Activation Lock via MDM / Apple Business Manager before restoring | Organizational |
| R6 | Compromised firmware download | Low | Low | HTTPS, SHA-1 check, device verifies Apple signature – manipulated firmware cannot be installed | Mitigated |
| R7 | Compromised third-party dependency (supply chain) | Low | High | Pinned `Cargo.lock`; add `cargo audit` / `cargo deny` and SBOM to CI | Open |
| R8 | Outdated bundled native tools | Medium | Medium | Tools are refreshed on every release build; document versions in release notes | Partially mitigated |
| R9 | Availability of ipsw.me (third-party service) | Low | Low | Internal mirror via `OBSIDIA_API`; local IPSW import | Mitigated |
| R10 | Device identifiers shared with Apple and ipsw.me | High | Low | Required for the restore process; document in the record of processing activities | Accepted |
| R11 | Malicious update through a compromised GitHub account or release pipeline | Low | High | Update needs user action and admin rights; SHA-256 check; companies disable the updater (`OBSIDIA_NO_UPDATE_CHECK=1`) and distribute reviewed versions. The Store version is updated only through Microsoft's certification | Partially mitigated |
| R12 | Personal data copied to the workstation (photos, system logs, crash reports, backups) | Medium | High | Only on explicit user action; define who may use these functions, store on encrypted volumes, delete when no longer needed | Organizational |
| R13 | Unwanted apps installed on managed devices via `.ipa` files | Low | Medium | iOS only installs apps with a valid Apple signature/provisioning; manage allowed apps through MDM; restrict usage to trained staff | Organizational |

Rating scale: Low / Medium / High.

---

## 9. Compliance considerations

- **GDPR / data protection:** Device identifiers and backups can be personal data if a
  device is assigned to an employee. Add Obsidia to the record of processing activities,
  define who may create backups, where they are stored and when they are deleted. Apple
  acts as an independent controller for the restore authorization.
- **Device ownership:** Only use Obsidia on company-owned devices or with the owner's
  consent. A restore cannot be undone.
- **Works council / employee monitoring:** Obsidia does not monitor users, but backups give
  access to personal content. Involve the works council where required.
- **Licensing:** MIT for Obsidia; LGPL for the bundled tools (no copyleft obligations for
  internal use; redistribution must keep the license notices).
- **Export control:** Uses standard TLS only; no custom cryptography.

---

## 10. Onboarding checklist

Use this checklist in the software approval process.

**Approval**
- [ ] Business owner and purpose documented (who uses Obsidia, for which devices)
- [ ] Risk register (section 8) reviewed and residual risks accepted
- [ ] Record of processing activities updated (section 9)
- [ ] License review completed (MIT, LGPL)

**Installation and hardening**
- [ ] Installer downloaded from the official GitHub release page only
- [ ] File hash of the installer recorded and checked before distribution
- [ ] Application control (AppLocker / WDAC / Smart App Control) rule created for Obsidia and its `tools` folder
- [ ] Apple USB drivers (Apple Devices app, iTunes or Apple Mobile Device Support) approved
- [ ] Decision on MSI or Microsoft Store version (the Store version is signed and updated by Microsoft)
- [ ] Distribution via software deployment (silent MSI install) to authorized workstations only
- [ ] Firewall / proxy allows `api.ipsw.me`, `*.cdn-apple.com`, `gs.apple.com`; hosts excluded from TLS inspection

**Data protection**
- [ ] Backup storage location defined, on an encrypted volume
- [ ] Backup encryption enabled on managed devices
- [ ] Retention and deletion rules for backups, restore logs and firmware defined (auto-clean on or off)
- [ ] Rules for copying photos, system logs and crash reports from devices (who, why, how long kept)

**Device management**
- [ ] Devices enrolled via Automated Device Enrollment so they re-enroll after a restore
- [ ] Process to remove Activation Lock before a restore
- [ ] Staff trained on the difference between "Keep data" and "Erase"

**Operation**
- [ ] Owner for updates assigned; new releases reviewed before rollout
- [ ] Decision on the built-in updater: allowed, or disabled via policy (`DisableUpdates`) and updates distributed centrally
- [ ] Process for reporting security issues (GitHub issues / maintainer contact)

---

## 11. Recommended improvements (vendor roadmap)

1. Authenticode-sign the MSI, `obsidia.exe` and all bundled tools; notarize the macOS app
   (the Microsoft Store package is already signed by Microsoft).
2. Publish an SBOM (CycloneDX) with every release (`SHA256SUMS` is already published).
3. Add `cargo audit` and `cargo deny` to CI and fail builds on known vulnerabilities.
4. Verify firmware with SHA-256 where available.
5. Optional admin settings (e.g. disable "Erase", fixed backup location) via registry / config file.
6. Optional audit log written to disk (who restored which device, when).
7. Sign update metadata (e.g. minisign) so updates can be verified independently of GitHub.

---

## 12. Contact

Security issues can be reported via the GitHub repository
(`https://github.com/LinzN/obsidia`). Please do not publish details of unfixed
vulnerabilities in public issues.
