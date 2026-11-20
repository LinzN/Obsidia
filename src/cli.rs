//! Command line interface.

use crate::device::{self, Device, Mode};
use crate::firmware::{self, Progress};
use crate::updater;
use anyhow::{anyhow, bail, Result};
use clap::Subcommand;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[derive(Subcommand)]
pub enum Cmd {
    /// Open the graphical user interface (default)
    Gui,
    /// List connected iPhones / iPads
    List,
    /// List firmwares for a device
    Firmwares {
        /// Product type, e.g. iPhone14,5 or iPad13,1 (default: connected device)
        #[arg(short, long)]
        product: Option<String>,
        /// Include unsigned firmwares
        #[arg(short, long)]
        all: bool,
    },
    /// Download a signed IPSW into the cache
    Download {
        #[arg(short, long)]
        product: Option<String>,
        /// iOS/iPadOS version or build (default: latest signed)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Flash firmware onto the device
    Flash {
        /// iOS/iPadOS version or build (default: latest signed)
        #[arg(short, long)]
        version: Option<String>,
        /// Use a local IPSW file instead of downloading
        #[arg(short = 'f', long)]
        ipsw: Option<PathBuf>,
        /// Update and keep user data instead of erase-restore
        #[arg(long)]
        update: bool,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
        /// Verbose idevicerestore output
        #[arg(short, long)]
        debug: bool,
    },
    /// Put the device into recovery mode
    Recovery,
    /// Kick the device out of recovery mode
    ExitRecovery,
    /// Check GitHub for a newer version and install it
    Update {
        /// Only check, do not install
        #[arg(long)]
        check: bool,
    },
    /// Create a local backup via idevicebackup2
    Backup { dir: Option<PathBuf> },
}

fn check_tools() -> Result<()> {
    let missing = device::missing_tools();
    if !missing.is_empty() {
        bail!(
            "missing tools: {}\n{}",
            missing.join(", "),
            device::INSTALL_HINT
        );
    }
    Ok(())
}

fn pick_device(sel: Option<&str>) -> Result<Device> {
    let devs = device::all_devices()?;
    if devs.is_empty() {
        bail!("no device found - {}", device::no_device_hint());
    }
    if let Some(sel) = sel {
        return devs
            .into_iter()
            .find(|d| d.udid == sel || d.ecid == sel)
            .ok_or_else(|| anyhow!("device '{sel}' not found"));
    }
    if devs.len() > 1 {
        bail!("multiple devices connected - select one with --udid (UDID or ECID)");
    }
    Ok(devs.into_iter().next().unwrap())
}

fn product_or_device(product: Option<String>, sel: Option<&str>) -> Result<String> {
    match product {
        Some(p) => Ok(p),
        None => {
            check_tools()?;
            Ok(pick_device(sel)?.product)
        }
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_lowercase().as_str(),
        "y" | "yes" | "j" | "ja"
    ))
}

fn download_with_bar(fw: &firmware::Firmware) -> Result<PathBuf> {
    let pb = ProgressBar::new(fw.filesize);
    pb.set_style(
        ProgressStyle::with_template(
            "{msg}\n[{bar:40.cyan/blue}] {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}",
        )?
        .progress_chars("=> "),
    );
    pb.set_message(format!("Downloading {}", fw.file_name()));
    let cancel = AtomicBool::new(false);
    let path = firmware::download(fw, &cancel, |p| match p {
        Progress::Downloading { done, total } => {
            pb.set_length(total);
            pb.set_position(done);
        }
        Progress::Verifying => pb.set_message("Verifying SHA-1 ..."),
    })?;
    pb.finish_and_clear();
    Ok(path)
}

pub fn run(cmd: Cmd, sel: Option<&str>) -> Result<()> {
    match cmd {
        Cmd::Gui => unreachable!("handled in main"),
        Cmd::List => {
            check_tools()?;
            let devs = device::all_devices()?;
            if devs.is_empty() {
                println!("No devices found ({}).", device::no_device_hint());
            }
            for d in devs {
                println!("[{}] {} {}  {}", d.mode, d.kind(), d.product, d.name);
                if let (Some(v), Some(b)) = (&d.version, &d.build) {
                    println!("    OS:   {v} ({b})");
                }
                if !d.udid.is_empty() {
                    println!("    UDID: {}", d.udid);
                }
                if !d.ecid.is_empty() {
                    println!("    ECID: {}", d.ecid);
                }
            }
        }
        Cmd::Firmwares { product, all } => {
            let product = product_or_device(product, sel)?;
            let info = firmware::fetch_device_info(&product)?;
            println!(
                "Firmwares for {} ({product}){}:",
                info.name,
                if all { "" } else { " - signed only" }
            );
            for f in info.firmwares.iter().filter(|f| all || f.signed) {
                println!(
                    "  {}  {:<10} {:<10} {:>10}  {}",
                    if f.signed { "SIGNED  " } else { "unsigned" },
                    f.version,
                    f.buildid,
                    indicatif::HumanBytes(f.filesize).to_string(),
                    f.release_day(),
                );
            }
        }
        Cmd::Download { product, version } => {
            let product = product_or_device(product, sel)?;
            let fw = firmware::resolve_firmware(&product, version.as_deref())?;
            println!("{}", download_with_bar(&fw)?.display());
        }
        Cmd::Flash {
            version,
            ipsw,
            update,
            yes,
            debug,
        } => {
            check_tools()?;
            let dev = pick_device(sel)?;
            let (label, fw) = match &ipsw {
                Some(p) if !p.is_file() => bail!("IPSW not found: {}", p.display()),
                Some(p) => (p.file_name().unwrap().to_string_lossy().into_owned(), None),
                None => {
                    let fw = firmware::resolve_firmware(&dev.product, version.as_deref())?;
                    (format!("{} ({})", fw.version, fw.buildid), Some(fw))
                }
            };
            println!(
                "Device : {} {} [{}] {}",
                dev.kind(),
                dev.product,
                dev.mode,
                dev.name
            );
            println!("Target : {label}");
            println!(
                "Action : {}",
                if update {
                    "UPDATE (keeps data)"
                } else {
                    "RESTORE (ERASES ALL DATA)"
                }
            );
            if !update {
                println!(
                    "Warning: everything on the device will be deleted. Make sure you know the"
                );
                println!("         Apple ID password (Activation Lock) and have a backup.");
            }
            if !yes && !confirm("Continue?")? {
                println!("Aborted.");
                return Ok(());
            }
            let ipsw = match (ipsw, fw) {
                (Some(p), _) => p,
                (None, Some(fw)) => download_with_bar(&fw)?,
                _ => unreachable!(),
            };
            println!("\nRunning idevicerestore ...\n");
            let status = device::restore_cmd(&dev, &ipsw, !update, debug)?.status()?;
            if !status.success() {
                bail!("idevicerestore failed ({status}). Tip: put the device into DFU mode and retry.");
            }
            println!("\nFlash complete. The device will reboot.");
        }
        Cmd::Recovery => {
            check_tools()?;
            let dev = pick_device(sel)?;
            if dev.mode != Mode::Normal {
                println!("Device is already in {} mode.", dev.mode);
            } else {
                device::enter_recovery(&dev)?;
                println!("Device is entering recovery mode.");
            }
        }
        Cmd::ExitRecovery => {
            check_tools()?;
            device::recovery_device()?
                .ok_or_else(|| anyhow!("no device in recovery/DFU mode found"))?;
            device::exit_recovery()?;
            println!("Reboot to normal mode requested.");
        }
        Cmd::Update { check } => {
            println!("Current version: {}", updater::CURRENT_VERSION);
            let Some(update) = updater::check()? else {
                println!("Obsidia is up to date.");
                return Ok(());
            };
            println!(
                "New version available: {} ({})",
                update.version, update.page
            );
            if check {
                return Ok(());
            }
            if !confirm("Download and install it now?")? {
                return Ok(());
            }
            let pb = ProgressBar::new(0);
            pb.set_style(ProgressStyle::with_template(
                "[{bar:40.cyan/blue}] {bytes}/{total_bytes} {bytes_per_sec}",
            )?);
            let cancel = AtomicBool::new(false);
            let path = updater::download(&update, &cancel, |done, total| {
                pb.set_length(total);
                pb.set_position(done);
            })?;
            pb.finish_and_clear();
            updater::install(&path)?;
            println!("Installer started: {}", path.display());
        }
        Cmd::Backup { dir } => {
            check_tools()?;
            let dev = pick_device(sel)?;
            if dev.mode != Mode::Normal {
                bail!("a backup requires the device to be booted into iOS/iPadOS");
            }
            let dir = dir.unwrap_or_else(device::default_backup_dir);
            let status = device::backup_cmd(&dev, &dir)?.status()?;
            if !status.success() {
                bail!("backup failed ({status})");
            }
            println!("Backup stored in {}", dir.display());
        }
    }
    Ok(())
}
