//! Firmware metadata (ipsw.me API) and IPSW downloads.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DEFAULT_API_BASE: &str = "https://api.ipsw.me/v4";

/// ipsw.me compatible API base URL; can be overridden with `OBSIDIA_API`
/// (e.g. to use a mirror).
fn api_base() -> String {
    std::env::var("OBSIDIA_API").unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}
const USER_AGENT: &str = concat!("obsidia/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub firmwares: Vec<Firmware>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Firmware {
    pub version: String,
    pub buildid: String,
    pub url: String,
    #[serde(default)]
    pub sha1sum: Option<String>,
    #[serde(default)]
    pub filesize: u64,
    #[serde(default)]
    pub releasedate: Option<String>,
    #[serde(default)]
    pub signed: bool,
}

impl Firmware {
    pub fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or("firmware.ipsw")
    }

    pub fn release_day(&self) -> &str {
        self.releasedate
            .as_deref()
            .and_then(|d| d.get(..10))
            .unwrap_or("")
    }

    /// Path in the cache; exists once the download has completed.
    pub fn cache_path(&self) -> Result<PathBuf> {
        Ok(cache_dir()?.join(self.file_name()))
    }

    pub fn is_cached(&self) -> bool {
        self.cache_path().map(|p| p.is_file()).unwrap_or(false)
    }
}

/// Entry of the full device catalog (`/devices`).
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub identifier: String,
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .timeout(None)
        .build()?)
}

pub fn fetch_device_info(product: &str) -> Result<DeviceInfo> {
    http()?
        .get(format!("{}/device/{product}?type=ipsw", api_base()))
        .send()?
        .error_for_status()
        .with_context(|| format!("unknown product type '{product}'"))?
        .json()
        .context("invalid response from ipsw.me")
}

/// All iPhones and iPads known to ipsw.me, sorted by name.
pub fn fetch_catalog() -> Result<Vec<CatalogEntry>> {
    let mut all: Vec<CatalogEntry> = http()?
        .get(format!("{}/devices", api_base()))
        .send()?
        .error_for_status()?
        .json()
        .context("invalid response from ipsw.me")?;
    all.retain(|d| d.identifier.starts_with("iPhone") || d.identifier.starts_with("iPad"));
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

pub fn resolve_firmware(product: &str, version: Option<&str>) -> Result<Firmware> {
    let info = fetch_device_info(product)?;
    let signed: Vec<_> = info.firmwares.into_iter().filter(|f| f.signed).collect();
    if signed.is_empty() {
        bail!("Apple is currently not signing any firmware for {product}");
    }
    match version {
        None => Ok(signed[0].clone()), // API lists newest first
        Some(v) => signed
            .iter()
            .find(|f| f.version == v || f.buildid == v)
            .cloned()
            .ok_or_else(|| {
                let list: Vec<_> = signed.iter().map(|f| f.version.as_str()).collect();
                anyhow!(
                    "firmware {v} is not signed for {product} (signed: {})",
                    list.join(", ")
                )
            }),
    }
}

pub fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| anyhow!("no cache directory available"))?;
    let dir = base.join("obsidia");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn sha1_of(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn verify(path: &Path, fw: &Firmware) -> Result<bool> {
    match &fw.sha1sum {
        Some(sum) if !sum.is_empty() => Ok(sha1_of(path)?.eq_ignore_ascii_case(sum)),
        _ => Ok(true),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Progress {
    Downloading { done: u64, total: u64 },
    Verifying,
}

/// Download an IPSW into the cache (resumable) and verify its checksum.
/// `progress` is called regularly; setting `cancel` aborts the download
/// (the partial file is kept so it can be resumed later).
pub fn download(
    fw: &Firmware,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    let path = fw.cache_path()?;
    let part = path.with_extension("ipsw.part");

    if path.exists() {
        progress(Progress::Verifying);
        if verify(&path, fw)? {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    let mut offset = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = http()?.get(&fw.url);
    if offset > 0 {
        req = req.header("Range", format!("bytes={offset}-"));
    }
    let mut resp = req.send()?.error_for_status()?;
    if offset > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        offset = 0; // server ignored the Range header
    }

    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .append(offset > 0)
        .truncate(offset == 0)
        .open(&part)?;

    let total = if fw.filesize > 0 {
        fw.filesize
    } else {
        offset + resp.content_length().unwrap_or(0)
    };
    let mut done = offset;
    progress(Progress::Downloading { done, total });

    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("download cancelled");
        }
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(Progress::Downloading { done, total });
    }
    out.flush()?;
    drop(out);

    progress(Progress::Verifying);
    if !verify(&part, fw)? {
        fs::remove_file(&part)?;
        bail!("SHA-1 mismatch - download corrupted, please retry");
    }
    fs::rename(&part, &path)?;
    Ok(path)
}
