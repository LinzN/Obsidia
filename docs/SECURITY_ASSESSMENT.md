# Obsidia – Security Assessment

| | |
|---|---|
| **Document type** | Vendor self-assessment for software onboarding |
| **Product** | Obsidia – iPhone / iPad firmware tool |
| **Version covered** | 0.4.x |
| **Platforms** | Windows 10/11 (x64), Debian/Ubuntu (amd64), macOS 11+ (Apple Silicon) |
| **Date** | 2026-11-20 |
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
also creates local backups and switches devices into and out of recovery mode.

| Area | Assessment |
|---|---|
| Data processed | Device identifiers, firmware files, optional device backups (may contain personal data) |
| Data sent to third parties | Device model to ipsw.me; device identifiers to Apple during a restore (required by Apple); update check to GitHub (no device data) |
| Telemetry / analytics | None (update check only, can be disabled) |
| User accounts / credentials | None |
| Network listeners | None |
| Required privileges | Administrator for installation; standard user for operation (see section 6) |
| Code signing | **Not signed yet** (see risk R1) |
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
|          ideviceenterrecovery                       |
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
| Apple Mobile Device Service (Windows) / `usbmuxd` (Linux) | USB multiplexing to the device | Apple / libimobiledevice |

On Windows the helper tools are shipped inside the MSI (`<install dir>\tools`). On Linux
and macOS they come from the system package manager.

---

## 4. Data inventory

| Data | Where it is stored | Leaves the workstation? | Classification |
|---|---|---|---|
| Device model (product type, e.g. `iPhone14,5`) | Memory only | Yes – to ipsw.me to list firmware | Public |
| UDID, ECID, serial number, device name, OS version, battery level, capacity | Memory, shown in the UI and log window | ECID and related identifiers are sent to Apple during a restore (required by Apple's restore process) | Internal / personal data if the device is assigned to a person |
| Firmware files (`.ipsw`) | Cache folder (see below) | No | Public (Apple software) |
| Device backups | `Documents/Obsidia Backups` | No | **Confidential** – may contain personal and business data |
| Activity log | Memory only, can be copied by the user | No | Internal |

Storage locations:

| Platform | Firmware cache | Backups |
|---|---|---|
| Windows | `%LOCALAPPDATA%\obsidia` | `%USERPROFILE%\Documents\Obsidia Backups` |
| Linux | `~/.cache/obsidia` | `~/Documents/Obsidia Backups` |
| macOS | `~/Library/Caches/obsidia` | `~/Documents/Obsidia Backups` |

**Backups:** Backups are created with `idevicebackup2`. They are only encrypted if backup
encryption is enabled on the device itself. Without it, backups contain readable personal
data (messages, photos, contacts, app data) on the workstation's disk.

Obsidia does not store passwords, Apple ID credentials or any other secrets, and it has no
configuration files.

---

## 5. Network communication

| Destination | Purpose | Protocol | Data sent | Initiated by |
|---|---|---|---|---|
| `api.ipsw.me` | Firmware list and signing status | HTTPS | Device model identifier | Obsidia |
| `updates.cdn-apple.com` and other Apple CDN hosts | Firmware download | HTTPS | None beyond the request | Obsidia |
| `gs.apple.com` | Restore authorization (Apple signing server) | HTTPS | Device identifiers such as ECID, firmware build | `idevicerestore` |
| `api.github.com` | Update check (latest release) on start-up and on demand | HTTPS | None beyond the request (IP address, user agent with version) | Obsidia |
| `github.com`, `objects.githubusercontent.com` / `release-assets.githubusercontent.com` | Download of an update installer and `SHA256SUMS`, only after the user clicks *Install update* | HTTPS | None beyond the request | Obsidia |

- No inbound connections, no listening ports.
- No telemetry, crash reporting or analytics. The only background request is the update
  check against GitHub (see section 5.1).
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

Firewall allow list: `api.ipsw.me`, `*.cdn-apple.com`, `gs.apple.com` (TCP 443), plus
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
- **Company control:** set `OBSIDIA_NO_UPDATE_CHECK=1` (system environment variable) to
  disable the automatic check and roll out updates through software distribution instead.
  `OBSIDIA_UPDATE_URL` can point to an internal, GitHub-compatible release endpoint.
  Standard users cannot install the per-machine MSI without administrator rights.

---

## 6. Privileges and system changes

| Platform | Installation | Operation | System changes |
|---|---|---|---|
| Windows | MSI, per-machine, requires administrator | Standard user | Files in `Program Files\Obsidia`, start menu and desktop shortcut, uninstall entry, registry key `HKLM\Software\MirraNET\Obsidia` |
| Linux | `.deb`, requires root | Standard user with USB access via `usbmuxd` | Binary in `/usr/bin`, desktop entry, icons; depends on `libimobiledevice-utils`, `irecovery`, `idevicerestore`, `usbmuxd` |
| macOS | Drag & drop `.app` | Standard user | App in `/Applications`; tools installed via Homebrew |

Obsidia does not install services, drivers, scheduled tasks or autostart entries. Updates run only when a user starts them (section 5.1). On
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
| Bundled native tools (Windows) | libimobiledevice, libirecovery, idevicerestore and their DLLs, taken from MSYS2 packages at build time |
| Build | GitHub Actions on GitHub-hosted runners; release artifacts are built from the tagged commit |
| Code signing | **Not yet** – MSI, executables and macOS app are unsigned / not notarized |
| SBOM | Not yet published |
| Vulnerability scanning | Not yet automated |

Licenses of bundled components: MIT / Apache-2.0 (Rust crates), LGPL-2.1 / LGPL-3.0
(libimobiledevice tools, shipped as separate executables and libraries).

---

## 8. Risk register

| ID | Risk | Likelihood | Impact | Mitigation | Status |
|---|---|---|---|---|---|
| R1 | Unsigned binaries cannot be verified for origin and integrity; blocked by Smart App Control / application control | High | Medium | Sign MSI, executables and bundled tools; notarize macOS app. Until then: allow by file hash, download only from the official release page | Open |
| R2 | Erase restore deletes all data on a device by mistake | Medium | High | Confirmation dialog showing device and firmware; backup function; restrict usage to trained staff | Partially mitigated |
| R3 | Unencrypted device backups on workstation disks | Medium | High | Enable backup encryption on devices, store backups on encrypted volumes, delete when no longer needed | Organizational |
| R4 | Restored device loses MDM supervision / enrollment | Medium | Medium | Use Automated Device Enrollment so devices re-enroll after restore | Organizational |
| R5 | Activation Lock blocks a restored device | Medium | Medium | Remove Activation Lock via MDM / Apple Business Manager before restoring | Organizational |
| R6 | Compromised firmware download | Low | Low | HTTPS, SHA-1 check, device verifies Apple signature – manipulated firmware cannot be installed | Mitigated |
| R7 | Compromised third-party dependency (supply chain) | Low | High | Pinned `Cargo.lock`; add `cargo audit` / `cargo deny` and SBOM to CI | Open |
| R8 | Outdated bundled native tools | Medium | Medium | Tools are refreshed on every release build; document versions in release notes | Partially mitigated |
| R9 | Availability of ipsw.me (third-party service) | Low | Low | Internal mirror via `OBSIDIA_API`; local IPSW import | Mitigated |
| R10 | Device identifiers shared with Apple and ipsw.me | High | Low | Required for the restore process; document in the record of processing activities | Accepted |
| R11 | Malicious update through a compromised GitHub account or release pipeline | Low | High | Update needs user action and admin rights; SHA-256 check; companies disable the updater (`OBSIDIA_NO_UPDATE_CHECK=1`) and distribute reviewed versions. Planned: signed installers | Partially mitigated |

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
- [ ] Apple USB drivers (Apple Devices app or iTunes) approved
- [ ] Distribution via software deployment (silent MSI install) to authorized workstations only
- [ ] Firewall / proxy allows `api.ipsw.me`, `*.cdn-apple.com`, `gs.apple.com`; hosts excluded from TLS inspection

**Data protection**
- [ ] Backup storage location defined, on an encrypted volume
- [ ] Backup encryption enabled on managed devices
- [ ] Retention and deletion rules for backups and firmware cache defined

**Device management**
- [ ] Devices enrolled via Automated Device Enrollment so they re-enroll after a restore
- [ ] Process to remove Activation Lock before a restore
- [ ] Staff trained on the difference between "Keep data" and "Erase"

**Operation**
- [ ] Owner for updates assigned; new releases reviewed before rollout
- [ ] Decision on the built-in updater: allowed, or disabled via `OBSIDIA_NO_UPDATE_CHECK=1` and updates distributed centrally
- [ ] Process for reporting security issues (GitHub issues / maintainer contact)

---

## 11. Recommended improvements (vendor roadmap)

1. Authenticode-sign the MSI, `obsidia.exe` and all bundled tools; notarize the macOS app.
2. Publish an SBOM (CycloneDX) and checksums (`SHA256SUMS`) with every release.
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
