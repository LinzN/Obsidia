//! Screenshots, photo export, crash reports, the live system log and the
//! "Find My" state of a device.

use crate::device::{self, Device};
use anyhow::{anyhow, bail, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};

fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// File-name friendly device name.
fn device_label(dev: &Device) -> String {
    let base = if dev.name.is_empty() || dev.name == "?" {
        dev.serial.clone().unwrap_or_else(|| dev.udid.clone())
    } else {
        dev.name.clone()
    };
    base.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn output_dir(kind: &str) -> Result<PathBuf> {
    let dir = crate::firmware::data_dir().join(kind);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Takes a screenshot. Needs the developer disk image on the device, which
/// is only available with Developer Mode on older iOS versions.
pub fn screenshot(dev: &Device) -> Result<PathBuf> {
    let _lock = device::shared();
    let path = output_dir("Screenshots")?.join(format!("{}-{}.png", device_label(dev), stamp()));
    let out = device::run(
        "idevicescreenshot",
        &["-u", &dev.udid, &path.to_string_lossy()],
    )?
    .unwrap_or_default();
    if path.is_file() {
        return Ok(path);
    }
    if out.contains("screenshotr") || out.contains("Developer disk image") {
        bail!(
            "The device does not offer screenshots. This needs Developer Mode and the developer \
             disk image, which current tools only support up to iOS 16."
        );
    }
    bail!(
        "{}",
        out.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Screenshot failed")
            .trim()
    )
}

/// New, empty folder for a photo export.
pub fn new_photo_dir(dev: &Device) -> Result<PathBuf> {
    let dir = output_dir("Photos")?.join(format!("{}-{}", device_label(dev), stamp()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Copies the camera roll (`/DCIM`) into `dir`.
pub fn export_photos(dev: &Device, dir: &Path) -> Result<()> {
    let mut cmd = device::tool_cmd("afcclient")?;
    cmd.args(["-u", &dev.udid, "get", "-r", "/DCIM"]).arg(dir);
    let _lock = device::shared();
    device::run_streaming(cmd, |_| {})?;
    Ok(())
}

/// Number of files and bytes below `dir`, for progress display.
pub fn count_files(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    entries
        .flatten()
        .fold((0, 0), |(n, size), e| match e.file_type() {
            Ok(t) if t.is_dir() => {
                let (a, b) = count_files(&e.path());
                (n + a, size + b)
            }
            Ok(_) => (n + 1, size + e.metadata().map(|m| m.len()).unwrap_or(0)),
            Err(_) => (n, size),
        })
}

/// Copies the crash reports (they stay on the device). Returns the folder
/// and the number of reports.
pub fn crash_reports(dev: &Device) -> Result<(PathBuf, usize)> {
    let dir = output_dir("CrashReports")?.join(format!("{}-{}", device_label(dev), stamp()));
    std::fs::create_dir_all(&dir)?;
    let _lock = device::shared();
    let (_, out) = device::run_full(
        "idevicecrashreport",
        &["-u", &dev.udid, "-k", "-e", &dir.to_string_lossy()],
        std::time::Duration::from_secs(600),
    )?;
    let copied = out.lines().filter(|l| l.starts_with("Copy:")).count();
    if copied == 0 {
        if let Some(err) = out.lines().find(|l| l.contains("ERROR")) {
            bail!("{}", err.trim());
        }
    }
    Ok((dir, copied))
}

/// Whether "Find My" is turned on, read from the `com.apple.fmip` lockdown
/// domain. Returns the state and the raw values of the domain (for the log).
/// A restored device with Find My on needs the owner's Apple ID to be
/// activated again (Activation Lock).
pub fn find_my_enabled(dev: &Device) -> (Option<bool>, String) {
    let raw = device::run("ideviceinfo", &["-u", &dev.udid, "-q", "com.apple.fmip"])
        .ok()
        .flatten()
        .unwrap_or_default();
    let values = device::parse_kv(&raw);
    let state = match values.get("IsAssociated").map(|v| v.trim()) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let raw = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    (state, raw)
}

/// A running `idevicesyslog`. Dropping it stops the process.
pub struct SysLog {
    child: Arc<Mutex<Option<Child>>>,
}

impl SysLog {
    pub fn start(dev: &Device, on_line: impl Fn(String) + Send + 'static) -> Result<SysLog> {
        let mut cmd = device::tool_cmd("idevicesyslog")?;
        cmd.args(["-u", &dev.udid, "--no-colors", "-x"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| device::spawn_error("idevicesyslog", e))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no output from idevicesyslog"))?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) if !l.trim().is_empty() => on_line(l),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        Ok(SysLog {
            child: Arc::new(Mutex::new(Some(child))),
        })
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SysLog {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Writes the collected log lines to `Documents/Obsidia/Logs`.
pub fn save_syslog(dev: &Device, lines: &[String]) -> Result<PathBuf> {
    let path =
        crate::firmware::logs_dir()?.join(format!("syslog-{}-{}.txt", device_label(dev), stamp()));
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path)
}
