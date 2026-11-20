//! Device detection and external tool handling (libimobiledevice).

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const REQUIRED_TOOLS: &[&str] = &["idevice_id", "ideviceinfo", "irecovery", "idevicerestore"];

#[cfg(windows)]
pub const INSTALL_HINT: &str = "\
The Windows installer already ships these tools.
Otherwise:
1. Install Apple's USB drivers: the 'Apple Devices' app (Microsoft Store) or iTunes
2. Install the libimobiledevice tools via MSYS2 (https://www.msys2.org):
     pacman -S mingw-w64-ucrt-x86_64-libimobiledevice mingw-w64-ucrt-x86_64-libirecovery mingw-w64-ucrt-x86_64-idevicerestore
3. Add C:\\msys64\\ucrt64\\bin to PATH, put the tools into a 'tools' folder next to
   obsidia.exe, or set OBSIDIA_TOOLS to their directory";
#[cfg(target_os = "macos")]
pub const INSTALL_HINT: &str = "brew install libimobiledevice libirecovery idevicerestore";
#[cfg(all(unix, not(target_os = "macos")))]
pub const INSTALL_HINT: &str = "\
Debian/Ubuntu: sudo apt install libimobiledevice-utils irecovery idevicerestore usbmuxd
Arch:          sudo pacman -S libimobiledevice libirecovery idevicerestore usbmuxd";

/// Directories searched for the libimobiledevice tools, in order of priority:
/// `$OBSIDIA_TOOLS`, `<exe dir>/tools`, `<exe dir>`, `$PATH`, and on
/// Windows the default MSYS2 install locations.
fn tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("OBSIDIA_TOOLS") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("tools"));
        dirs.push(exe_dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    // Apps started from Finder or a desktop launcher get a minimal PATH.
    if cfg!(target_os = "macos") {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    } else if cfg!(unix) {
        dirs.extend(["/usr/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    if cfg!(windows) {
        for d in [
            "C:\\msys64\\ucrt64\\bin",
            "C:\\msys64\\mingw64\\bin",
            "C:\\msys64\\clang64\\bin",
        ] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// True if `name` is an executable somewhere in PATH.
pub fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// Locate an external tool, returning its full path.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    tool_dirs()
        .into_iter()
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Names of required tools that could not be found.
pub fn missing_tools() -> Vec<&'static str> {
    REQUIRED_TOOLS
        .iter()
        .copied()
        .filter(|t| find_tool(t).is_none())
        .collect()
}

/// Build a `Command` for an external tool. On Windows the tool's directory is
/// prepended to PATH (so its DLLs are found) and no console window is opened.
pub fn tool_cmd(name: &str) -> Result<Command> {
    let path = find_tool(name)
        .ok_or_else(|| anyhow!("'{name}' not found - install it:\n{INSTALL_HINT}"))?;
    #[allow(unused_mut)]
    let mut cmd = Command::new(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        if let Some(dir) = path.parent() {
            let mut paths = vec![dir.to_path_buf()];
            if let Some(old) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&old));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
            }
        }
    }
    Ok(cmd)
}

/// Turn a process start failure into an actionable error message.
pub fn spawn_error(name: &str, e: std::io::Error) -> anyhow::Error {
    // ERROR_APPCONTROL_POLICY (4551) / ERROR_ACCESS_DISABLED_BY_POLICY (1260):
    // Windows Smart App Control or an App Control policy refused to run the
    // (unsigned) tool.
    if matches!(e.raw_os_error(), Some(4551) | Some(1260)) {
        return anyhow!(
            "Windows blocked '{name}' (Smart App Control / App Control policy). \
             The bundled libimobiledevice tools are not code-signed. Turn off Smart App Control \
             in Windows Security > App & browser control, or install the tools via MSYS2 \
             and remove the 'tools' folder next to obsidia.exe. ({e})"
        );
    }
    anyhow!("failed to execute '{name}': {e}")
}

/// Run a tool and return stdout; `Ok(None)` if it exited non-zero.
pub fn run(name: &str, args: &[&str]) -> Result<Option<String>> {
    let out = tool_cmd(name)?
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| spawn_error(name, e))?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Recovery,
    Dfu,
    Other(String),
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Normal => write!(f, "Normal"),
            Mode::Recovery => write!(f, "Recovery"),
            Mode::Dfu => write!(f, "DFU"),
            Mode::Other(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub mode: Mode,
    pub udid: String,
    pub ecid: String,
    pub product: String,
    pub name: String,
    pub version: Option<String>,
    pub build: Option<String>,
    pub serial: Option<String>,
    pub battery: Option<u8>,
    pub capacity: Option<u64>,
}

impl Device {
    pub fn kind(&self) -> &'static str {
        if self.product.starts_with("iPhone") {
            "iPhone"
        } else if self.product.starts_with("iPad") {
            "iPad"
        } else if self.product.starts_with("iPod") {
            "iPod touch"
        } else {
            "Apple device"
        }
    }

    /// Stable identifier used for selection (UDID in normal mode, ECID otherwise).
    pub fn key(&self) -> &str {
        if self.udid.is_empty() {
            &self.ecid
        } else {
            &self.udid
        }
    }
}

fn parse_kv(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn query(udid: &str, domain: Option<&str>, key: &str) -> Option<String> {
    let mut args = vec!["-u", udid, "-k", key];
    if let Some(d) = domain {
        args.extend(["-q", d]);
    }
    run("ideviceinfo", &args)
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Devices booted into iOS/iPadOS (visible via usbmuxd / Apple Mobile Device Service).
pub fn normal_devices() -> Result<Vec<Device>> {
    let list = run("idevice_id", &["-l"])?.unwrap_or_default();
    let mut devs = Vec::new();
    for udid in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let info = parse_kv(&run("ideviceinfo", &["-u", udid])?.unwrap_or_default());
        devs.push(Device {
            mode: Mode::Normal,
            udid: udid.to_string(),
            ecid: info.get("UniqueChipID").cloned().unwrap_or_default(),
            product: info
                .get("ProductType")
                .cloned()
                .unwrap_or_else(|| "?".into()),
            name: info
                .get("DeviceName")
                .cloned()
                .unwrap_or_else(|| "?".into()),
            version: info.get("ProductVersion").cloned(),
            build: info.get("BuildVersion").cloned(),
            serial: info.get("SerialNumber").cloned(),
            battery: query(
                udid,
                Some("com.apple.mobile.battery"),
                "BatteryCurrentCapacity",
            )
            .and_then(|s| s.parse().ok()),
            capacity: query(udid, Some("com.apple.disk_usage"), "TotalDiskCapacity")
                .and_then(|s| s.parse().ok()),
        });
    }
    Ok(devs)
}

/// A device in Recovery or DFU mode (irecovery handles one at a time).
pub fn recovery_device() -> Result<Option<Device>> {
    let Some(out) = run("irecovery", &["-q"])? else {
        return Ok(None);
    };
    let info = parse_kv(&out);
    let Some(mode) = info.get("MODE") else {
        return Ok(None);
    };
    let mode = match mode.to_ascii_lowercase().as_str() {
        "recovery" => Mode::Recovery,
        "dfu" => Mode::Dfu,
        other => Mode::Other(other.to_string()),
    };
    Ok(Some(Device {
        mode,
        udid: String::new(),
        ecid: info.get("ECID").cloned().unwrap_or_default(),
        product: info.get("PRODUCT").cloned().unwrap_or_else(|| "?".into()),
        name: info.get("NAME").cloned().unwrap_or_else(|| "-".into()),
        version: None,
        build: None,
        serial: info.get("SRNM").cloned(),
        battery: None,
        capacity: None,
    }))
}

pub fn all_devices() -> Result<Vec<Device>> {
    let mut devs = normal_devices()?;
    devs.extend(recovery_device()?);
    Ok(devs)
}

pub fn no_device_hint() -> &'static str {
    if cfg!(windows) {
        "check the cable, the 'Trust this computer' prompt and that the Apple Devices app / iTunes is installed"
    } else if cfg!(target_os = "macos") {
        "check the cable and the 'Trust this computer' prompt"
    } else {
        "check the cable, the 'Trust this computer' prompt and that usbmuxd is running"
    }
}

pub fn enter_recovery(dev: &Device) -> Result<()> {
    run("ideviceenterrecovery", &[&dev.udid])?
        .ok_or_else(|| anyhow!("ideviceenterrecovery failed"))?;
    Ok(())
}

pub fn exit_recovery() -> Result<()> {
    run("irecovery", &["-n"])?.ok_or_else(|| anyhow!("irecovery -n failed"))?;
    Ok(())
}

/// Default backup directory for a device.
pub fn default_backup_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Obsidia Backups")
}

/// Command that creates a backup of `dev` into `dir`.
pub fn backup_cmd(dev: &Device, dir: &Path) -> Result<Command> {
    std::fs::create_dir_all(dir)?;
    let mut cmd = tool_cmd("idevicebackup2")?;
    cmd.args(["-u", &dev.udid, "backup"]).arg(dir);
    Ok(cmd)
}

/// Command that flashes `ipsw` onto `dev`. `erase` = full restore, else update.
pub fn restore_cmd(dev: &Device, ipsw: &Path, erase: bool, debug: bool) -> Result<Command> {
    let mut cmd = tool_cmd("idevicerestore")?;
    if erase {
        cmd.arg("-e");
    }
    if !dev.ecid.is_empty() {
        cmd.args(["-i", &dev.ecid]);
    } else if !dev.udid.is_empty() {
        cmd.args(["-u", &dev.udid]);
    }
    if debug {
        cmd.arg("-d");
    }
    cmd.arg(ipsw);
    Ok(cmd)
}

/// Run a command, streaming every output line (stdout + stderr) to `on_line`.
/// Lines are split on `\n` and `\r` so that progress updates arrive live.
pub fn run_streaming(
    mut cmd: Command,
    on_line: impl Fn(String) + Send + Sync + 'static,
) -> Result<()> {
    use std::io::Read;
    use std::sync::Arc;

    let program = Path::new(cmd.get_program())
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(&program, e))?;

    let on_line = Arc::new(on_line);
    let mut readers = Vec::new();
    let streams: Vec<Box<dyn Read + Send>> = vec![
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    for mut stream in streams {
        let cb = Arc::clone(&on_line);
        readers.push(std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut line = Vec::new();
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            cb(String::from_utf8_lossy(&line).into_owned());
                            line.clear();
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
            if !line.is_empty() {
                cb(String::from_utf8_lossy(&line).into_owned());
            }
        }));
    }
    let status = child.wait()?;
    for r in readers {
        let _ = r.join();
    }
    if !status.success() {
        anyhow::bail!("process exited with {status}");
    }
    Ok(())
}
