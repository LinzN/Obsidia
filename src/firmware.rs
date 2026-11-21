//! Firmware metadata (ipsw.me API) and IPSW downloads.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
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
    /// Product type this entry was listed for, e.g. `iPhone14,5`.
    #[serde(default)]
    pub identifier: String,
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

    /// Path in the firmware folder; exists once the download has completed.
    pub fn cache_path(&self) -> Result<PathBuf> {
        Ok(firmware_dir()?.join(self.file_name()))
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

pub(crate) fn http() -> Result<reqwest::blocking::Client> {
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

/// `Documents/Obsidia` - backups and logs live here (and firmware outside Windows).
pub fn data_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Documents")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Obsidia")
}

static CUSTOM_FIRMWARE_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Use a different firmware folder (from the settings); `None` resets it.
pub fn set_firmware_dir(dir: Option<PathBuf>) {
    if let Ok(mut d) = CUSTOM_FIRMWARE_DIR.write() {
        *d = dir;
    }
}

/// Per-user firmware folder: the default on Linux and macOS, and the
/// fallback on Windows if the shared folder can't be written.
fn user_firmware_dir() -> PathBuf {
    data_dir().join("Firmware")
}

/// `C:\Obsidia\Firmware` on Windows, shared by all users of the computer.
/// The installer creates it with write access for all users.
#[cfg(windows)]
fn shared_firmware_dir() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    PathBuf::from(format!("{drive}\\"))
        .join("Obsidia")
        .join("Firmware")
}

#[cfg(windows)]
fn writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    let ok = File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

pub fn default_firmware_dir() -> PathBuf {
    static RESOLVED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            #[cfg(windows)]
            {
                let shared = shared_firmware_dir();
                if writable(&shared) {
                    return shared;
                }
            }
            user_firmware_dir()
        })
        .clone()
}

/// Moves firmware from the per-user folder of earlier versions into the
/// shared folder. Only renames on the same drive, nothing is copied.
pub fn migrate_user_firmware() -> usize {
    let target = default_firmware_dir();
    let old = user_firmware_dir();
    if target == old {
        return 0;
    }
    let Ok(entries) = fs::read_dir(&old) else {
        return 0;
    };
    let mut moved = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        if !(name.ends_with(".ipsw") || name.ends_with(".ipsw.json")) || !path.is_file() {
            continue;
        }
        let dest = target.join(&name);
        if !dest.exists() && fs::rename(&path, &dest).is_ok() && name.ends_with(".ipsw") {
            moved += 1;
        }
    }
    moved
}

/// True if the path is inside a OneDrive folder (synced to the cloud).
pub fn is_cloud_synced(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .starts_with("onedrive")
    })
}

/// Free space on the drive holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    fs2::available_space(path).ok()
}

pub fn firmware_dir() -> Result<PathBuf> {
    let custom = CUSTOM_FIRMWARE_DIR.read().ok().and_then(|d| d.clone());
    let dir = custom.unwrap_or_else(default_firmware_dir);
    fs::create_dir_all(&dir)?;
    migrate_old_cache(&dir);
    Ok(dir)
}

pub fn logs_dir() -> Result<PathBuf> {
    let dir = data_dir().join("Logs");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Older versions kept firmware in the cache folder; move it over once.
fn migrate_old_cache(target: &Path) {
    let Some(old) = dirs::cache_dir().map(|d| d.join("obsidia")) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&old) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_firmware = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".ipsw") || n.ends_with(".ipsw.part"));
        if is_firmware {
            if let Some(name) = path.file_name() {
                let _ = fs::rename(&path, target.join(name));
            }
        }
    }
}

/// Cache folder for temporary files such as update installers.
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

/// Stored next to each downloaded IPSW (`<name>.ipsw.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirmwareMeta {
    pub product: String,
    pub version: String,
    pub buildid: String,
    #[serde(default)]
    pub sha1sum: Option<String>,
    #[serde(default)]
    pub size: u64,
}

fn meta_path(ipsw: &Path) -> PathBuf {
    ipsw.with_extension("ipsw.json")
}

pub fn read_meta(ipsw: &Path) -> Option<FirmwareMeta> {
    let text = fs::read_to_string(meta_path(ipsw)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_meta(ipsw: &Path, fw: &Firmware) {
    let meta = FirmwareMeta {
        product: fw.identifier.clone(),
        version: fw.version.clone(),
        buildid: fw.buildid.clone(),
        sha1sum: fw.sha1sum.clone(),
        size: fs::metadata(ipsw).map(|m| m.len()).unwrap_or(0),
    };
    if let Ok(json) = serde_json::to_string_pretty(&meta) {
        let _ = fs::write(meta_path(ipsw), json);
    }
}

/// An IPSW in the firmware folder.
#[derive(Debug, Clone)]
pub struct LocalFirmware {
    pub path: PathBuf,
    pub size: u64,
    pub product: Option<String>,
    pub version: Option<String>,
    pub buildid: Option<String>,
}

impl LocalFirmware {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

pub fn local_firmware() -> Vec<LocalFirmware> {
    let Ok(dir) = firmware_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<LocalFirmware> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ipsw"))
        })
        .map(|path| {
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let meta = read_meta(&path);
            let (product, buildid) = match &meta {
                Some(m) => (Some(m.product.clone()), Some(m.buildid.clone())),
                None => parse_file_name(&path),
            };
            LocalFirmware {
                version: meta.map(|m| m.version),
                path,
                size,
                product: product.filter(|p| !p.is_empty()),
                buildid,
            }
        })
        .collect();
    list.sort_by(|a, b| a.path.cmp(&b.path));
    list
}

/// Best effort for files without metadata, e.g. `iPhone14,5_18.2_22C152_Restore.ipsw`.
fn parse_file_name(path: &Path) -> (Option<String>, Option<String>) {
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    let parts: Vec<&str> = name.split('_').collect();
    // shared firmware lists several products: "iPhone13,2,iPhone13,3_..."
    let product = parts
        .first()
        .filter(|p| (p.starts_with("iPhone") || p.starts_with("iPad")) && p.contains(','))
        .map(|p| p.splitn(3, ',').take(2).collect::<Vec<_>>().join(","));
    let buildid = parts
        .iter()
        .position(|p| *p == "Restore")
        .and_then(|i| i.checked_sub(1))
        .map(|i| parts[i].to_string());
    (product, buildid)
}

pub fn delete_local(path: &Path) -> Result<()> {
    fs::remove_file(path)?;
    let _ = fs::remove_file(meta_path(path));
    // idevicerestore extracts parts of the IPSW into a folder next to it
    let extracted = path.with_extension("");
    if extracted.is_dir() {
        let _ = fs::remove_dir_all(extracted);
    }
    Ok(())
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
        // Reuse an earlier download. Files verified before (metadata with the
        // same checksum and size) are not hashed again.
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let known_good =
            read_meta(&path).is_some_and(|m| m.sha1sum == fw.sha1sum && m.size == size && size > 0);
        if known_good {
            return Ok(path);
        }
        progress(Progress::Verifying);
        if verify(&path, fw)? {
            write_meta(&path, fw);
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
    write_meta(&path, fw);
    Ok(path)
}
