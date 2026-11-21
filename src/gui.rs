//! Desktop GUI built with egui.

use crate::device::{self, Device, Mode};
use crate::drivers;
use crate::firmware::{self, CatalogEntry, DeviceInfo, Firmware, Progress};
use crate::guides::Family;
use crate::icons;
use crate::settings::Settings;
use crate::theme::{self, Surface, ACCENT, DANGER, GREEN, ORANGE, RED};
use crate::updater::{self, Update};
use eframe::egui::{self, Color32, RichText, Stroke};
use egui_extras::{Column, TableBuilder};
use indicatif::HumanBytes;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCAN_INTERVAL: Duration = Duration::from_secs(3);
const LOG_LIMIT: usize = 5000;
const MIRRANET_URL: &str = "https://mirranet.de";
const GITHUB_URL: &str = "https://github.com/LinzN/obsidia";
const ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

pub fn run() -> anyhow::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Obsidia")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1040.0, 640.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Obsidia",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
    .map_err(|e| anyhow::anyhow!("failed to start GUI: {e}"))
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG).ok()?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    Some(ctx.load_texture("obsidia-logo", image, egui::TextureOptions::LINEAR))
}

enum Msg {
    Devices(Result<Vec<Device>, String>),
    Catalog(Result<Vec<CatalogEntry>, String>),
    Firmwares(String, Result<DeviceInfo, String>),
    DlProgress(String, Progress),
    DlDone(String, Result<PathBuf, String>),
    Log(String),
    JobStatus(String),
    JobDone(Result<String, String>),
    UpdateChecked(Result<Option<Update>, String>, bool),
    UpdateProgress(u64, u64),
    UpdateDownloaded(Result<PathBuf, String>),
    DriverStatus(drivers::Status),
}

/// Sender that also wakes up the UI.
#[derive(Clone)]
struct Out {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Out {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

struct Download {
    progress: Progress,
    cancel: Arc<AtomicBool>,
}

#[derive(PartialEq, Clone, Copy)]
enum JobKind {
    Flash,
    Other,
}

struct Job {
    kind: JobKind,
    title: String,
    status: String,
    percent: Option<f32>,
    started: Instant,
}

#[derive(Clone)]
enum FlashSource {
    Firmware(Firmware),
    Local(PathBuf),
}

struct FlashRequest {
    device: Device,
    source: FlashSource,
    label: String,
    erase: bool,
}

/// Full-size progress / result view shown in place of the firmware list.
struct FlashView {
    label: String,
    result: Option<Result<String, String>>,
}

enum UpdateState {
    Checking,
    UpToDate,
    Available(Update),
    Downloading {
        update: Update,
        done: u64,
        total: u64,
        cancel: Arc<AtomicBool>,
    },
    Failed(Option<Update>, String),
}

/// Window for installing the classic Apple drivers (Windows).
struct DriverPanel {
    status: Option<drivers::Status>,
    remove_store: bool,
}

struct DfuAssistant {
    family: Family,
    state: DfuState,
}

enum DfuState {
    Ready,
    Countdown(Instant),
    Running {
        phase: usize,
        since: Instant,
    },
    /// All steps done, waiting a moment for the device to show up.
    Waiting(Instant),
    Success,
    Failed(String),
}

impl DfuAssistant {
    fn active(&self) -> bool {
        matches!(
            self.state,
            DfuState::Countdown(_) | DfuState::Running { .. } | DfuState::Waiting(_)
        )
    }
}

const DFU_COUNTDOWN: f32 = 3.0;
const DFU_GRACE: f32 = 5.0;

#[derive(PartialEq, Clone, Copy)]
enum LocalStatus {
    Signed,
    Expired,
    Checking,
    Unknown,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Device,
    Flash,
    Toolbox,
}

#[derive(PartialEq, Clone, Copy)]
enum FlashPage {
    SmartFlash,
    FirmwareDownload,
    LocalFirmware,
    Guide,
}

/// What the Flash button will install.
#[derive(Clone, PartialEq)]
enum Selection {
    Firmware(String), // firmware URL
    Local(PathBuf),
}

struct App {
    logo: Option<egui::TextureHandle>,
    out: Out,
    rx: Receiver<Msg>,

    tab: Tab,
    page: FlashPage,

    missing_tools: Vec<&'static str>,
    devices: Vec<Device>,
    selected_device: Option<String>,
    scanning: bool,
    last_scan: Option<Instant>,
    scan_error: Option<String>,

    catalog: Option<Load<Vec<CatalogEntry>>>,
    browse: Option<CatalogEntry>,
    search: String,
    firmwares: HashMap<String, Load<DeviceInfo>>,
    show_unsigned: bool,
    downloads: HashMap<String, Download>,
    local_files: Vec<firmware::LocalFirmware>,
    clean_pending: bool,

    selection: Option<Selection>,
    erase: bool,

    job: Option<Job>,
    flash_view: Option<FlashView>,
    log: Vec<String>,
    show_log: bool,
    pending_flash: Option<FlashRequest>,
    notice: Option<(String, bool)>,

    update: Option<UpdateState>,
    update_dismissed: bool,
    show_release_notes: bool,

    settings: Settings,
    show_settings: bool,
    show_about: bool,
    guide_family: Option<Family>,
    dfu: Option<DfuAssistant>,
    drivers: Option<DriverPanel>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        let settings = Settings::load();
        firmware::set_firmware_dir(settings.firmware_dir.clone());
        let (tx, rx) = channel();
        let mut app = Self {
            logo: load_logo(ctx),
            out: Out {
                tx,
                ctx: ctx.clone(),
            },
            rx,
            tab: Tab::Flash,
            page: FlashPage::SmartFlash,
            missing_tools: device::missing_tools(),
            devices: Vec::new(),
            selected_device: None,
            scanning: false,
            last_scan: None,
            scan_error: None,
            catalog: None,
            browse: None,
            search: String::new(),
            firmwares: HashMap::new(),
            show_unsigned: false,
            downloads: HashMap::new(),
            local_files: Vec::new(),
            clean_pending: false,
            selection: None,
            erase: true,
            job: None,
            flash_view: None,
            log: vec![format!("Obsidia {} started", env!("CARGO_PKG_VERSION"))],
            show_log: false,
            pending_flash: None,
            notice: None,
            update: None,
            update_dismissed: false,
            show_release_notes: false,
            settings,
            show_settings: false,
            show_about: false,
            guide_family: None,
            dfu: None,
            drivers: None,
        };
        if updater::auto_check_enabled() && app.settings.check_updates {
            app.check_for_updates(false);
        }
        app.refresh_local_files();
        app
    }

    fn spawn(&self, f: impl FnOnce(Out) + Send + 'static) {
        let out = self.out.clone();
        std::thread::spawn(move || f(out));
    }

    fn log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }

    fn device(&self) -> Option<&Device> {
        let key = self.selected_device.as_deref()?;
        self.devices.iter().find(|d| d.key() == key)
    }

    fn device_product(&self) -> Option<String> {
        self.device()
            .map(|d| d.product.clone())
            .filter(|p| p != "?")
    }

    fn model_name(&self, product: &str) -> Option<&str> {
        match self.firmwares.get(product) {
            Some(Load::Ready(info)) => Some(&info.name),
            _ => None,
        }
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn refresh_local_files(&mut self) {
        self.local_files = firmware::local_firmware();
        // Look up the signing status of every downloaded firmware.
        let products: Vec<String> = self
            .local_files
            .iter()
            .filter_map(|f| f.product.clone())
            .collect();
        for p in products {
            self.ensure_firmwares(&p);
        }
        self.clean_pending = true;
    }

    fn local_status(&self, file: &firmware::LocalFirmware) -> LocalStatus {
        let (Some(product), Some(build)) = (&file.product, &file.buildid) else {
            return LocalStatus::Unknown;
        };
        match self.firmwares.get(product) {
            Some(Load::Ready(info)) => match info.firmwares.iter().find(|f| &f.buildid == build) {
                Some(fw) if fw.signed => LocalStatus::Signed,
                Some(_) => LocalStatus::Expired,
                None => LocalStatus::Unknown,
            },
            Some(Load::Loading) | None => LocalStatus::Checking,
            Some(Load::Failed(_)) => LocalStatus::Unknown,
        }
    }

    /// Delete downloaded firmware that Apple no longer signs. Files that are
    /// selected for flashing or in use by a running job are kept.
    fn remove_expired(&mut self, reason: &str) -> usize {
        if self.busy() {
            return 0;
        }
        let expired: Vec<PathBuf> = self
            .local_files
            .iter()
            .filter(|f| self.local_status(f) == LocalStatus::Expired)
            .filter(|f| self.selection != Some(Selection::Local(f.path.clone())))
            .map(|f| f.path.clone())
            .collect();
        let mut removed = 0;
        for path in expired {
            match firmware::delete_local(&path) {
                Ok(()) => {
                    removed += 1;
                    self.log(format!(
                        "{reason}: removed {} (no longer signed)",
                        path.display()
                    ));
                }
                Err(e) => self.log(format!("Could not delete {}: {e}", path.display())),
            }
        }
        if removed > 0 {
            self.local_files = firmware::local_firmware();
        }
        removed
    }

    fn auto_clean(&mut self) {
        if !self.clean_pending || !self.settings.auto_clean || self.busy() {
            return;
        }
        // wait until the signing status of every file is known
        if self
            .local_files
            .iter()
            .any(|f| self.local_status(f) == LocalStatus::Checking)
        {
            return;
        }
        self.clean_pending = false;
        self.remove_expired("Auto-clean");
    }

    fn process_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Devices(res) => {
                    self.scanning = false;
                    self.last_scan = Some(Instant::now());
                    match res {
                        Ok(devs) => {
                            self.scan_error = None;
                            for d in &devs {
                                if !self.devices.iter().any(|o| o.key() == d.key()) {
                                    self.log(format!(
                                        "Connected: {} {} [{}]",
                                        d.kind(),
                                        d.product,
                                        d.mode
                                    ));
                                }
                            }
                            let gone: Vec<String> = self
                                .devices
                                .iter()
                                .filter(|o| !devs.iter().any(|d| d.key() == o.key()))
                                .map(|o| format!("Disconnected: {} {}", o.kind(), o.product))
                                .collect();
                            for g in gone {
                                self.log(g);
                            }
                            self.devices = devs;
                            if self.device().is_none() {
                                self.selected_device =
                                    self.devices.first().map(|d| d.key().to_string());
                            }
                        }
                        Err(e) => self.scan_error = Some(e),
                    }
                }
                Msg::Catalog(res) => {
                    self.catalog = Some(match res {
                        Ok(c) => Load::Ready(c),
                        Err(e) => Load::Failed(e),
                    })
                }
                Msg::Firmwares(product, res) => {
                    let state = match res {
                        Ok(info) => Load::Ready(info),
                        Err(e) => {
                            self.log(format!("Firmware list for {product} failed: {e}"));
                            Load::Failed(e)
                        }
                    };
                    self.firmwares.insert(product, state);
                    self.clean_pending = true;
                }
                Msg::DlProgress(url, p) => {
                    if let Some(d) = self.downloads.get_mut(&url) {
                        d.progress = p;
                    }
                }
                Msg::DlDone(url, res) => {
                    self.downloads.remove(&url);
                    match res {
                        Ok(path) => self.log(format!("Download complete: {}", path.display())),
                        Err(e) => self.log(format!("Download failed: {e}")),
                    }
                    self.refresh_local_files();
                }
                Msg::Log(line) => {
                    if let Some(job) = &mut self.job {
                        if let Some(p) = parse_percent(&line) {
                            job.percent = Some(p);
                        }
                        job.status = line.clone();
                    }
                    self.log(line);
                }
                Msg::JobStatus(s) => {
                    if let Some(job) = &mut self.job {
                        job.status = s;
                        job.percent = None;
                    }
                }
                Msg::UpdateChecked(res, manual) => {
                    self.update = Some(match res {
                        Ok(Some(update)) => {
                            self.log(format!("Update available: Obsidia {}", update.version));
                            self.update_dismissed = false;
                            UpdateState::Available(update)
                        }
                        Ok(None) => {
                            if manual {
                                self.notice = Some(("Obsidia is up to date.".into(), false));
                            }
                            UpdateState::UpToDate
                        }
                        Err(e) => {
                            self.log(format!("Update check failed: {e}"));
                            if manual {
                                self.notice = Some((format!("Update check failed: {e}"), true));
                            }
                            UpdateState::Failed(None, e)
                        }
                    });
                }
                Msg::DriverStatus(status) => {
                    if let Some(panel) = &mut self.drivers {
                        panel.remove_store = status.store_app;
                        panel.status = Some(status);
                    }
                }
                Msg::UpdateProgress(d, t) => {
                    if let Some(UpdateState::Downloading { done, total, .. }) = &mut self.update {
                        *done = d;
                        *total = t;
                    }
                }
                Msg::UpdateDownloaded(res) => {
                    let update = match self.update.take() {
                        Some(UpdateState::Downloading { update, .. }) => Some(update),
                        _ => None,
                    };
                    match res {
                        Ok(path) => match updater::install(&path) {
                            Ok(()) => {
                                self.log(format!("Starting installer {}", path.display()));
                                self.out.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            Err(e) => {
                                self.log(format!("Update failed: {e:#}"));
                                self.update = Some(UpdateState::Failed(update, format!("{e:#}")));
                            }
                        },
                        Err(e) => {
                            self.log(format!("Update download failed: {e}"));
                            self.update = Some(UpdateState::Failed(update, e));
                        }
                    }
                }
                Msg::JobDone(res) => {
                    let job = self.job.take();
                    let title = job.as_ref().map(|j| j.title.clone()).unwrap_or_default();
                    match &res {
                        Ok(msg) => {
                            self.log(format!("{title}: {msg}"));
                            self.notice = Some((msg.clone(), false));
                        }
                        Err(e) => {
                            self.log(format!("{title} failed: {e}"));
                            self.notice = Some((format!("{title} failed: {e}"), true));
                        }
                    }
                    if job.is_some_and(|j| j.kind == JobKind::Flash) {
                        if let Some(view) = &mut self.flash_view {
                            view.result = Some(res);
                        }
                    }
                    self.refresh_local_files();
                    self.last_scan = None; // rescan right away
                }
            }
        }
    }

    fn maybe_scan(&mut self) {
        // Never poll the device while a job (restore / backup) is running.
        if self.scanning || self.busy() || !self.missing_tools.is_empty() {
            return;
        }
        // poll every second while the DFU assistant is waiting for the device
        let interval = if self.dfu.as_ref().is_some_and(|d| d.active()) {
            Duration::from_secs(1)
        } else {
            SCAN_INTERVAL
        };
        if self.last_scan.is_some_and(|t| t.elapsed() < interval) {
            return;
        }
        self.scanning = true;
        self.spawn(|out| {
            out.send(Msg::Devices(
                device::all_devices().map_err(|e| format!("{e:#}")),
            ))
        });
    }

    fn ensure_firmwares(&mut self, product: &str) {
        if self.firmwares.contains_key(product) {
            return;
        }
        self.firmwares.insert(product.to_string(), Load::Loading);
        let product = product.to_string();
        self.spawn(move |out| {
            let res = firmware::fetch_device_info(&product).map_err(|e| format!("{e:#}"));
            out.send(Msg::Firmwares(product, res));
        });
    }

    fn ensure_catalog(&mut self) {
        if self.catalog.is_none() {
            self.catalog = Some(Load::Loading);
            self.spawn(|out| {
                out.send(Msg::Catalog(
                    firmware::fetch_catalog().map_err(|e| format!("{e:#}")),
                ))
            });
        }
    }

    fn start_download(&mut self, fw: &Firmware) {
        if self.downloads.contains_key(&fw.url) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.downloads.insert(
            fw.url.clone(),
            Download {
                progress: Progress::Downloading {
                    done: 0,
                    total: fw.filesize,
                },
                cancel: cancel.clone(),
            },
        );
        self.log(format!("Downloading {} ...", fw.file_name()));
        let fw = fw.clone();
        self.spawn(move |out| {
            let mut last = Instant::now() - Duration::from_secs(1);
            let res = firmware::download(&fw, &cancel, |p| {
                // Throttle UI updates to ~10 per second.
                if matches!(p, Progress::Verifying) || last.elapsed() > Duration::from_millis(100) {
                    last = Instant::now();
                    out.send(Msg::DlProgress(fw.url.clone(), p));
                }
            });
            out.send(Msg::DlDone(
                fw.url.clone(),
                res.map_err(|e| format!("{e:#}")),
            ));
        });
    }

    fn start_job(
        &mut self,
        kind: JobKind,
        title: &str,
        work: impl FnOnce(&Out) -> Result<String, String> + Send + 'static,
    ) {
        self.job = Some(Job {
            kind,
            title: title.to_string(),
            status: "Starting ...".into(),
            percent: None,
            started: Instant::now(),
        });
        self.notice = None;
        self.log(format!("--- {title} ---"));
        self.spawn(move |out| {
            let res = work(&out);
            out.send(Msg::JobDone(res));
        });
    }

    fn start_flash(&mut self, req: FlashRequest) {
        let FlashRequest {
            device: dev,
            source,
            label,
            erase,
        } = req;
        let title = if erase {
            format!("Restore to {label}")
        } else {
            format!("Update to {label}")
        };
        self.flash_view = Some(FlashView {
            label: label.clone(),
            result: None,
        });
        let auto_retry = self.settings.auto_retry;
        self.tab = Tab::Flash;
        self.page = FlashPage::SmartFlash;
        self.start_job(JobKind::Flash, &title, move |out| {
            let ipsw = match source {
                FlashSource::Local(p) => p,
                FlashSource::Firmware(fw) => {
                    out.send(Msg::JobStatus(format!(
                        "Downloading {} ...",
                        fw.file_name()
                    )));
                    let cancel = AtomicBool::new(false);
                    let mut last = Instant::now() - Duration::from_secs(1);
                    firmware::download(&fw, &cancel, |p| {
                        if last.elapsed() > Duration::from_millis(250) {
                            last = Instant::now();
                            let line = match p {
                                Progress::Downloading { done, total } if total > 0 => format!(
                                    "Downloading firmware ... {:.1}%",
                                    done as f64 * 100.0 / total as f64
                                ),
                                Progress::Downloading { .. } => {
                                    "Downloading firmware ...".to_string()
                                }
                                Progress::Verifying => "Verifying firmware ...".to_string(),
                            };
                            // The progress bar reads the percentage from the status line.
                            out.send(Msg::JobStatus(line));
                        }
                    })
                    .map_err(|e| format!("{e:#}"))?
                }
            };
            // Keep the computer awake until the restore is finished.
            let _awake = device::KeepAwake::new();
            preflight(&ipsw).map_err(|e| e.to_string())?;

            let attempts = if auto_retry { MAX_RESTORE_ATTEMPTS } else { 1 };
            let mut dev = dev;
            let mut attempt = 1;
            loop {
                if attempt > 1 {
                    out.send(Msg::Log(format!("--- Attempt {attempt} of {attempts} ---")));
                }
                out.send(Msg::JobStatus(if attempt > 1 {
                    format!("Preparing device (attempt {attempt} of {attempts}) ...")
                } else {
                    "Preparing device ...".into()
                }));
                let cmd = device::restore_cmd(&dev, &ipsw, erase, false)
                    .map_err(|e| format!("{e:#}"))?;
                let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
                let (line_out, collected) = (out.clone(), lines.clone());
                // no device queries while idevicerestore talks to the device
                let lock = device::exclusive();
                let result = device::run_streaming(cmd, move |l| {
                    if let Ok(mut v) = collected.lock() {
                        v.push(l.clone());
                    }
                    line_out.send(Msg::Log(l));
                });
                drop(lock);
                let lines = lines.lock().map(|v| v.clone()).unwrap_or_default();
                if let Some(path) = write_restore_log(&lines) {
                    out.send(Msg::Log(format!("Restore log saved to {}", path.display())));
                }

                let Err(e) = result else {
                    return Ok(
                        "Flashing completed successfully - the device will now reboot.".to_string(),
                    );
                };
                let detail = match last_error_line(&lines) {
                    Some(line) => format!("{line} ({e:#})"),
                    None => format!("{e:#}"),
                };
                let upload_failed = lines.iter().any(|l| l.contains("Unable to upload data"));
                if attempt >= attempts || !retryable(&lines) || dev.ecid.is_empty() {
                    let mut msg = if attempt > 1 {
                        format!("{detail} - failed after {attempt} attempts")
                    } else {
                        detail
                    };
                    if upload_failed {
                        msg.push_str(
                            ". The device does not accept data over USB. Close the Apple Devices app / iTunes, \
                             force restart the device into recovery or DFU mode, use a USB port directly on \
                             the computer and try again.",
                        );
                        if drivers::supported() {
                            msg.push_str(
                                " Newer iPhones often need the classic iTunes drivers for this: \
                                 Toolbox \u{2192} Apple Drivers installs them.",
                            );
                        }
                    }
                    return Err(msg);
                }
                if upload_failed {
                    // A USB reset often clears a stuck recovery-mode connection.
                    out.send(Msg::Log("Upload failed - resetting the USB connection".into()));
                    let _lock = device::exclusive();
                    device::reset_recovery_usb();
                    std::thread::sleep(Duration::from_secs(3));
                }

                // Typical failure: the connection to the device is lost when it
                // switches into restore mode and it boots back. Wait until it is
                // back and start over.
                out.send(Msg::Log(format!(
                    "Attempt {attempt} failed: {detail}. Waiting for the device to reconnect ..."
                )));
                out.send(Msg::JobStatus("Waiting for the device to reconnect ...".into()));
                match device::wait_for_device(&dev.ecid, Duration::from_secs(180)) {
                    Some(found) => {
                        out.send(Msg::Log(format!(
                            "Device is back in {} mode, retrying in a few seconds",
                            found.mode
                        )));
                        dev = found;
                        // give the device time to settle after reconnecting
                        std::thread::sleep(Duration::from_secs(5));
                    }
                    None => {
                        return Err(format!(
                            "{detail} - the device did not reconnect. Put it into recovery or DFU mode and try again."
                        ))
                    }
                }
                attempt += 1;
            }
        });
    }

    fn check_for_updates(&mut self, manual: bool) {
        if matches!(
            self.update,
            Some(UpdateState::Checking | UpdateState::Downloading { .. })
        ) {
            return;
        }
        self.update = Some(UpdateState::Checking);
        self.spawn(move |out| {
            let res = updater::check().map_err(|e| format!("{e:#}"));
            out.send(Msg::UpdateChecked(res, manual));
        });
    }

    fn start_update(&mut self, update: Update) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.log(format!("Downloading Obsidia {} ...", update.version));
        self.update = Some(UpdateState::Downloading {
            update: update.clone(),
            done: 0,
            total: update.asset.as_ref().map(|a| a.size).unwrap_or(0),
            cancel: cancel.clone(),
        });
        self.spawn(move |out| {
            let mut last = Instant::now() - Duration::from_secs(1);
            let res = updater::download(&update, &cancel, |done, total| {
                if last.elapsed() > Duration::from_millis(100) {
                    last = Instant::now();
                    out.send(Msg::UpdateProgress(done, total));
                }
            });
            out.send(Msg::UpdateDownloaded(res.map_err(|e| format!("{e:#}"))));
        });
    }

    /// Fluent-style info bar shown below the header when an update is available.
    fn update_bar(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<&str> = None;
        let busy = self.busy();
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x1f, 0x33, 0x40))
            .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.35)))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(14, 8));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| match &self.update {
                Some(UpdateState::Available(update)) => {
                    ui.label(RichText::new("\u{e2ce}").color(ACCENT).size(16.0));
                    ui.label(RichText::new("Update available").strong());
                    ui.label(format!(
                        "Obsidia {} is available (you have {}).",
                        update.version,
                        updater::CURRENT_VERSION
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("\u{e4f6}")
                            .on_hover_text("Dismiss")
                            .clicked()
                        {
                            action = Some("dismiss");
                        }
                        let can_install = update.asset.is_some() && !busy && updater::can_install();
                        let resp = ui.add_enabled(
                            can_install,
                            theme::accent_button("Install update", ACCENT, can_install),
                        );
                        let resp = if !updater::can_install() {
                            resp.on_disabled_hover_text(
                                "Installing updates needs administrator rights - ask your IT administrator",
                            )
                        } else if busy {
                            resp.on_disabled_hover_text(
                                "Wait until the current operation has finished",
                            )
                        } else {
                            resp.on_disabled_hover_text(
                                "No installer for this platform in the release",
                            )
                        };
                        if resp.clicked() {
                            action = Some("install");
                        }
                        if ui.button("Release notes").clicked() {
                            action = Some("notes");
                        }
                    });
                }
                Some(UpdateState::Downloading {
                    update,
                    done,
                    total,
                    ..
                }) => {
                    ui.spinner();
                    ui.label(format!("Downloading Obsidia {} ...", update.version));
                    let frac = if *total > 0 {
                        *done as f32 / *total as f32
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(240.0)
                            .fill(ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            action = Some("cancel");
                        }
                    });
                }
                Some(UpdateState::Failed(_, e)) => {
                    ui.label(RichText::new("\u{e4e0}").color(ORANGE));
                    ui.label(RichText::new(format!("Update failed: {e}")).color(RED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("\u{e4f6}").clicked() {
                            action = Some("dismiss");
                        }
                        if ui.button("Open release page").clicked() {
                            action = Some("page");
                        }
                    });
                }
                _ => {}
            });
        });

        match action {
            Some("install") => {
                if let Some(UpdateState::Available(update)) = &self.update {
                    let update = update.clone();
                    self.start_update(update);
                }
            }
            Some("cancel") => {
                if let Some(UpdateState::Downloading { cancel, .. }) = &self.update {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Some("notes") => self.show_release_notes = true,
            Some("page") => {
                let url = match &self.update {
                    Some(UpdateState::Failed(Some(u), _)) => u.page.clone(),
                    _ => format!("{GITHUB_URL}/releases"),
                };
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            Some("dismiss") => self.update_dismissed = true,
            _ => {}
        }
    }

    fn show_update_bar(&self) -> bool {
        !self.update_dismissed
            && matches!(
                self.update,
                Some(UpdateState::Available(_) | UpdateState::Downloading { .. })
                    | Some(UpdateState::Failed(Some(_), _))
            )
    }

    fn release_notes_window(&mut self, ctx: &egui::Context) {
        let Some(UpdateState::Available(update)) = &self.update else {
            self.show_release_notes = false;
            return;
        };
        let mut open = self.show_release_notes;
        let (title, notes, page) = (
            format!("What's new in Obsidia {}", update.version),
            update.notes.clone(),
            update.page.clone(),
        );
        egui::Window::new(title)
            .open(&mut open)
            .default_size([560.0, 380.0])
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if notes.trim().is_empty() {
                            ui.label(RichText::new("No release notes.").weak());
                        } else {
                            ui.label(&notes);
                        }
                    });
                ui.add_space(8.0);
                if ui.button("Open release page").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(page.clone()));
                }
            });
        self.show_release_notes = open;
    }

    fn start_backup(&mut self, dev: Device) {
        self.start_job(JobKind::Other, "Backup", move |out| {
            let dir = device::default_backup_dir();
            let cmd = device::backup_cmd(&dev, &dir).map_err(|e| format!("{e:#}"))?;
            let _lock = device::exclusive();
            let o = out.clone();
            device::run_streaming(cmd, move |l| o.send(Msg::Log(l)))
                .map_err(|e| format!("{e:#}"))?;
            Ok(format!("Backup stored in {}", dir.display()))
        });
    }

    fn start_enter_recovery(&mut self, dev: Device) {
        self.start_job(JobKind::Other, "Enter recovery", move |_| {
            device::enter_recovery(&dev).map_err(|e| format!("{e:#}"))?;
            Ok("Device is entering recovery mode.".into())
        });
    }

    fn start_exit_recovery(&mut self) {
        self.start_job(JobKind::Other, "Exit recovery", |_| {
            device::exit_recovery().map_err(|e| format!("{e:#}"))?;
            Ok("Reboot to normal mode requested.".into())
        });
    }

    /// Prepare the confirmation dialog for the current selection.
    fn request_flash(&mut self) {
        let Some(dev) = self.device().cloned() else {
            return;
        };
        let req = match self.selection.clone() {
            Some(Selection::Local(path)) => FlashRequest {
                label: file_name(&path),
                source: FlashSource::Local(path),
                device: dev,
                erase: self.erase,
            },
            Some(Selection::Firmware(url)) => {
                let Some(fw) = self.find_firmware(&dev.product, &url) else {
                    return;
                };
                FlashRequest {
                    label: format!("{} ({})", fw.version, fw.buildid),
                    source: FlashSource::Firmware(fw),
                    device: dev,
                    erase: self.erase,
                }
            }
            None => return,
        };
        self.pending_flash = Some(req);
    }

    fn find_firmware(&self, product: &str, url: &str) -> Option<Firmware> {
        match self.firmwares.get(product) {
            Some(Load::Ready(info)) => info.firmwares.iter().find(|f| f.url == url).cloned(),
            _ => None,
        }
    }

    /// Why the Flash button is disabled, or `None` if flashing is possible.
    fn flash_blocker(&self) -> Option<&'static str> {
        if !self.missing_tools.is_empty() {
            return Some("Required tools are missing");
        }
        if self.busy() {
            return Some("Another operation is running");
        }
        let Some(dev) = self.device() else {
            return Some("Connect a device first");
        };
        match &self.selection {
            None => Some("Select a firmware first"),
            Some(Selection::Local(_)) => None,
            Some(Selection::Firmware(url)) => match self.find_firmware(&dev.product, url) {
                Some(fw) if fw.signed => None,
                Some(_) => Some("Apple no longer signs this version"),
                None => Some("Select a firmware for the connected device"),
            },
        }
    }

    fn import_local_firmware(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import local firmware")
            .add_filter("iOS / iPadOS firmware", &["ipsw"])
            .pick_file();
        if let Some(path) = picked {
            self.log(format!("Selected local firmware: {}", path.display()));
            self.selection = Some(Selection::Local(path));
            self.page = FlashPage::SmartFlash;
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(32.0, 32.0)));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(theme::title("Obsidia", 20.0));
                link(ui, "by MirraNET", MIRRANET_URL);
            });
            ui.add_space(28.0);

            for (tab, label) in [
                (Tab::Device, "Device"),
                (Tab::Flash, "Flash"),
                (Tab::Toolbox, "Toolbox"),
            ] {
                if tab_button(ui, label, self.tab == tab).clicked() {
                    self.tab = tab;
                }
                ui.add_space(4.0);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if header_button(ui, icons::INFO, "About Obsidia").clicked() {
                    self.show_about = !self.show_about;
                }
                if header_button(ui, icons::GEAR, "Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                if header_button(ui, "\u{e2f2}", "Log").clicked() {
                    self.show_log = !self.show_log;
                }
                let active = self.downloads.len();
                let dl = if active > 0 {
                    format!("\u{e20c} {active}")
                } else {
                    "\u{e20c}".to_string()
                };
                let resp = ui
                    .add(
                        egui::Button::new(RichText::new(dl).size(16.0))
                            .min_size(egui::vec2(34.0, 32.0)),
                    )
                    .on_hover_text(if active > 0 {
                        format!("{active} download(s) running - open firmware folder")
                    } else {
                        "Open the firmware folder".to_string()
                    });
                if resp.clicked() {
                    if let Ok(dir) = firmware::firmware_dir() {
                        open_path(&dir);
                    }
                }
                if active > 0 {
                    ui.spinner();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.missing_tools.is_empty() {
                ui.label(RichText::new("● Tools ready").color(GREEN).small());
            } else {
                ui.label(RichText::new("● Tools missing").color(ORANGE).small());
            }
            ui.separator();
            let n = self.devices.len();
            ui.label(
                RichText::new(match n {
                    0 => "No device".to_string(),
                    1 => "1 device connected".to_string(),
                    n => format!("{n} devices connected"),
                })
                .small(),
            );
            if self.scanning {
                ui.spinner();
            }
            if let Some(job) = &self.job {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} - {}s",
                        job.title,
                        job.started.elapsed().as_secs()
                    ))
                    .small(),
                );
            } else if let Some((msg, err)) = &self.notice {
                ui.separator();
                ui.label(
                    RichText::new(msg)
                        .small()
                        .color(if *err { RED } else { GREEN }),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("View log").clicked() {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    /// Version and update state shown under the project card.
    fn version_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Version {}", updater::CURRENT_VERSION)).small());
            ui.label(RichText::new("·").small().weak());
            match &self.update {
                Some(UpdateState::Checking) => {
                    ui.label(RichText::new("Checking for updates ...").small().weak());
                }
                Some(UpdateState::UpToDate) => {
                    ui.label(
                        RichText::new(format!("{} Up to date", "\u{e182}"))
                            .small()
                            .color(GREEN),
                    );
                }
                Some(UpdateState::Available(u))
                | Some(UpdateState::Downloading { update: u, .. }) => {
                    let text = RichText::new(format!("Update {} available", u.version))
                        .small()
                        .color(ACCENT);
                    if ui
                        .add(egui::Label::new(text).sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Show update")
                        .clicked()
                    {
                        self.update_dismissed = false;
                    }
                }
                Some(UpdateState::Failed(..)) => {
                    if ui
                        .add(
                            egui::Label::new(
                                RichText::new("Update check failed").small().color(ORANGE),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Click to try again")
                        .clicked()
                    {
                        self.check_for_updates(true);
                    }
                }
                None => {
                    let text = if updater::auto_check_enabled() {
                        "Check for updates"
                    } else {
                        "Updates managed by IT"
                    };
                    if ui
                        .add(
                            egui::Label::new(RichText::new(text).small().weak())
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Check now")
                        .clicked()
                    {
                        self.check_for_updates(true);
                    }
                }
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        section_label(ui, "FLASH");
        if nav_button(
            ui,
            "\u{e2de}  Smart Flash",
            self.page == FlashPage::SmartFlash,
        )
        .clicked()
        {
            self.page = FlashPage::SmartFlash;
        }
        ui.add_space(12.0);
        section_label(ui, "FIRMWARE");
        if nav_button(
            ui,
            "\u{e20c}  Firmware Download",
            self.page == FlashPage::FirmwareDownload,
        )
        .clicked()
        {
            self.page = FlashPage::FirmwareDownload;
        }
        if nav_button(
            ui,
            "\u{e256}  Local Firmware",
            self.page == FlashPage::LocalFirmware,
        )
        .clicked()
        {
            self.page = FlashPage::LocalFirmware;
            self.refresh_local_files();
        }
        ui.add_space(12.0);
        section_label(ui, "TOOLS");
        if nav_button(
            ui,
            "\u{e0e6}  Recovery & DFU",
            self.page == FlashPage::Guide,
        )
        .clicked()
        {
            self.page = FlashPage::Guide;
        }
        if nav_button(ui, "\u{eca0}  Toolbox", false).clicked() {
            self.tab = Tab::Toolbox;
        }

        // keep the project card at the bottom of the sidebar
        const CARD_HEIGHT: f32 = 190.0;
        let free = ui.available_height() - CARD_HEIGHT;
        if free > 0.0 {
            ui.add_space(free);
        }
        project_card(ui);
        ui.add_space(6.0);
        self.version_status(ui);
    }

    fn smart_flash_page(&mut self, ui: &mut egui::Ui) {
        if self.flash_view.is_some() {
            self.flash_progress_view(ui);
            return;
        }

        page_title(
            ui,
            "Smart Flash",
            "Pick a firmware, choose your options on the right and press Flash.",
        );
        ui.horizontal(|ui| {
            if ui.button("\u{e61e} Import Local Firmware").clicked() {
                self.import_local_firmware();
            }
            if ui.button("\u{e256} Open Folder").clicked() {
                if let Ok(dir) = firmware::firmware_dir() {
                    open_path(&dir);
                }
            }
            if ui.button("\u{e036} Refresh").clicked() {
                if let Some(p) = self.device_product() {
                    self.firmwares.remove(&p);
                }
                self.last_scan = None;
            }
            ui.checkbox(&mut self.show_unsigned, "Show unsigned");
        });
        ui.add_space(8.0);

        if let Some(Selection::Local(path)) = self.selection.clone() {
            theme::surface(ui, Surface::Card, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{eb08}").color(ACCENT));
                    ui.label(RichText::new("Local firmware:").weak());
                    ui.label(RichText::new(file_name(&path)).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("\u{e4f6} Remove").clicked() {
                            self.selection = None;
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }

        let Some(product) = self.device_product() else {
            empty_state(
                ui,
                "\u{e1e0}",
                "No device connected",
                &format!(
                    "Connect an iPhone or iPad via USB and {}.",
                    device::no_device_hint()
                ),
                self.scan_error.as_deref(),
            );
            return;
        };
        self.firmware_table(ui, &product, true);
    }

    fn firmware_download_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Firmware Download",
            "Download firmware for any iPhone or iPad model.",
        );
        ui.horizontal(|ui| {
            let current = self
                .browse
                .as_ref()
                .map(|b| b.identifier.clone())
                .or_else(|| self.device_product());
            let label = match (&self.browse, &current) {
                (Some(b), _) => format!("{} ({})", b.name, b.identifier),
                (None, Some(p)) => {
                    format!("{} ({p})", self.model_name(p).unwrap_or("Connected device"))
                }
                (None, None) => "Select a model ...".to_string(),
            };
            egui::ComboBox::from_id_salt("model_select")
                .selected_text(label)
                .width(340.0)
                .height(420.0)
                .show_ui(ui, |ui| {
                    self.ensure_catalog();
                    if self.device().is_some()
                        && ui
                            .selectable_label(self.browse.is_none(), "\u{e1e0} Connected device")
                            .clicked()
                    {
                        self.browse = None;
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Search model, e.g. iPad Pro"),
                    );
                    ui.separator();
                    match &self.catalog {
                        Some(Load::Ready(cat)) => {
                            let q = self.search.to_lowercase();
                            let mut chosen = None;
                            for entry in cat
                                .iter()
                                .filter(|e| {
                                    q.is_empty()
                                        || e.name.to_lowercase().contains(&q)
                                        || e.identifier.to_lowercase().contains(&q)
                                })
                                .take(300)
                            {
                                let sel = self
                                    .browse
                                    .as_ref()
                                    .is_some_and(|b| b.identifier == entry.identifier);
                                if ui
                                    .selectable_label(
                                        sel,
                                        format!("{}  ({})", entry.name, entry.identifier),
                                    )
                                    .clicked()
                                {
                                    chosen = Some(entry.clone());
                                }
                            }
                            if let Some(c) = chosen {
                                self.browse = Some(c);
                            }
                        }
                        Some(Load::Failed(e)) => {
                            ui.colored_label(RED, format!("Could not load models: {e}"));
                        }
                        _ => {
                            ui.spinner();
                        }
                    }
                });
            ui.checkbox(&mut self.show_unsigned, "Show unsigned");
        });
        ui.add_space(8.0);

        let product = self
            .browse
            .as_ref()
            .map(|b| b.identifier.clone())
            .or_else(|| self.device_product());
        match product {
            Some(p) => self.firmware_table(ui, &p, false),
            None => empty_state(
                ui,
                "\u{e20c}",
                "Choose a model",
                "Select an iPhone or iPad model above.",
                None,
            ),
        }
    }

    fn open_dfu_assistant(&mut self, family: Option<Family>) {
        let family = family
            .or(self.guide_family)
            .or_else(|| self.device().and_then(|d| Family::from_product(&d.product)))
            .unwrap_or(Family::IPhoneModern);
        self.dfu = Some(DfuAssistant {
            family,
            state: DfuState::Ready,
        });
    }

    /// Advance the assistant's timeline and react to the detected device mode.
    fn dfu_tick(&mut self) {
        let dfu_found = self.devices.iter().any(|d| d.mode == Mode::Dfu);
        let recovery_found = self.devices.iter().any(|d| d.mode == Mode::Recovery);
        let normal_found = self.devices.iter().any(|d| d.mode == Mode::Normal);
        let tools_ok = self.missing_tools.is_empty();
        let Some(dfu) = &mut self.dfu else { return };

        if dfu.active() && dfu_found {
            dfu.state = DfuState::Success;
            self.log("DFU assistant: device is in DFU mode");
            return;
        }
        let timeline = dfu.family.dfu_timeline();
        match dfu.state {
            DfuState::Countdown(since) if since.elapsed().as_secs_f32() >= DFU_COUNTDOWN => {
                dfu.state = DfuState::Running {
                    phase: 0,
                    since: Instant::now(),
                };
            }
            DfuState::Running { phase, since }
                if since.elapsed().as_secs_f32() >= timeline[phase].secs =>
            {
                dfu.state = if phase + 1 < timeline.len() {
                    DfuState::Running {
                        phase: phase + 1,
                        since: Instant::now(),
                    }
                } else {
                    DfuState::Waiting(Instant::now())
                };
            }
            DfuState::Waiting(since) if since.elapsed().as_secs_f32() >= DFU_GRACE => {
                let reason = if recovery_found {
                    "The device is in recovery mode instead. The button was probably held too long \
                     in the first step, or released too late. Try again and follow the timer closely."
                } else if normal_found {
                    "The device restarted normally (Apple logo). Start again and keep holding the \
                     second button until the timer ends."
                } else if tools_ok {
                    "No device detected. If the screen is black, the device may be in DFU mode but \
                     not visible: check the cable and, on Windows, that Apple's drivers (Apple \
                     Devices app or iTunes) are installed."
                } else {
                    "Device detection is not available because required tools are missing."
                };
                dfu.state = DfuState::Failed(reason.to_string());
                self.log(format!("DFU assistant: DFU mode not detected - {reason}"));
            }
            _ => {}
        }
    }

    fn dfu_window(&mut self, ctx: &egui::Context) {
        if self.dfu.is_none() {
            return;
        }
        self.dfu_tick();
        if self.dfu.as_ref().is_some_and(|d| d.active()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let detected = self.device().map(|d| d.mode.clone());
        let device_family = self.device().and_then(|d| Family::from_product(&d.product));
        let mut open = true;
        let mut action: Option<&str> = None;
        let Some(dfu) = &mut self.dfu else { return };
        let timeline = dfu.family.dfu_timeline();

        egui::Window::new("DFU assistant")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Device:").weak());
                    if matches!(dfu.state, DfuState::Ready | DfuState::Failed(_)) {
                        egui::ComboBox::from_id_salt("dfu_family")
                            .selected_text(dfu.family.label())
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                for f in Family::ALL {
                                    let label = if Some(f) == device_family {
                                        format!("{} (connected)", f.label())
                                    } else {
                                        f.label().to_string()
                                    };
                                    ui.selectable_value(&mut dfu.family, f, label);
                                }
                            });
                    } else {
                        ui.label(dfu.family.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Detected:").weak());
                    match &detected {
                        Some(mode) => mode_badge(ui, mode),
                        None => {
                            ui.label(RichText::new("no device").weak());
                        }
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                match &dfu.state {
                    DfuState::Ready => {
                        ui.add(
                            egui::Label::new(
                                "Connect the device with a USB cable. After pressing Start, do exactly \
                                 what the assistant shows, when the timer tells you to.",
                            )
                            .wrap(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("The device turns off during the process - that is expected.")
                                .weak()
                                .small(),
                        );
                        ui.add_space(12.0);
                        steps_overview(ui, timeline, None);
                        ui.add_space(12.0);
                        if ui
                            .add(
                                theme::accent_button("\u{e3d0}  Start", ACCENT, true)
                                    .min_size(egui::vec2(140.0, 34.0)),
                            )
                            .clicked()
                        {
                            action = Some("start");
                        }
                    }
                    DfuState::Countdown(since) => {
                        let left = (DFU_COUNTDOWN - since.elapsed().as_secs_f32()).max(0.0).ceil();
                        big_step(ui, "Get ready ...", None, left, 1.0 - left / DFU_COUNTDOWN);
                        ui.add_space(12.0);
                        steps_overview(ui, timeline, None);
                    }
                    DfuState::Running { phase, since } => {
                        let p = &timeline[*phase];
                        let elapsed = since.elapsed().as_secs_f32();
                        let left = (p.secs - elapsed).max(0.0).ceil();
                        ui.label(
                            RichText::new(format!("Step {} of {}", phase + 1, timeline.len())).weak(),
                        );
                        big_step(ui, p.text, p.hint, left, (elapsed / p.secs).min(1.0));
                        ui.add_space(12.0);
                        steps_overview(ui, timeline, Some(*phase));
                    }
                    DfuState::Waiting(_) => {
                        big_step(
                            ui,
                            "Release all buttons",
                            Some("Checking for the device ..."),
                            0.0,
                            1.0,
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Waiting for the device ...");
                        });
                    }
                    DfuState::Success => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("\u{e182}").color(GREEN).size(28.0));
                            ui.vertical(|ui| {
                                ui.label(theme::title("DFU mode detected", 18.0));
                                ui.label("You can release the button. The device is ready to be flashed.");
                            });
                        });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(theme::accent_button("\u{e2de}  Go to Smart Flash", ACCENT, true))
                                .clicked()
                            {
                                action = Some("flash");
                            }
                            if ui.button("Close").clicked() {
                                action = Some("close");
                            }
                        });
                    }
                    DfuState::Failed(reason) => {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("\u{e4f6}").color(RED).size(24.0));
                            ui.vertical(|ui| {
                                ui.label(theme::title("DFU mode not detected", 17.0));
                                ui.add(egui::Label::new(reason.as_str()).wrap());
                            });
                        });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(theme::accent_button("\u{e094}  Try again", ACCENT, true))
                                .clicked()
                            {
                                action = Some("start");
                            }
                            if ui.button("Show written guide").clicked() {
                                action = Some("guide");
                            }
                            if ui.button("Close").clicked() {
                                action = Some("close");
                            }
                        });
                    }
                }

                if dfu.active() {
                    ui.add_space(10.0);
                    if ui.button("Cancel").clicked() {
                        action = Some("close");
                    }
                }
            });

        match action {
            Some("start") => {
                if detected == Some(Mode::Dfu) {
                    dfu.state = DfuState::Success;
                } else {
                    dfu.state = DfuState::Countdown(Instant::now());
                    self.last_scan = None;
                }
            }
            Some("flash") => {
                self.dfu = None;
                self.tab = Tab::Flash;
                self.page = FlashPage::SmartFlash;
            }
            Some("guide") => {
                let family = dfu.family;
                self.dfu = None;
                self.guide_family = Some(family);
                self.tab = Tab::Flash;
                self.page = FlashPage::Guide;
            }
            Some("close") => self.dfu = None,
            _ => {}
        }
        if !open {
            self.dfu = None;
        }
    }

    fn guide_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Recovery & DFU mode",
            "If a restore fails, put the device into recovery or DFU mode and flash again.",
        );
        let detected = self.device().and_then(|d| Family::from_product(&d.product));
        let family = self
            .guide_family
            .or(detected)
            .unwrap_or(Family::IPhoneModern);

        ui.horizontal(|ui| {
            if ui
                .add(theme::accent_button(
                    "\u{e3d0}  Start DFU assistant",
                    ACCENT,
                    true,
                ))
                .on_hover_text(
                    "Guides you through the button sequence with a timer and detects DFU mode",
                )
                .clicked()
            {
                self.open_dfu_assistant(Some(family));
            }
            ui.add_space(12.0);
            ui.label("Device:");
            egui::ComboBox::from_id_salt("guide_family")
                .selected_text(family.label())
                .width(320.0)
                .show_ui(ui, |ui| {
                    for f in Family::ALL {
                        let label = if Some(f) == detected {
                            format!("{} (connected)", f.label())
                        } else {
                            f.label().to_string()
                        };
                        if ui.selectable_label(f == family, label).clicked() {
                            self.guide_family = Some(f);
                        }
                    }
                });
        });
        ui.add_space(10.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                theme::surface(&mut cols[0], Surface::Card, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::title("Recovery mode", 16.0));
                    ui.label(
                        RichText::new("Use this first. The screen shows a cable pointing to a computer.")
                            .weak()
                            .small(),
                    );
                    ui.add_space(8.0);
                    steps(ui, family.recovery_steps());
                });
                theme::surface(&mut cols[1], Surface::Card, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::title("DFU mode", 16.0));
                    ui.label(
                        RichText::new("Deepest restore mode - use it when recovery mode does not help. The screen stays black.")
                            .weak()
                            .small(),
                    );
                    ui.add_space(8.0);
                    steps(ui, family.dfu_steps());
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("If the Apple logo appears, a button was held too long - start again.")
                            .color(ORANGE)
                            .small(),
                    );
                });
            });
            ui.add_space(10.0);
            theme::surface(ui, Surface::Card, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Leave recovery / DFU mode").strong());
                ui.label(
                    "Use \"Exit recovery\" in Obsidia, or force restart the device:",
                );
                ui.label(RichText::new(family.force_restart()).weak());
            });
            if let Some(dev) = self.device() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Current mode:").weak());
                    mode_badge(ui, &dev.mode);
                });
            }
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About Obsidia")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(96.0, 96.0)));
                    }
                    ui.label(theme::title("Obsidia", 24.0));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).weak(),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "{}  Free and open-source software",
                            icons::SEAL_CHECK
                        ))
                        .color(GREEN),
                    );
                    ui.label("An open-source project by the MirraNET organization.");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        // center the two buttons
                        ui.add_space((ui.available_width() - 300.0).max(0.0) / 2.0);
                        if ui
                            .add(theme::accent_button(
                                &format!("{}  Source code", icons::GITHUB_LOGO),
                                ACCENT,
                                true,
                            ))
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(GITHUB_URL));
                        }
                        if ui
                            .button(format!("{}  mirranet.de", icons::GLOBE))
                            .clicked()
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(MIRRANET_URL));
                        }
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                egui::Grid::new("about")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("License").weak());
                        ui.label("MIT");
                        ui.end_row();
                        ui.label(RichText::new("Publisher").weak());
                        link(ui, "MirraNET", MIRRANET_URL);
                        ui.end_row();
                        ui.label(RichText::new("Issues").weak());
                        link(
                            ui,
                            "Report a problem on GitHub",
                            &format!("{GITHUB_URL}/issues"),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Uses").weak());
                        ui.vertical(|ui| {
                            link(
                                ui,
                                "libimobiledevice (LGPL)",
                                "https://libimobiledevice.org",
                            );
                            link(ui, "ipsw.me firmware data", "https://ipsw.me");
                            link(ui, "Phosphor Icons (MIT)", "https://phosphoricons.com");
                            link(ui, "Inter font (OFL)", "https://rsms.me/inter/");
                        });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Not affiliated with or endorsed by Apple Inc.")
                        .weak()
                        .small(),
                );
            });
        self.show_about = open;
    }

    fn open_drivers(&mut self) {
        self.drivers = Some(DriverPanel {
            status: None,
            remove_store: true,
        });
        self.spawn(|out| out.send(Msg::DriverStatus(drivers::status())));
    }

    fn start_driver_install(&mut self, remove_store: bool) {
        self.start_job(JobKind::Other, "Install Apple drivers", move |out| {
            let fail = |e: anyhow::Error| format!("{e:#}");
            if remove_store {
                out.send(Msg::JobStatus("Removing the Apple Devices app ...".into()));
                out.send(Msg::Log(
                    "Removing Store packages AppleInc.AppleDevices / AppleInc.iTunes".into(),
                ));
                drivers::remove_store_apps().map_err(fail)?;
            }
            out.send(Msg::JobStatus(
                "Downloading the iTunes installer from apple.com ...".into(),
            ));
            let mut last = 0u64;
            let installer = drivers::download_installer(|done, total| {
                let pct = (done * 100).checked_div(total).unwrap_or(0);
                if pct != last {
                    last = pct;
                    out.send(Msg::JobStatus(format!(
                        "Downloading the iTunes installer ... {pct}% ({})",
                        HumanBytes(done)
                    )));
                }
            })
            .map_err(fail)?;
            out.send(Msg::Log(format!(
                "Installer verified: {}",
                installer.display()
            )));
            out.send(Msg::JobStatus("Unpacking the driver package ...".into()));
            let msi = drivers::extract_driver(&installer).map_err(fail)?;
            out.send(Msg::Log(format!("Driver package: {}", msi.display())));
            out.send(Msg::JobStatus(
                "Installing drivers - confirm the Windows prompt ...".into(),
            ));
            drivers::install_msi(&msi).map_err(fail)?;
            out.send(Msg::DriverStatus(drivers::status()));
            Ok(
                "Apple drivers installed. Unplug the device and connect it again before flashing."
                    .into(),
            )
        });
    }

    fn drivers_window(&mut self, ctx: &egui::Context) {
        let Some(panel) = &mut self.drivers else {
            return;
        };
        let mut open = true;
        let mut install = None;
        let busy = self.job.is_some();
        egui::Window::new("Apple Drivers")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(500.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label(
                    "Newer devices, iPhone 14 and later in particular, often stop accepting firmware \
                     uploads in recovery mode with the drivers of the Microsoft Store apps. iTunes from \
                     apple.com ships a different driver package that handles these transfers reliably.",
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Obsidia downloads the official installer from apple.com, checks Apple's signature \
                         and installs only Apple Mobile Device Support - not iTunes itself.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("Installed").strong());
                match &panel.status {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Checking ...");
                        });
                    }
                    Some(status) => {
                        egui::Grid::new("driver-status").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                            ui.label(RichText::new("Classic drivers").weak());
                            match &status.classic {
                                Some(v) => ui.label(RichText::new(format!("\u{e182} Version {v}")).color(GREEN)),
                                None => ui.label(RichText::new("Not installed").color(ORANGE)),
                            };
                            ui.end_row();
                            ui.label(RichText::new("Store app").weak());
                            if status.store_app {
                                ui.label(RichText::new("Apple Devices / iTunes (Store) installed").color(ORANGE));
                            } else {
                                ui.label("Not installed");
                            }
                            ui.end_row();
                        });
                        ui.add_space(8.0);
                        if status.store_app {
                            ui.checkbox(
                                &mut panel.remove_store,
                                "Remove the Store app first (recommended, it brings its own drivers)",
                            );
                        }
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Windows asks for administrator rights. Disconnect all devices before you start.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = match panel.status.as_ref().and_then(|s| s.classic.as_ref()) {
                        Some(_) => "Reinstall drivers",
                        None => "Install drivers",
                    };
                    let enabled = panel.status.is_some() && !busy;
                    if ui.add_enabled(enabled, theme::accent_button(label, ACCENT, enabled)).clicked() {
                        let remove = panel.status.as_ref().is_some_and(|s| s.store_app) && panel.remove_store;
                        install = Some(remove);
                    }
                    if busy {
                        ui.label(RichText::new("Another task is running.").weak().small());
                    }
                });
            });
        if !open {
            self.drivers = None;
        }
        if let Some(remove_store) = install {
            self.drivers = None;
            self.start_driver_install(remove_store);
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut changed = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.label(RichText::new("Firmware").strong());
                changed |= ui
                    .checkbox(&mut self.settings.auto_clean, "Auto-clean expired firmware")
                    .changed();
                ui.label(
                    RichText::new(
                        "Downloaded firmware is reused while Apple signs it. With auto-clean, files Apple no longer signs are deleted automatically.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("Restore").strong());
                changed |= ui
                    .checkbox(
                        &mut self.settings.auto_retry,
                        "Retry automatically if the connection is lost (up to 3 attempts)",
                    )
                    .changed();
                ui.add_space(10.0);

                ui.label(RichText::new("Firmware folder").strong());
                let current = firmware::firmware_dir()
                    .unwrap_or_else(|_| firmware::default_firmware_dir());
                ui.label(RichText::new(current.display().to_string()).small());
                if firmware::is_cloud_synced(&current) {
                    ui.label(
                        RichText::new(
                            "\u{e4e0} This folder is synced with OneDrive. Firmware files are several GB and syncing \
                             can lock files during a restore - a local folder is recommended.",
                        )
                        .color(ORANGE)
                        .small(),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button("Change ...").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Choose firmware folder")
                            .set_directory(&current)
                            .pick_folder()
                        {
                            self.settings.firmware_dir = Some(dir);
                            changed = true;
                        }
                    }
                    if self.settings.firmware_dir.is_some() && ui.button("Reset to default").clicked() {
                        self.settings.firmware_dir = None;
                        changed = true;
                    }
                    if ui.button("Open").clicked() {
                        open_path(&current);
                    }
                });
                ui.label(
                    RichText::new("Existing files are not moved; copy them over if you want to keep them.")
                        .weak()
                        .small(),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("Updates").strong());
                changed |= ui
                    .checkbox(&mut self.settings.check_updates, "Check for updates on start-up")
                    .changed();
                if !updater::can_install() {
                    ui.label(
                        RichText::new(
                            "Installing updates needs administrator rights. You will still be told about new versions.",
                        )
                        .weak()
                        .small(),
                    );
                }
                if !updater::auto_check_enabled() {
                    ui.label(
                        RichText::new("Disabled by your administrator (policy or OBSIDIA_NO_UPDATE_CHECK).")
                            .color(ORANGE)
                            .small(),
                    );
                }
                if ui.button("Check now").clicked() {
                    self.update_dismissed = false;
                    self.check_for_updates(true);
                }
                ui.add_space(10.0);

                ui.label(RichText::new("Folders").strong());
                let base = firmware::data_dir();
                egui::Grid::new("folders").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                    for (label, dir) in [
                        ("Backups", base.join("Backups")),
                        ("Logs", base.join("Logs")),
                    ] {
                        ui.label(RichText::new(label).weak());
                        ui.label(RichText::new(dir.display().to_string()).small());
                        if ui.button("Open").clicked() {
                            let _ = std::fs::create_dir_all(&dir);
                            open_path(&dir);
                        }
                        ui.end_row();
                    }
                });
            });
        self.show_settings = open;
        if changed {
            firmware::set_firmware_dir(self.settings.firmware_dir.clone());
            self.settings.save();
            self.refresh_local_files();
        }
    }

    fn local_firmware_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Local Firmware",
            "Downloaded firmware is reused as long as Apple still signs it.",
        );
        ui.horizontal(|ui| {
            if ui.button("\u{e61e} Import Local Firmware").clicked() {
                self.import_local_firmware();
            }
            if ui.button("\u{e256} Open Folder").clicked() {
                if let Ok(dir) = firmware::firmware_dir() {
                    open_path(&dir);
                }
            }
            if ui.button("\u{e036} Refresh").clicked() {
                self.refresh_local_files();
            }
            let expired = self
                .local_files
                .iter()
                .filter(|f| self.local_status(f) == LocalStatus::Expired)
                .count();
            if ui
                .add_enabled(
                    expired > 0 && !self.busy(),
                    egui::Button::new(format!("\u{e4a6} Remove expired ({expired})")),
                )
                .clicked()
            {
                let n = self.remove_expired("Cleanup");
                self.notice = Some((format!("Removed {n} expired firmware file(s)."), false));
            }
            if ui
                .checkbox(&mut self.settings.auto_clean, "Auto-clean")
                .on_hover_text("Automatically delete firmware that Apple no longer signs")
                .changed()
            {
                self.settings.save();
                self.clean_pending = true;
            }
        });
        ui.add_space(8.0);

        if self.local_files.is_empty() {
            empty_state(
                ui,
                "\u{e256}",
                "No firmware downloaded yet",
                &format!(
                    "Downloaded firmware is stored in {}.",
                    firmware::data_dir().join("Firmware").display()
                ),
                None,
            );
            return;
        }

        let files = self.local_files.clone();
        let mut delete = None;
        let mut select = None;
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(150.0))
            .column(Column::exact(115.0))
            .column(Column::exact(75.0))
            .column(Column::exact(95.0))
            .column(Column::exact(205.0))
            .header(26.0, |mut h| {
                for t in ["File", "Version", "Size", "Status", ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|mut body| {
                for file in &files {
                    let status = self.local_status(file);
                    body.row(36.0, |mut row| {
                        row.col(|ui| {
                            ui.add(egui::Label::new(file.file_name()).truncate())
                                .on_hover_text(file.path.display().to_string());
                        });
                        row.col(|ui| {
                            let text = match (&file.version, &file.buildid) {
                                (Some(v), Some(b)) => format!("{v} ({b})"),
                                (None, Some(b)) => b.clone(),
                                _ => "-".to_string(),
                            };
                            ui.label(text);
                        });
                        row.col(|ui| {
                            ui.label(HumanBytes(file.size).to_string());
                        });
                        row.col(|ui| match status {
                            LocalStatus::Signed => {
                                ui.label(RichText::new("● Signed").color(GREEN));
                            }
                            LocalStatus::Expired => {
                                ui.label(RichText::new("\u{e18a} Expired").color(RED))
                                    .on_hover_text("Apple no longer signs this firmware");
                            }
                            LocalStatus::Checking => {
                                ui.spinner();
                            }
                            LocalStatus::Unknown => {
                                ui.label(RichText::new("? Unknown").weak())
                                    .on_hover_text("Signing status could not be determined");
                            }
                        });
                        row.col(|ui| {
                            let usable = status != LocalStatus::Expired;
                            if ui
                                .add_enabled(usable, egui::Button::new("Use for flashing"))
                                .on_disabled_hover_text("Apple no longer signs this firmware")
                                .clicked()
                            {
                                select = Some(file.path.clone());
                            }
                            if ui.button("\u{e4a6} Delete").clicked() {
                                delete = Some(file.path.clone());
                            }
                        });
                    });
                }
            });

        if let Some(p) = select {
            self.selection = Some(Selection::Local(p));
            self.page = FlashPage::SmartFlash;
        }
        if let Some(p) = delete {
            match firmware::delete_local(&p) {
                Ok(()) => self.log(format!("Deleted {}", p.display())),
                Err(e) => self.log(format!("Could not delete {}: {e}", p.display())),
            }
            if self.selection == Some(Selection::Local(p)) {
                self.selection = None;
            }
            self.refresh_local_files();
        }
    }

    /// Firmware list for `product`. With `selectable`, rows can be picked for flashing.
    fn firmware_table(&mut self, ui: &mut egui::Ui, product: &str, selectable: bool) {
        self.ensure_firmwares(product);
        let firmwares: Vec<Firmware> = match self.firmwares.get(product) {
            Some(Load::Ready(info)) => info
                .firmwares
                .iter()
                .filter(|f| self.show_unsigned || f.signed)
                .cloned()
                .collect(),
            Some(Load::Failed(e)) => {
                let e = e.clone();
                ui.colored_label(RED, format!("Could not load firmware list: {e}"));
                if ui.button("Retry").clicked() {
                    self.firmwares.remove(product);
                }
                return;
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading firmware list ...");
                });
                return;
            }
        };
        if firmwares.is_empty() {
            ui.label("Apple is currently not signing any firmware for this model.");
            return;
        }

        // Pre-select the newest signed firmware for the connected device.
        if selectable && self.selection.is_none() {
            if let Some(fw) = firmwares.iter().find(|f| f.signed) {
                self.selection = Some(Selection::Firmware(fw.url.clone()));
            }
        }

        let installed = self
            .device()
            .filter(|d| d.product == product)
            .and_then(|d| d.build.clone());
        let mut download = None;
        let mut cancel = None;
        let mut clicked = None;

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click());
        if selectable {
            table = table.column(Column::exact(28.0));
        }
        table
            .column(Column::exact(110.0))
            .column(Column::exact(80.0))
            .column(Column::exact(92.0))
            .column(Column::exact(76.0))
            .column(Column::exact(96.0))
            .column(Column::remainder().at_least(150.0))
            .header(26.0, |mut h| {
                if selectable {
                    h.col(|_| {});
                }
                for t in ["Version", "Build", "Released", "Size", "Status", "Download"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|mut body| {
                for fw in &firmwares {
                    let selected =
                        selectable && self.selection == Some(Selection::Firmware(fw.url.clone()));
                    body.row(38.0, |mut row| {
                        row.set_selected(selected);
                        if selectable {
                            row.col(|ui| {
                                let (c, t) = if selected {
                                    (ACCENT, "\u{eb08}")
                                } else {
                                    (Color32::GRAY, "\u{e18a}")
                                };
                                ui.label(RichText::new(t).color(c).size(16.0));
                            });
                        }
                        row.col(|ui| {
                            ui.label(RichText::new(&fw.version).strong());
                            if installed.as_deref() == Some(fw.buildid.as_str()) {
                                ui.label(RichText::new("installed").small().color(ACCENT));
                            }
                        });
                        row.col(|ui| {
                            ui.label(&fw.buildid);
                        });
                        row.col(|ui| {
                            ui.label(fw.release_day());
                        });
                        row.col(|ui| {
                            ui.label(HumanBytes(fw.filesize).to_string());
                        });
                        row.col(|ui| {
                            if fw.signed {
                                ui.label(RichText::new("● Signed").color(GREEN));
                            } else {
                                ui.label(RichText::new("\u{e18a} Not signed").weak());
                            }
                        });
                        row.col(|ui| {
                            if let Some(dl) = self.downloads.get(&fw.url) {
                                match dl.progress {
                                    Progress::Downloading { done, total } => {
                                        let frac = if total > 0 {
                                            done as f32 / total as f32
                                        } else {
                                            0.0
                                        };
                                        ui.add(
                                            egui::ProgressBar::new(frac)
                                                .desired_width(140.0)
                                                .show_percentage(),
                                        );
                                        if ui
                                            .button("\u{e4f6}")
                                            .on_hover_text("Cancel (can be resumed)")
                                            .clicked()
                                        {
                                            cancel = Some(fw.url.clone());
                                        }
                                    }
                                    Progress::Verifying => {
                                        ui.spinner();
                                        ui.label("Verifying ...");
                                    }
                                }
                            } else if fw.is_cached() {
                                ui.label(RichText::new("\u{e182} Downloaded").color(GREEN));
                            } else if ui.button("\u{e20c} Download").clicked() {
                                download = Some(fw.clone());
                            }
                        });
                        if row.response().clicked() {
                            clicked = Some(fw.url.clone());
                        }
                    });
                }
            });

        if let Some(fw) = download {
            self.start_download(&fw);
        }
        if let Some(url) = cancel {
            if let Some(d) = self.downloads.get(&url) {
                d.cancel.store(true, Ordering::Relaxed);
            }
        }
        if selectable {
            if let Some(url) = clicked {
                self.selection = Some(Selection::Firmware(url));
            }
        }
    }

    fn flash_progress_view(&mut self, ui: &mut egui::Ui) {
        let Some(view) = &self.flash_view else { return };
        let label = view.label.clone();
        let result = view.result.clone();
        let mut close = false;

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if ui.button("View log").clicked() {
                self.show_log = true;
            }
        });

        ui.vertical_centered(|ui| {
            let avail = ui.available_height();
            ui.add_space((avail * 0.12).max(10.0));

            // Hero image: the Obsidia crystal, with a status badge.
            let size = 170.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            if let Some(logo) = &self.logo {
                egui::Image::new(logo).paint_at(ui, rect);
            }
            let badge = rect.right_bottom() - egui::vec2(18.0, 18.0);
            match &result {
                Some(Ok(_)) => {
                    ui.painter().circle_filled(badge, 22.0, GREEN);
                    ui.painter().text(
                        badge,
                        egui::Align2::CENTER_CENTER,
                        "\u{e182}",
                        egui::FontId::proportional(24.0),
                        Color32::WHITE,
                    );
                }
                Some(Err(_)) => {
                    ui.painter().circle_filled(badge, 22.0, DANGER);
                    ui.painter().text(
                        badge,
                        egui::Align2::CENTER_CENTER,
                        "\u{e4f6}",
                        egui::FontId::proportional(22.0),
                        Color32::WHITE,
                    );
                }
                None => {}
            }

            ui.add_space(28.0);
            let width = (ui.available_width() * 0.7).min(640.0);
            match (&result, &self.job) {
                (None, Some(job)) => {
                    let pct = job.percent.or_else(|| parse_percent(&job.status));
                    let bar = match pct {
                        Some(p) => egui::ProgressBar::new(p / 100.0),
                        None => egui::ProgressBar::new(0.0).animate(true),
                    };
                    ui.add(bar.desired_width(width).desired_height(8.0).fill(ACCENT));
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(format!("Flashing {label}"))
                            .size(17.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(&job.status).weak());
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Do not disconnect the device.")
                            .color(ORANGE)
                            .small(),
                    );
                }
                (Some(Ok(msg)), _) => {
                    ui.add(
                        egui::ProgressBar::new(1.0)
                            .desired_width(width)
                            .desired_height(8.0)
                            .fill(GREEN),
                    );
                    ui.add_space(14.0);
                    ui.label(theme::title("Congratulations!", 18.0));
                    ui.label(RichText::new(msg).weak());
                    ui.add_space(14.0);
                    if ui
                        .add(
                            theme::accent_button("Done", ACCENT, true)
                                .min_size(egui::vec2(160.0, 34.0)),
                        )
                        .clicked()
                    {
                        close = true;
                    }
                }
                (Some(Err(e)), _) => {
                    ui.add(
                        egui::ProgressBar::new(1.0)
                            .desired_width(width)
                            .desired_height(8.0)
                            .fill(DANGER),
                    );
                    ui.add_space(14.0);
                    ui.label(theme::title("Flashing failed", 18.0).color(RED));
                    ui.label(RichText::new(e).weak());
                    ui.add_space(14.0);
                    if ui
                        .add(
                            theme::accent_button("Back", ACCENT, true)
                                .min_size(egui::vec2(160.0, 34.0)),
                        )
                        .clicked()
                    {
                        close = true;
                    }
                    ui.add_space(6.0);
                    if ui.button("Enter DFU mode (assistant)").clicked() {
                        close = true;
                        self.open_dfu_assistant(None);
                    }
                }
                (None, None) => close = true,
            }
        });

        if close {
            self.flash_view = None;
        }
    }

    /// Right-hand device panel with options and the Flash button.
    fn device_side_panel(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.device().cloned() else {
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);
                device_picture(ui, "iPhone", 150.0, false);
                ui.add_space(12.0);
                ui.label(RichText::new("No device").strong());
                ui.label(RichText::new("Connect via USB").weak().small());
            });
            return;
        };
        if dev.product != "?" {
            self.ensure_firmwares(&dev.product);
        }
        let model = self
            .model_name(&dev.product)
            .unwrap_or(dev.kind())
            .to_string();

        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            device_picture(ui, dev.kind(), 150.0, dev.mode == Mode::Normal);
        });
        ui.add_space(10.0);

        if self.devices.len() > 1 {
            let devs = self.devices.clone();
            egui::ComboBox::from_id_salt("device_select")
                .selected_text(&model)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for d in &devs {
                        let sel = self.selected_device.as_deref() == Some(d.key());
                        if ui
                            .selectable_label(sel, format!("{} [{}] {}", d.kind(), d.mode, d.name))
                            .clicked()
                        {
                            self.selected_device = Some(d.key().to_string());
                            self.selection = None;
                        }
                    }
                });
        } else {
            ui.label(theme::title(&model, 17.0));
        }
        if dev.name != model && dev.name != "-" {
            ui.label(RichText::new(&dev.name).weak());
        }
        ui.add_space(8.0);

        egui::Grid::new("side_info")
            .num_columns(2)
            .spacing([14.0, 5.0])
            .show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(RichText::new(k).weak());
                    ui.label(v);
                    ui.end_row();
                };
                if let Some(s) = &dev.serial {
                    row("SN", s.clone());
                }
                if let Some(v) = &dev.version {
                    row(
                        if dev.kind() == "iPad" {
                            "iPadOS"
                        } else {
                            "iOS"
                        },
                        v.clone(),
                    );
                }
                row("Mode", format!("{} Mode", dev.mode));
                if let Some(c) = dev.capacity {
                    row("Capacity", HumanBytes(c).to_string());
                }
                if let Some(b) = dev.battery {
                    row("Battery", format!("{b} %"));
                }
            });

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new("Keep data or not").strong());
        ui.add_space(2.0);
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.radio_value(&mut self.erase, false, "Keep data (update)");
            ui.radio_value(&mut self.erase, true, "Erase (clean restore)");
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(if self.erase {
                "All data on the device will be deleted."
            } else {
                "Apps and data are kept. Not possible for downgrades."
            })
            .small()
            .weak(),
        );

        // Flash button pinned to the bottom of the panel.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            let blocker = self.flash_blocker();
            let label = if self.busy() {
                "Flashing ..."
            } else {
                "\u{e2de}  Flash"
            };
            let btn = theme::accent_button(
                label,
                if self.erase { ACCENT } else { GREEN },
                blocker.is_none(),
            )
            .min_size(egui::vec2(ui.available_width(), 42.0));
            let resp = ui.add_enabled(blocker.is_none(), btn);
            let resp = match blocker {
                Some(reason) => resp.on_disabled_hover_text(reason),
                None => resp,
            };
            if resp.clicked() {
                self.request_flash();
            }
            if let Some(reason) = blocker {
                ui.label(RichText::new(reason).small().weak());
            }
        });
    }

    fn device_tab(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Device", "Overview of the connected iPhone or iPad.");
        let Some(dev) = self.device().cloned() else {
            empty_state(
                ui,
                "\u{e1e0}",
                "No device connected",
                &format!(
                    "Connect an iPhone or iPad via USB and {}.",
                    device::no_device_hint()
                ),
                self.scan_error.as_deref(),
            );
            return;
        };
        if dev.product != "?" {
            self.ensure_firmwares(&dev.product);
        }
        let model = self
            .model_name(&dev.product)
            .unwrap_or(dev.kind())
            .to_string();

        if self.devices.len() > 1 {
            ui.horizontal(|ui| {
                let devs = self.devices.clone();
                for d in &devs {
                    let sel = self.selected_device.as_deref() == Some(d.key());
                    if ui
                        .selectable_label(sel, format!("{} {} [{}]", d.kind(), d.name, d.mode))
                        .clicked()
                    {
                        self.selected_device = Some(d.key().to_string());
                        self.selection = None;
                    }
                }
            });
            ui.add_space(8.0);
        }

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(260.0);
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    device_picture(ui, dev.kind(), 300.0, dev.mode == Mode::Normal)
                });
            });
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.label(theme::title(&model, 26.0));
                if dev.name != model && dev.name != "-" {
                    ui.label(RichText::new(&dev.name).weak().size(15.0));
                }
                ui.add_space(6.0);
                mode_badge(ui, &dev.mode);
                ui.add_space(14.0);
                theme::surface(ui, Surface::Card, |ui| {
                    ui.set_width(ui.available_width().min(560.0));
                    egui::Grid::new("device_info")
                        .num_columns(2)
                        .spacing([24.0, 8.0])
                        .show(ui, |ui| {
                            let mut row = |k: &str, v: String| {
                                ui.label(RichText::new(k).weak());
                                ui.label(v);
                                ui.end_row();
                            };
                            row("Model identifier", dev.product.clone());
                            if let Some(v) = &dev.version {
                                row(
                                    if dev.kind() == "iPad" {
                                        "iPadOS"
                                    } else {
                                        "iOS"
                                    },
                                    v.clone(),
                                );
                            }
                            if let Some(b) = &dev.build {
                                row("Build", b.clone());
                            }
                            if let Some(c) = dev.capacity {
                                row("Capacity", HumanBytes(c).to_string());
                            }
                            if let Some(b) = dev.battery {
                                row("Battery", format!("{b} %"));
                            }
                            if let Some(s) = &dev.serial {
                                row("Serial number", s.clone());
                            }
                            if !dev.ecid.is_empty() {
                                row("ECID", dev.ecid.clone());
                            }
                            if !dev.udid.is_empty() {
                                row("UDID", dev.udid.clone());
                            }
                        });
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let busy = self.busy();
                    let normal = dev.mode == Mode::Normal;
                    if ui
                        .add_enabled(!busy && normal, egui::Button::new("\u{e248}  Backup"))
                        .clicked()
                    {
                        self.start_backup(dev.clone());
                    }
                    if ui
                        .add_enabled(
                            !busy && normal,
                            egui::Button::new("\u{e5d4}  Enter recovery"),
                        )
                        .clicked()
                    {
                        self.start_enter_recovery(dev.clone());
                    }
                    if ui
                        .add_enabled(
                            !busy && !normal,
                            egui::Button::new("\u{e08a}  Exit recovery"),
                        )
                        .clicked()
                    {
                        self.start_exit_recovery();
                    }
                    if ui
                        .add(theme::accent_button("\u{e2de}  Flash", ACCENT, true))
                        .clicked()
                    {
                        self.tab = Tab::Flash;
                        self.page = FlashPage::SmartFlash;
                    }
                });
            });
        });
    }

    fn toolbox_tab(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Toolbox", "Useful tools for your device.");
        let dev = self.device().cloned();
        let busy = self.busy();
        let normal = dev.as_ref().is_some_and(|d| d.mode == Mode::Normal);
        let in_recovery = dev.as_ref().is_some_and(|d| d.mode != Mode::Normal);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(14.0, 14.0);
            if tool_tile(
                ui,
                "\u{e248}",
                "Backup",
                "Back up apps and data",
                !busy && normal,
            ) {
                if let Some(d) = dev.clone() {
                    self.start_backup(d);
                }
            }
            if tool_tile(
                ui,
                "\u{e5d4}",
                "Enter Recovery",
                "Boot into recovery mode",
                !busy && normal,
            ) {
                if let Some(d) = dev.clone() {
                    self.start_enter_recovery(d);
                }
            }
            if tool_tile(
                ui,
                "\u{e08a}",
                "Exit Recovery",
                "Reboot into iOS / iPadOS",
                !busy && in_recovery,
            ) {
                self.start_exit_recovery();
            }
            if tool_tile(ui, "\u{e256}", "Backups", "Open the backup folder", true) {
                let dir = device::default_backup_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
            if tool_tile(ui, "\u{e20c}", "Firmware", "Open the firmware folder", true) {
                if let Ok(dir) = firmware::firmware_dir() {
                    open_path(&dir);
                }
            }
            if tool_tile(ui, "\u{e2f2}", "Log", "Show the activity log", true) {
                self.show_log = true;
            }
            let checking = matches!(self.update, Some(UpdateState::Checking));
            if tool_tile(
                ui,
                "\u{e036}",
                "Updates",
                "Check for a new version",
                !checking,
            ) {
                self.update_dismissed = false;
                self.check_for_updates(true);
            }
            if tool_tile(
                ui,
                "\u{e3d0}",
                "DFU Assistant",
                "Timed guide into DFU mode",
                true,
            ) {
                self.open_dfu_assistant(None);
            }
            if tool_tile(
                ui,
                "\u{e0e6}",
                "Recovery & DFU",
                "Step-by-step guides",
                true,
            ) {
                self.tab = Tab::Flash;
                self.page = FlashPage::Guide;
            }
            if drivers::supported()
                && tool_tile(
                    ui,
                    "\u{e956}",
                    "Apple Drivers",
                    "Classic iTunes USB drivers",
                    !busy,
                )
            {
                self.open_drivers();
            }
            if tool_tile(ui, icons::INFO, "About", "Version, license, MirraNET", true) {
                self.show_about = true;
            }
            if tool_tile(
                ui,
                "\u{e270}",
                "Settings",
                "Auto-clean, updates, folders",
                true,
            ) {
                self.show_settings = true;
            }
        });

        if !self.missing_tools.is_empty() {
            ui.add_space(16.0);
            self.tools_banner(ui);
        }
    }

    fn tools_banner(&mut self, ui: &mut egui::Ui) {
        theme::surface_frame(Surface::Card)
            .fill(ORANGE.gamma_multiply(0.14))
            .stroke(Stroke::new(1.0, ORANGE.gamma_multiply(0.6)))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("\u{e4e0} Required tools missing: {}", self.missing_tools.join(", ")))
                        .strong()
                        .color(ORANGE),
                );
                ui.label("Firmware browsing and downloads work, but devices cannot be detected or flashed:");
                ui.label(RichText::new(device::INSTALL_HINT).monospace());
                if ui.button("Check again").clicked() {
                    self.missing_tools = device::missing_tools();
                    self.last_scan = None;
                }
            });
        ui.add_space(8.0);
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_log;
        egui::Window::new("Log")
            .open(&mut open)
            .default_size([760.0, 420.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(self.log.join("\n"));
                    }
                    if ui.button("Clear").clicked() {
                        self.log.clear();
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(RichText::new(line).monospace().size(12.0));
                        }
                    });
            });
        self.show_log = open;
    }

    fn flash_dialog(&mut self, ctx: &egui::Context) {
        let Some(req) = &mut self.pending_flash else {
            return;
        };
        let mut action = None;
        egui::Modal::new(egui::Id::unique("flash_confirm"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(theme::surface_frame(Surface::Raised).inner_margin(22))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Flash firmware");
                ui.add_space(8.0);
                egui::Grid::new("flash_info").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    ui.label(RichText::new("Device").weak());
                    ui.label(format!("{} {} [{}]", req.device.kind(), req.device.product, req.device.mode));
                    ui.end_row();
                    ui.label(RichText::new("Firmware").weak());
                    ui.label(RichText::new(&req.label).strong());
                    ui.end_row();
                    ui.label(RichText::new("Mode").weak());
                    ui.label(if req.erase { "Erase (clean restore)" } else { "Keep data (update)" });
                    ui.end_row();
                });
                ui.add_space(10.0);
                if req.erase {
                    ui.label(
                        RichText::new(
                            "\u{e4e0} All data on the device will be deleted. Create a backup first and make sure you know \
                             the Apple ID password (Activation Lock).",
                        )
                        .color(ORANGE),
                    );
                } else {
                    ui.label(RichText::new("Apps and data are kept. A backup is still recommended.").weak());
                }
                ui.label(RichText::new("Do not disconnect the device until it has rebooted.").weak());
                if let Some(b) = req.device.battery.filter(|b| *b < 20) {
                    ui.label(
                        RichText::new(format!(
                            "\u{e4e0} Battery is at {b} %. Charge the device first for a reliable restore."
                        ))
                        .color(ORANGE),
                    );
                }
                if firmware::firmware_dir().is_ok_and(|d| firmware::is_cloud_synced(&d)) {
                    ui.label(
                        RichText::new(
                            "\u{e4e0} The firmware folder is synced with OneDrive. Pause OneDrive or choose a local \
                             folder in the settings if the restore fails.",
                        )
                        .color(ORANGE)
                        .small(),
                    );
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = Some(false);
                    }
                    let txt = if req.erase { "Erase & flash" } else { "Update" };
                    if ui.add(theme::accent_button(txt, if req.erase { DANGER } else { ACCENT }, true)).clicked() {
                        action = Some(true);
                    }
                });
            });
        match action {
            Some(true) => {
                let req = self.pending_flash.take().unwrap();
                self.start_flash(req);
            }
            Some(false) => self.pending_flash = None,
            None => {}
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(path) = dropped.into_iter().find(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ipsw"))
        }) {
            self.log(format!("Selected local firmware: {}", path.display()));
            self.selection = Some(Selection::Local(path));
            self.tab = Tab::Flash;
            self.page = FlashPage::SmartFlash;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_messages();
        self.maybe_scan();
        self.auto_clean();
        self.handle_dropped_files(&ctx);
        ctx.request_repaint_after(Duration::from_millis(if self.busy() { 250 } else { 1000 }));

        theme::paint_backdrop(ui);
        let outer = |l: i8, r: i8, t: i8, b: i8| {
            egui::Frame::NONE.inner_margin(egui::Margin {
                left: l,
                right: r,
                top: t,
                bottom: b,
            })
        };

        egui::Panel::top("header")
            .show_separator_line(false)
            .frame(outer(16, 16, 10, 6))
            .show(ui, |ui| self.header(ui));

        if self.show_update_bar() {
            egui::Panel::top("update_bar")
                .show_separator_line(false)
                .frame(outer(12, 12, 0, 6))
                .show(ui, |ui| self.update_bar(ui));
        }

        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(outer(18, 18, 2, 8))
            .show(ui, |ui| self.status_bar(ui));

        match self.tab {
            Tab::Flash => {
                egui::Panel::left("sidebar")
                    .resizable(false)
                    .show_separator_line(false)
                    .exact_size(220.0)
                    .frame(outer(10, 4, 4, 4))
                    .show(ui, |ui| self.sidebar(ui));

                egui::Panel::right("device_side")
                    .resizable(false)
                    .show_separator_line(false)
                    .exact_size(300.0)
                    .frame(outer(4, 12, 4, 4))
                    .show(ui, |ui| {
                        theme::surface_fill(ui, Surface::Layer, |ui| self.device_side_panel(ui))
                    });

                egui::CentralPanel::no_frame().show(ui, |ui| {
                    outer(4, 4, 4, 4).show(ui, |ui| {
                        theme::surface_fill(ui, Surface::Layer, |ui| {
                            if !self.missing_tools.is_empty() {
                                self.tools_banner(ui);
                            }
                            match self.page {
                                FlashPage::SmartFlash => self.smart_flash_page(ui),
                                FlashPage::FirmwareDownload => self.firmware_download_page(ui),
                                FlashPage::LocalFirmware => self.local_firmware_page(ui),
                                FlashPage::Guide => self.guide_page(ui),
                            }
                        })
                    });
                });
            }
            Tab::Device | Tab::Toolbox => {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    outer(12, 12, 4, 4).show(ui, |ui| {
                        theme::surface_fill(ui, Surface::Layer, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if self.tab == Tab::Device {
                                    self.device_tab(ui);
                                } else {
                                    self.toolbox_tab(ui);
                                }
                            });
                        })
                    });
                });
            }
        }

        self.log_window(&ctx);
        self.release_notes_window(&ctx);
        self.settings_window(&ctx);
        self.dfu_window(&ctx);
        self.about_window(&ctx);
        self.drivers_window(&ctx);
        self.flash_dialog(&ctx);
    }
}

/// Selector-bar style tab: plain text, accent pill under the active tab.
fn tab_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let text = if active {
        theme::title(label, 15.0)
    } else {
        RichText::new(label)
            .size(15.0)
            .color(Color32::from_gray(200))
    };
    let resp = ui.add(
        egui::Button::new(text)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(4)
            .min_size(egui::vec2(84.0, 36.0)),
    );
    if active {
        let r = resp.rect;
        let pill = egui::Rect::from_center_size(
            egui::pos2(r.center().x, r.bottom() - 2.0),
            egui::vec2(18.0, 3.0),
        );
        ui.painter().rect_filled(pill, 1.5, ACCENT);
    }
    resp
}

/// Navigation view item with the accent indicator on the left.
fn nav_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let text = RichText::new(label).size(14.0);
    let fill = if active {
        Color32::from_white_alpha(14)
    } else {
        Color32::TRANSPARENT
    };
    let resp = ui.add(
        egui::Button::new(text)
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(4)
            .min_size(egui::vec2(ui.available_width(), 36.0)),
    );
    if active {
        let r = resp.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                r.left_top() + egui::vec2(0.0, 10.0),
                egui::vec2(3.0, r.height() - 20.0),
            ),
            1.5,
            ACCENT,
        );
    }
    resp
}

fn link(ui: &mut egui::Ui, text: &str, url: &str) {
    let resp = ui
        .add(
            egui::Label::new(RichText::new(text).small().color(ACCENT)).sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(url);
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(11.0).weak());
    ui.add_space(2.0);
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(theme::title(title, 20.0));
    ui.add_space(2.0);
    ui.label(RichText::new(subtitle).weak());
    ui.add_space(14.0);
}

fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, text: &str, error: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.2);
        ui.label(RichText::new(icon).size(46.0).weak());
        ui.add_space(6.0);
        ui.label(theme::title(title, 17.0));
        ui.label(RichText::new(text).weak());
        if let Some(e) = error {
            ui.add_space(8.0);
            ui.label(RichText::new(e).color(RED).small());
        }
    });
}

/// Large clickable tile for the Toolbox tab. Returns true when clicked.
fn tool_tile(ui: &mut egui::Ui, icon: &str, title: &str, text: &str, enabled: bool) -> bool {
    let size = egui::vec2(190.0, 120.0);
    let (rect, resp) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hovered = enabled && resp.hovered();
    let fill = Color32::from_white_alpha(if hovered { 22 } else { 10 });
    let stroke = Color32::from_white_alpha(if hovered { 60 } else { 28 });
    ui.painter().rect(
        rect,
        16.0,
        fill,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    let alpha = if enabled { 255 } else { 90 };
    let text_color = Color32::from_white_alpha(alpha);
    ui.painter().text(
        rect.left_top() + egui::vec2(18.0, 18.0),
        egui::Align2::LEFT_TOP,
        icon,
        egui::FontId::proportional(28.0),
        if enabled { ACCENT } else { text_color },
    );
    ui.painter().text(
        rect.left_top() + egui::vec2(18.0, 64.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(15.0),
        text_color,
    );
    ui.painter().text(
        rect.left_top() + egui::vec2(18.0, 86.0),
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(11.5),
        Color32::from_white_alpha(alpha / 2),
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    } else {
        false
    }
}

/// Draw a stylized iPhone / iPad of the given height.
fn device_picture(ui: &mut egui::Ui, kind: &str, height: f32, screen_on: bool) {
    let is_ipad = kind == "iPad";
    let width = if is_ipad {
        height * 0.74
    } else {
        height * 0.49
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let p = ui.painter();
    let radius = width * if is_ipad { 0.08 } else { 0.16 };

    // Soft shadow, body and frame.
    p.rect_filled(
        rect.translate(egui::vec2(0.0, 6.0)).expand(2.0),
        radius + 2.0,
        Color32::from_black_alpha(70),
    );
    p.rect(
        rect,
        radius,
        Color32::from_rgb(0x14, 0x16, 0x1b),
        Stroke::new(2.0, Color32::from_rgb(0x5a, 0x61, 0x6e)),
        egui::StrokeKind::Inside,
    );

    // Screen with a gentle gradient.
    let bezel = width * if is_ipad { 0.05 } else { 0.06 };
    let screen = rect.shrink(bezel);
    let (top, bottom) = if screen_on {
        (
            Color32::from_rgb(0x2a, 0x4f, 0x8a),
            Color32::from_rgb(0x12, 0x1c, 0x30),
        )
    } else {
        (
            Color32::from_rgb(0x1c, 0x1f, 0x26),
            Color32::from_rgb(0x0c, 0x0d, 0x10),
        )
    };
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(screen.left_top(), top);
    mesh.colored_vertex(screen.right_top(), top);
    mesh.colored_vertex(screen.right_bottom(), bottom);
    mesh.colored_vertex(screen.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    // Clip the square gradient to the rounded screen by drawing it first and
    // re-stroking the rounded corners in body color.
    p.add(mesh);
    p.rect_stroke(
        screen.expand(bezel * 0.5),
        radius - bezel * 0.5,
        Stroke::new(bezel, Color32::from_rgb(0x14, 0x16, 0x1b)),
        egui::StrokeKind::Middle,
    );

    // Dynamic Island for iPhones.
    if !is_ipad {
        let island = egui::Rect::from_center_size(
            egui::pos2(screen.center().x, screen.top() + width * 0.07),
            egui::vec2(width * 0.3, width * 0.08),
        );
        p.rect_filled(island, width * 0.04, Color32::BLACK);
    }

    // top edge highlight
    p.line_segment(
        [
            rect.left_top() + egui::vec2(radius, 1.5),
            rect.right_top() + egui::vec2(-radius, 1.5),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(70)),
    );
}

fn mode_badge(ui: &mut egui::Ui, mode: &Mode) {
    let color = match mode {
        Mode::Normal => GREEN,
        Mode::Recovery => ORANGE,
        Mode::Dfu => RED,
        Mode::Other(_) => Color32::GRAY,
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.22))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("● {mode} Mode")).color(color).small());
        });
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Extract a trailing percentage like "... 42.5%" from a progress line.
fn parse_percent(line: &str) -> Option<f32> {
    let end = line.rfind('%')?;
    let start = line[..end]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    line[start..end]
        .parse::<f32>()
        .ok()
        .filter(|p| (0.0..=100.0).contains(p))
}

fn open_path(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// Most useful error printed by idevicerestore. stdout and stderr arrive out
/// of order, so an answer from Apple's signing server wins over the generic
/// last error line.
fn last_error_line(lines: &[String]) -> Option<String> {
    if let Some(tss) = lines.iter().find(|l| l.contains("TSS server returned")) {
        let message = tss
            .split("MESSAGE=")
            .nth(1)
            .map(|m| m.trim().to_string())
            .unwrap_or_else(|| tss.trim().to_string());
        let status = tss
            .split("STATUS=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("?");
        let hint = match status {
            "94" => " Apple refused to sign this firmware for this device. Make sure the version is still signed; if it is, the bundled restore tool may be outdated.",
            _ => "",
        };
        return Some(format!(
            "Apple signing server (status {status}): {message}.{hint}"
        ));
    }
    lines
        .iter()
        .rev()
        .find_map(|l| l.find("ERROR:").map(|i| l[i..].trim().to_string()))
}

/// Save the output of a restore to Documents/Obsidia/Logs.
fn write_restore_log(lines: &[String]) -> Option<PathBuf> {
    let dir = firmware::logs_dir().ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("restore-{stamp}.log"));
    std::fs::write(&path, lines.join("\n")).ok()?;
    Some(path)
}

fn steps(ui: &mut egui::Ui, steps: &[&str]) {
    for (i, step) in steps.iter().enumerate() {
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 10.0, Color32::from_white_alpha(18));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                egui::FontId::proportional(12.0),
                ACCENT,
            );
            ui.add(egui::Label::new(*step).wrap());
        });
        ui.add_space(4.0);
    }
}

/// Large instruction with countdown for the DFU assistant.
fn big_step(ui: &mut egui::Ui, text: &str, hint: Option<&str>, seconds_left: f32, progress: f32) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(430.0);
            ui.add(egui::Label::new(theme::title(text, 20.0)).wrap());
            if let Some(h) = hint {
                ui.label(RichText::new(h).weak());
            }
        });
        if seconds_left > 0.0 {
            ui.label(
                RichText::new(format!("{seconds_left:.0}"))
                    .size(40.0)
                    .strong()
                    .color(ACCENT),
            );
        }
    });
    ui.add_space(6.0);
    ui.add(
        egui::ProgressBar::new(progress.clamp(0.0, 1.0))
            .desired_height(6.0)
            .fill(ACCENT),
    );
}

/// Compact list of all assistant steps; the current one is highlighted.
fn steps_overview(ui: &mut egui::Ui, timeline: &[crate::guides::Phase], current: Option<usize>) {
    for (i, phase) in timeline.iter().enumerate() {
        let (marker, color) = match current {
            Some(c) if i < c => ("\u{e182}", GREEN),
            Some(c) if i == c => ("\u{e3d0}", ACCENT),
            _ => ("•", Color32::from_gray(140)),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(marker).color(color));
            let duration = if phase.secs < 3.0 {
                "quick".to_string()
            } else {
                format!("{:.0} s", phase.secs)
            };
            let text = format!("{}  ({duration})", phase.text);
            let text = if current == Some(i) {
                RichText::new(text).strong()
            } else {
                RichText::new(text).weak()
            };
            ui.label(text);
        });
    }
}

const MAX_RESTORE_ATTEMPTS: u32 = 3;

/// A retry only makes sense for connection problems, not if Apple refused to
/// sign the firmware or the IPSW does not fit the device.
fn retryable(lines: &[String]) -> bool {
    let fatal = [
        "TSS server returned",
        "isn't eligible",
        "not compatible",
        "No matching build identity",
        "checksum",
        "Unable to open ipsw",
    ];
    !lines.iter().any(|l| fatal.iter().any(|f| l.contains(f)))
}

/// Checks before a restore so it doesn't fail halfway through.
fn preflight(ipsw: &Path) -> anyhow::Result<()> {
    let blocking = device::blocking_apple_apps();
    if !blocking.is_empty() {
        anyhow::bail!(
            "Please close {} first. It takes over devices in recovery mode, so the firmware can't be \
             uploaded.",
            blocking.join(" and ")
        );
    }
    if device::apple_service_running() == Some(false) {
        anyhow::bail!(
            "The Apple Mobile Device Service is not running. Install or repair the Apple Devices app \
             (or iTunes), or start the service in services.msc."
        );
    }
    // idevicerestore extracts the root filesystem next to the IPSW.
    let size = std::fs::metadata(ipsw).map(|m| m.len()).unwrap_or(0);
    let extracted = ipsw.with_extension("").is_dir();
    if let (Some(dir), false) = (ipsw.parent(), extracted) {
        if let Some(free) = firmware::free_space(dir) {
            let needed = size + 1024 * 1024 * 1024;
            if free < needed {
                anyhow::bail!(
                    "Not enough free disk space in {}: {} needed, {} available.",
                    dir.display(),
                    HumanBytes(needed),
                    HumanBytes(free)
                );
            }
        }
    }
    Ok(())
}

/// Card at the bottom of the sidebar that marks Obsidia as a MirraNET
/// open-source project.
fn project_card(ui: &mut egui::Ui) {
    theme::surface(ui, Surface::Card, |ui| {
        ui.set_width(ui.available_width());
        project_card_content(ui);
    });
}

fn project_card_content(ui: &mut egui::Ui) {
    {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::SEAL_CHECK).color(GREEN).size(16.0));
            ui.label(RichText::new("Open source").strong());
        });
        ui.add(
            egui::Label::new(
                RichText::new("Free software by the MirraNET organization, MIT licensed.")
                    .small()
                    .weak(),
            )
            .wrap(),
        );
        ui.add_space(6.0);
        let width = ui.available_width();
        if ui
            .add(
                egui::Button::new(format!("{}  View on GitHub", icons::GITHUB_LOGO))
                    .min_size(egui::vec2(width, 28.0)),
            )
            .on_hover_text(GITHUB_URL)
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(GITHUB_URL));
        }
        if ui
            .add(
                egui::Button::new(format!("{}  MirraNET.de", icons::GLOBE))
                    .min_size(egui::vec2(width, 28.0)),
            )
            .on_hover_text(MIRRANET_URL)
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(MIRRANET_URL));
        }
    }
}

/// Square icon button for the header toolbar.
fn header_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(icon).size(16.0)).min_size(egui::vec2(34.0, 32.0)))
        .on_hover_text(tooltip)
}
