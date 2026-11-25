# Privacy policy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it, with these
exceptions:

- **Update check:** on start-up Obsidia asks the GitHub API for the latest release. No device
  or personal data is sent. The check can be turned off in the settings or by policy
  (see [README](../README.md#updates)); the Microsoft Store version has no own update check.
- **Firmware list:** when a device is connected, its model identifier is sent to
  [ipsw.me](https://ipsw.me) to list the available firmware.
- **Restore:** during a restore, device identifiers are sent to Apple's signing server.
  This is required by Apple to install firmware.
- **Apple drivers (Windows):** only when you ask for it, Apple's official installer is
  downloaded from apple.com. No data is sent beyond the request.

Device information that Obsidia reads over USB – battery and hardware details, the list of
installed apps, the "Find My" state, photos, crash reports and the system log – is only
shown in the program or saved to folders on your computer when you ask for it. It is never
sent anywhere. Backup passwords are only passed to the backup tool and are not stored.

Obsidia has no telemetry, analytics or crash reporting and no user accounts.

Contact: niklas.linz@mirranet.de ([MirraNET](https://mirranet.de))

Details are in the [security assessment](SECURITY_ASSESSMENT.md).
