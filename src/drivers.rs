//! Classic Apple USB drivers on Windows.
//!
//! Newer devices (iPhone 14 and later in particular) often abort firmware
//! uploads in recovery mode with the drivers that come with the Microsoft
//! Store apps ("Apple Devices", Store iTunes). The drivers from the classic
//! iTunes installer handle these transfers reliably. This module installs
//! only that driver package: it downloads the official iTunes installer from
//! apple.com, checks its Authenticode signature, extracts
//! `AppleMobileDeviceSupport64.msi` and installs it. iTunes itself is not
//! installed.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Apple's permanent link to the current 64-bit iTunes installer.
const ITUNES_URL: &str = "https://www.apple.com/itunes/download/win64";
const DRIVER_MSI: &str = "AppleMobileDeviceSupport64.msi";
/// Store packages that bring their own copy of the device drivers.
const STORE_PACKAGES: [&str; 2] = ["AppleInc.AppleDevices", "AppleInc.iTunes"];

#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Store app ("Apple Devices" or Store iTunes) installed for this user.
    pub store_app: bool,
    /// Apple Mobile Device Support from the classic installer.
    pub classic: Option<String>,
}

pub fn supported() -> bool {
    cfg!(windows)
}

fn hidden(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdin(Stdio::null());
    cmd
}

fn powershell(script: &str) -> Result<String> {
    let out = hidden("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .context("failed to start PowerShell")?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        bail!("PowerShell failed: {first}");
    }
    Ok(stdout)
}

/// Single quotes are the only special character in a PowerShell literal.
fn ps_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

pub fn status() -> Status {
    if !supported() {
        return Status::default();
    }
    let names = STORE_PACKAGES.map(|p| format!("'{p}'")).join(",");
    let store_app = powershell(&format!(
        "@(Get-AppxPackage | Where-Object {{ $_.Name -in {names} }}).Count"
    ))
    .ok()
    .and_then(|n| n.parse::<u32>().ok())
    .is_some_and(|n| n > 0);
    Status {
        store_app,
        classic: classic_version(),
    }
}

/// Version of a classic Apple Mobile Device Support install, from the
/// uninstall entries in the registry.
fn classic_version() -> Option<String> {
    let script = "Get-ItemProperty 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*' \
                  -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -eq 'Apple Mobile Device Support' } \
                  | Select-Object -First 1 -ExpandProperty DisplayVersion";
    powershell(script).ok().filter(|v| !v.is_empty())
}

pub fn remove_store_apps() -> Result<()> {
    let names = STORE_PACKAGES.map(|p| format!("'{p}'")).join(",");
    powershell(&format!(
        "Get-AppxPackage | Where-Object {{ $_.Name -in {names} }} | Remove-AppxPackage"
    ))?;
    Ok(())
}

fn work_dir() -> Result<PathBuf> {
    let dir = crate::firmware::cache_dir()?.join("drivers");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Downloads the iTunes installer (reused if already present and valid).
pub fn download_installer(mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    use std::io::{Read, Write};

    let path = work_dir()?.join("iTunes64Setup.exe");
    if path.is_file() && verify_signature(&path).is_ok() {
        return Ok(path);
    }
    let mut resp = crate::firmware::http()?
        .get(ITUNES_URL)
        .send()?
        .error_for_status()
        .context("downloading the iTunes installer from apple.com failed")?;
    let total = resp.content_length().unwrap_or(0);
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    drop(file);
    std::fs::rename(&part, &path)?;
    verify_signature(&path)?;
    Ok(path)
}

/// The file must carry a valid Authenticode signature issued to Apple Inc.
pub fn verify_signature(path: &Path) -> Result<()> {
    let out = powershell(&format!(
        "$s = Get-AuthenticodeSignature -LiteralPath {}; \"$($s.Status)|$($s.SignerCertificate.Subject)\"",
        ps_quote(path)
    ))?;
    let (state, subject) = out.split_once('|').unwrap_or((out.as_str(), ""));
    if state != "Valid" || !subject.contains("O=Apple Inc.") {
        let _ = std::fs::remove_file(path);
        bail!("the downloaded installer is not validly signed by Apple ({state}, {subject})");
    }
    Ok(())
}

/// Unpacks the installer and returns the driver package.
pub fn extract_driver(installer: &Path) -> Result<PathBuf> {
    let dir = work_dir()?.join("extracted");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    // The bootstrapper writes the MSIs next to itself.
    let exe = dir.join("iTunes64Setup.exe");
    std::fs::copy(installer, &exe)?;
    let status = hidden(&exe.to_string_lossy())
        .arg("/extract")
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to unpack the iTunes installer")?;
    let msi = find_file(&dir, DRIVER_MSI)
        .with_context(|| format!("{DRIVER_MSI} not found in the installer (exit {status})"))?;
    verify_signature(&msi)?;
    Ok(msi)
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        {
            return Some(path);
        }
    }
    None
}

/// Installs the driver package. Windows asks for administrator rights.
pub fn install_msi(msi: &Path) -> Result<()> {
    let log = crate::firmware::logs_dir()
        .map(|d| d.join("apple-drivers-install.log"))
        .unwrap_or_else(|_| msi.with_extension("log"));
    let script = format!(
        "$p = Start-Process -FilePath msiexec.exe -Verb RunAs -Wait -PassThru -ArgumentList \
         @('/i', '\"{}\"', '/passive', '/norestart', '/l*v', '\"{}\"'); exit $p.ExitCode",
        msi.display(),
        log.display()
    );
    let status = hidden("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to start the driver installation")?;
    match status.code() {
        Some(0) | Some(3010) => Ok(()),
        Some(1602) => bail!("installation cancelled"),
        Some(1618) => bail!("another installation is running, try again when it has finished"),
        Some(1) => bail!("administrator rights were not granted"),
        code => bail!(
            "the driver installation failed (exit code {}), see {}",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".into()),
            log.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(
            ps_quote(Path::new("C:\\O'Neil\\a.exe")),
            "'C:\\O''Neil\\a.exe'"
        );
    }
}
