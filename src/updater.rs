//! Self-update from GitHub releases.
//!
//! Looks at the latest (non pre-release) release, picks the installer for the
//! current platform, verifies it against the release's SHA256SUMS and starts it.
//! Set `OBSIDIA_NO_UPDATE_CHECK=1` to disable the automatic check,
//! `OBSIDIA_UPDATE_URL` overrides the release endpoint.

use crate::firmware;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const REPO: &str = "LinzN/obsidia";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Update {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub asset: Option<Asset>,
    checksums_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

/// `OBSIDIA_UPDATE_URL` can point to a GitHub-compatible "latest release"
/// endpoint, e.g. an internal mirror.
fn latest_release_url() -> String {
    std::env::var("OBSIDIA_UPDATE_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"))
}

/// False if updates are turned off by the administrator: environment variable
/// `OBSIDIA_NO_UPDATE_CHECK=1` or, on Windows, the policy value
/// `HKLM\Software\Policies\MirraNET\Obsidia\DisableUpdates = 1`.
pub fn auto_check_enabled() -> bool {
    let env_off = matches!(
        std::env::var("OBSIDIA_NO_UPDATE_CHECK").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    !env_off && !disabled_by_policy()
}

fn disabled_by_policy() -> bool {
    if !cfg!(windows) {
        return false;
    }
    #[allow(unused_mut)]
    let mut cmd = Command::new("reg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.args([
        "query",
        r"HKLM\Software\Policies\MirraNET\Obsidia",
        "/v",
        "DisableUpdates",
    ])
    .stderr(std::process::Stdio::null())
    .output()
    .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("0x1"))
    .unwrap_or(false)
}

/// Whether this user can install an update. The Windows MSI installs for all
/// users and needs administrator rights; standard users can't elevate.
pub fn can_install() -> bool {
    #[cfg(windows)]
    {
        windows_can_elevate()
    }
    #[cfg(target_os = "linux")]
    {
        crate::device::command_exists("pkexec")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        true
    }
}

#[cfg(windows)]
fn windows_can_elevate() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TokenElevationType, TOKEN_ELEVATION,
        TOKEN_ELEVATION_TYPE, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: plain Win32 token queries with correctly sized out buffers.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return true; // unknown - let the installer decide
        }
        let mut kind: TOKEN_ELEVATION_TYPE = 0;
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevationType,
            &mut kind as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut len,
        );
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let ok2 = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        if ok == 0 || ok2 == 0 {
            return true;
        }
        // 2 = full (elevated), 3 = limited admin token that can elevate,
        // 1 = default: standard user unless already elevated (UAC off)
        kind == 2 || kind == 3 || elevation.TokenIsElevated != 0
    }
}

/// Returns the latest release if it is newer than the running version.
pub fn check() -> Result<Option<Update>> {
    let resp = firmware::http()?
        .get(latest_release_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("could not reach GitHub")?;
    // 404: no release published yet
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let release: Release = resp
        .error_for_status()
        .context("could not reach GitHub")?
        .json()
        .context("unexpected response from GitHub")?;

    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = release.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&version, CURRENT_VERSION) {
        return Ok(None);
    }

    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .map(|a| a.url.clone());
    let asset = release
        .assets
        .into_iter()
        .find(|a| matches_platform(&a.name));

    Ok(Some(Update {
        version,
        notes: release.body.unwrap_or_default(),
        page: release.html_url,
        asset,
        checksums_url,
    }))
}

fn matches_platform(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if cfg!(windows) {
        name.ends_with(".msi")
    } else if cfg!(target_os = "macos") {
        name.ends_with(".dmg")
    } else {
        name.ends_with(".deb") && name.contains("amd64")
    }
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Download the installer for this platform and verify its checksum.
pub fn download(
    update: &Update,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let asset = update
        .asset
        .as_ref()
        .ok_or_else(|| anyhow!("this release has no installer for your platform"))?;
    let checksums_url = update
        .checksums_url
        .as_ref()
        .ok_or_else(|| anyhow!("this release has no SHA256SUMS file"))?;

    let expected = expected_checksum(checksums_url, &asset.name)?;

    let dir = firmware::cache_dir()?.join("updates");
    fs::create_dir_all(&dir)?;
    let path = dir.join(&asset.name);

    let mut resp = firmware::http()?
        .get(&asset.url)
        .send()?
        .error_for_status()?;
    let total = resp.content_length().unwrap_or(asset.size);
    let mut file = File::create(&path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = fs::remove_file(&path);
            bail!("download cancelled");
        }
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    drop(file);

    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(&expected) {
        let _ = fs::remove_file(&path);
        bail!("checksum mismatch for {} - update aborted", asset.name);
    }
    Ok(path)
}

fn expected_checksum(url: &str, file_name: &str) -> Result<String> {
    let text = firmware::http()?
        .get(url)
        .send()?
        .error_for_status()?
        .text()?;
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?, parts.next()?.trim_start_matches('*')))
        })
        .find(|(_, name)| *name == file_name)
        .map(|(hash, _)| hash.to_string())
        .ok_or_else(|| anyhow!("{file_name} is not listed in SHA256SUMS"))
}

/// Start the downloaded installer. The caller should exit afterwards so the
/// installer can replace the running program.
pub fn install(path: &Path) -> Result<()> {
    if cfg!(windows) {
        Command::new("msiexec")
            .arg("/i")
            .arg(path)
            .arg("/passive")
            .arg("/norestart")
            .spawn()
            .context("could not start msiexec")?;
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .arg(path)
            .spawn()
            .context("could not open the disk image")?;
    } else if crate::device::command_exists("pkexec") {
        Command::new("pkexec")
            .args(["apt-get", "install", "-y"])
            .arg(path)
            .spawn()
            .context("could not start pkexec")?;
    } else {
        bail!(
            "install the update manually: sudo apt install {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.4.1", "0.4.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.9", "0.4.0"));
        assert!(!is_newer("garbage", "0.4.0"));
        assert!(is_newer("0.5.0-beta", "0.4.0"));
    }
}
