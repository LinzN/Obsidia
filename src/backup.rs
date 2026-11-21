//! Device backups made with `idevicebackup2`.
//!
//! Every backup lives in `<backup dir>/<UDID>` and carries an `Info.plist`
//! with the device details and a `Manifest.plist` that tells whether it is
//! encrypted.

use crate::device::{self, Device};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub dir: PathBuf,
    /// UDID of the device the backup was made from.
    pub udid: String,
    pub device_name: String,
    pub product: String,
    pub version: String,
    pub serial: String,
    /// "YYYY-MM-DD HH:MM" (UTC).
    pub date: String,
    pub size: u64,
    pub encrypted: bool,
}

fn read_plist(path: &Path) -> Option<plist::Dictionary> {
    plist::Value::from_file(path).ok()?.into_dictionary()
}

fn string(dict: &plist::Dictionary, key: &str) -> String {
    dict.get(key)
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string()
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// All backups in `root`, newest first. Walks every backup to sum up its
/// size, so call it off the UI thread.
pub fn list(root: &Path) -> Vec<BackupInfo> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut list: Vec<BackupInfo> = entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let info = read_plist(&dir.join("Info.plist"))?;
            let manifest = read_plist(&dir.join("Manifest.plist"));
            let date = info
                .get("Last Backup Date")
                .and_then(|v| v.as_date())
                .map(|d| {
                    let s = d.to_xml_format();
                    // 2026-11-21T22:10:00Z -> 2026-11-21 22:10
                    s.get(..16).unwrap_or(&s).replace('T', " ")
                })
                .unwrap_or_default();
            Some(BackupInfo {
                udid: e.file_name().to_string_lossy().into_owned(),
                device_name: string(&info, "Device Name"),
                product: string(&info, "Product Type"),
                version: string(&info, "Product Version"),
                serial: string(&info, "Serial Number"),
                date,
                size: dir_size(&dir),
                encrypted: manifest
                    .and_then(|m| m.get("IsEncrypted").and_then(|v| v.as_boolean()))
                    .unwrap_or(false),
                dir,
            })
        })
        .collect();
    list.sort_by(|a, b| b.date.cmp(&a.date));
    list
}

/// Restores `backup` onto `target`, including settings and system files.
/// The device restarts afterwards. "Find My" has to be off.
pub fn restore_cmd(
    target: &Device,
    backup: &BackupInfo,
    password: Option<&str>,
) -> Result<Command> {
    let Some(root) = backup.dir.parent() else {
        bail!("invalid backup folder");
    };
    let mut cmd = device::tool_cmd("idevicebackup2")?;
    cmd.args([
        "-u",
        &target.udid,
        "-s",
        &backup.udid,
        "restore",
        "--system",
        "--settings",
        "--reboot",
    ]);
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        // passed via the environment so it does not show up in process lists
        cmd.env("BACKUP_PASSWORD", pw);
    }
    cmd.arg(root);
    Ok(cmd)
}

/// Whether the device encrypts its backups.
pub fn encryption_enabled(dev: &Device) -> Option<bool> {
    let out = device::run(
        "ideviceinfo",
        &[
            "-u",
            &dev.udid,
            "-q",
            "com.apple.mobile.backup",
            "-k",
            "WillEncrypt",
        ],
    )
    .ok()
    .flatten()?;
    match out.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Turns backup encryption on or off. Turning it off needs the current
/// password; the device may ask for the passcode.
pub fn set_encryption(dev: &Device, on: bool, password: &str) -> Result<String> {
    let _lock = device::shared();
    let mut cmd = device::tool_cmd("idevicebackup2")?;
    cmd.args(["-u", &dev.udid, "encryption", if on { "on" } else { "off" }])
        // the new password when turning it on, the current one when turning it off
        .env(
            if on {
                "BACKUP_PASSWORD_NEW"
            } else {
                "BACKUP_PASSWORD"
            },
            password,
        )
        .stdin(std::process::Stdio::null());
    let out = cmd
        .output()
        .map_err(|e| device::spawn_error("idevicebackup2", e))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if !out.status.success() || last.to_ascii_lowercase().contains("error") {
        bail!(
            "{}",
            if last.is_empty() {
                "failed".into()
            } else {
                last
            }
        );
    }
    Ok(last)
}

pub fn delete(backup: &BackupInfo) -> Result<()> {
    // only ever remove a folder that really is a backup
    if !backup.dir.join("Info.plist").is_file() {
        bail!("{} is not a backup folder", backup.dir.display());
    }
    std::fs::remove_dir_all(&backup.dir)?;
    Ok(())
}
