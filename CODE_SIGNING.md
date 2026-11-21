# Code signing policy

Windows releases of Obsidia are signed with an Authenticode certificate so that Windows,
SmartScreen and Smart App Control can verify where the files come from.

Free code signing provided by [SignPath.io](https://signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

*Status: the application to the SignPath Foundation open-source program is pending. Until
it is approved, releases are published unsigned.*

## What is signed

Only files built from this repository by GitHub Actions on GitHub-hosted runners
(`.github/workflows/build.yml`) are signed:

- `obsidia.exe`
- the bundled libimobiledevice tools (`idevicerestore.exe`, `irecovery.exe`, ...) and their
  libraries, built from upstream source in the same workflow
- the Windows installer (`obsidia-<version>-x64.msi`)

Signing happens only for release builds (tags `v*`) created through the *Release* workflow.
Every signing request has to be approved manually in SignPath before the files are
signed. The artifact configurations are kept in [`packaging/signpath`](packaging/signpath).

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [Niklas Linz (@LinzN)](https://github.com/LinzN) |
| Approvers | [Niklas Linz (@LinzN)](https://github.com/LinzN) |

All team members use multi-factor authentication for GitHub and SignPath.

## Privacy policy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it, with these
exceptions:

- **Update check:** on start-up Obsidia asks the GitHub API for the latest release. No device
  or personal data is sent. The check can be turned off in the settings or by policy
  (see [README](README.md#updates)).
- **Firmware list:** when a device is connected, its model identifier is sent to
  [ipsw.me](https://ipsw.me) to list the available firmware.
- **Restore:** during a restore, device identifiers are sent to Apple's signing server.
  This is required by Apple to install firmware.

Details are in the [security assessment](docs/SECURITY_ASSESSMENT.md).
