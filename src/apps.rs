//! Installed apps, via `ideviceinstaller`.

use crate::device::{self, Device};
use anyhow::{bail, Result};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct App {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    /// App size plus documents and data, in bytes.
    pub size: Option<u64>,
}

/// User-installed apps, sorted by name.
pub fn list(dev: &Device) -> Result<Vec<App>> {
    let _lock = device::shared();
    let (_, out) = device::run_full(
        "ideviceinstaller",
        &[
            "-u",
            &dev.udid,
            "list",
            "--user",
            "--xml",
            "-a",
            "CFBundleIdentifier",
            "-a",
            "CFBundleDisplayName",
            "-a",
            "CFBundleName",
            "-a",
            "CFBundleShortVersionString",
            "-a",
            "StaticDiskUsage",
            "-a",
            "DynamicDiskUsage",
        ],
        std::time::Duration::from_secs(60),
    )?;
    // ideviceinstaller before 1.2 only knows the old option syntax
    let out = if out.contains("<plist") {
        out
    } else {
        device::run_full(
            "ideviceinstaller",
            &["-u", &dev.udid, "-l", "-o", "xml", "-o", "list_user"],
            std::time::Duration::from_secs(60),
        )?
        .1
    };
    let mut apps = parse_list(&out)?;
    apps.sort_by_key(|a| a.name.to_lowercase());
    Ok(apps)
}

fn parse_list(out: &str) -> Result<Vec<App>> {
    let Some(start) = out.find("<?xml").or_else(|| out.find("<plist")) else {
        let msg = out
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no answer");
        bail!("could not read the app list: {}", msg.trim());
    };
    let value = plist::Value::from_reader_xml(&out.as_bytes()[start..])?;
    let Some(items) = value.as_array() else {
        bail!("unexpected app list");
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let d = item.as_dictionary()?;
            let text = |k: &str| d.get(k).and_then(|v| v.as_string()).map(String::from);
            let num = |k: &str| d.get(k).and_then(|v| v.as_unsigned_integer());
            let bundle_id = text("CFBundleIdentifier")?;
            let name = text("CFBundleDisplayName")
                .filter(|n| !n.is_empty())
                .or_else(|| text("CFBundleName"))
                .unwrap_or_else(|| bundle_id.clone());
            let size = match (num("StaticDiskUsage"), num("DynamicDiskUsage")) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
            };
            Some(App {
                bundle_id,
                name,
                version: text("CFBundleShortVersionString").unwrap_or_default(),
                size,
            })
        })
        .collect())
}

fn check((ok, out): (bool, String)) -> Result<()> {
    let error = out
        .lines()
        .find(|l| l.contains("ERROR") || l.contains("Error"))
        .map(|l| l.trim().to_string());
    match (ok, error) {
        (_, Some(e)) => bail!("{e}"),
        (false, None) => bail!(
            "{}",
            out.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
                .trim()
        ),
        (true, None) => Ok(()),
    }
}

pub fn install(dev: &Device, ipa: &Path) -> Result<()> {
    let _lock = device::shared();
    let path = ipa.to_string_lossy();
    legacy_fallback(
        &["-u", &dev.udid, "install", &path],
        &["-u", &dev.udid, "-i", &path],
        600,
    )
}

pub fn uninstall(dev: &Device, bundle_id: &str) -> Result<()> {
    let _lock = device::shared();
    legacy_fallback(
        &["-u", &dev.udid, "uninstall", bundle_id],
        &["-u", &dev.udid, "-U", bundle_id],
        120,
    )
}

/// Runs the current command syntax and falls back to the old one if the
/// installed ideviceinstaller does not know it.
fn legacy_fallback(args: &[&str], legacy: &[&str], timeout: u64) -> Result<()> {
    let timeout = std::time::Duration::from_secs(timeout);
    let (ok, out) = device::run_full("ideviceinstaller", args, timeout)?;
    if out.contains("Usage:") || out.contains("Unknown") {
        return check(device::run_full("ideviceinstaller", legacy, timeout)?);
    }
    check((ok, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_app_list() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><array>
<dict><key>CFBundleIdentifier</key><string>com.example.notes</string>
<key>CFBundleDisplayName</key><string>Notes Pro</string>
<key>CFBundleShortVersionString</key><string>2.1</string>
<key>StaticDiskUsage</key><integer>1000</integer>
<key>DynamicDiskUsage</key><integer>500</integer></dict>
<dict><key>CFBundleIdentifier</key><string>com.example.raw</string></dict>
</array></plist>"#;
        let apps = parse_list(xml).unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Notes Pro");
        assert_eq!(apps[0].size, Some(1500));
        assert_eq!(apps[1].name, "com.example.raw");
    }
}
