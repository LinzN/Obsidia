//! Desktop GUI built with egui.

use crate::device::{self, Device, Mode};
use crate::firmware::{self, CatalogEntry, DeviceInfo, Firmware, Progress};
use crate::theme::{self, Surface, ACCENT, DANGER, GREEN, ORANGE, RED};
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
    local_files: Vec<(PathBuf, u64)>,

    selection: Option<Selection>,
    erase: bool,

    job: Option<Job>,
    flash_view: Option<FlashView>,
    log: Vec<String>,
    show_log: bool,
    pending_flash: Option<FlashRequest>,
    notice: Option<(String, bool)>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::apply(ctx);
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
            selection: None,
            erase: true,
            job: None,
            flash_view: None,
            log: vec![format!("Obsidia {} started", env!("CARGO_PKG_VERSION"))],
            show_log: false,
            pending_flash: None,
            notice: None,
        };
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
        self.local_files = firmware::cache_dir()
            .ok()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| {
                let mut v: Vec<(PathBuf, u64)> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("ipsw"))
                    })
                    .map(|p| {
                        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                        (p, size)
                    })
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
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
        if self.last_scan.is_some_and(|t| t.elapsed() < SCAN_INTERVAL) {
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
            out.send(Msg::JobStatus("Preparing device ...".into()));
            let cmd =
                device::restore_cmd(&dev, &ipsw, erase, false).map_err(|e| format!("{e:#}"))?;
            let line_out = out.clone();
            device::run_streaming(cmd, move |l| line_out.send(Msg::Log(l)))
                .map_err(|e| format!("{e:#} - tip: put the device into DFU mode and try again"))?;
            Ok("Flashing completed successfully - the device will now reboot.".to_string())
        });
    }

    fn start_backup(&mut self, dev: Device) {
        self.start_job(JobKind::Other, "Backup", move |out| {
            let dir = device::default_backup_dir();
            let cmd = device::backup_cmd(&dev, &dir).map_err(|e| format!("{e:#}"))?;
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
            ui.label(RichText::new("Obsidia").size(21.0).strong());
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
                if ui.button("☰ Log").on_hover_text("Show the log").clicked() {
                    self.show_log = !self.show_log;
                }
                let active = self.downloads.len();
                let dl = if active > 0 {
                    format!("⬇ {active}")
                } else {
                    "⬇".to_string()
                };
                let resp = ui.button(dl).on_hover_text(if active > 0 {
                    format!("{active} download(s) running - open firmware folder")
                } else {
                    "Open the firmware folder".to_string()
                });
                if resp.clicked() {
                    if let Ok(dir) = firmware::cache_dir() {
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
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
                ui.separator();
                if ui.small_button("View log").clicked() {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        section_label(ui, "FLASH");
        if nav_button(ui, "⚡  Smart Flash", self.page == FlashPage::SmartFlash).clicked() {
            self.page = FlashPage::SmartFlash;
        }
        ui.add_space(12.0);
        section_label(ui, "FIRMWARE");
        if nav_button(
            ui,
            "⬇  Firmware Download",
            self.page == FlashPage::FirmwareDownload,
        )
        .clicked()
        {
            self.page = FlashPage::FirmwareDownload;
        }
        if nav_button(
            ui,
            "🗀  Local Firmware",
            self.page == FlashPage::LocalFirmware,
        )
        .clicked()
        {
            self.page = FlashPage::LocalFirmware;
            self.refresh_local_files();
        }
        ui.add_space(12.0);
        section_label(ui, "TOOLS");
        if nav_button(ui, "🧰  Toolbox", false).clicked() {
            self.tab = Tab::Toolbox;
        }
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
            if ui.button("⤓ Import Local Firmware").clicked() {
                self.import_local_firmware();
            }
            if ui.button("🗀 Open Folder").clicked() {
                if let Ok(dir) = firmware::cache_dir() {
                    open_path(&dir);
                }
            }
            if ui.button("⟳ Refresh").clicked() {
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
                    ui.label(RichText::new("◉").color(ACCENT));
                    ui.label(RichText::new("Local firmware:").weak());
                    ui.label(RichText::new(file_name(&path)).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖ Remove").clicked() {
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
                "📱",
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
                            .selectable_label(self.browse.is_none(), "📱 Connected device")
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
                "⬇",
                "Choose a model",
                "Select an iPhone or iPad model above.",
                None,
            ),
        }
    }

    fn local_firmware_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Local Firmware",
            "Firmware files downloaded by Obsidia.",
        );
        ui.horizontal(|ui| {
            if ui.button("⤓ Import Local Firmware").clicked() {
                self.import_local_firmware();
            }
            if ui.button("🗀 Open Folder").clicked() {
                if let Ok(dir) = firmware::cache_dir() {
                    open_path(&dir);
                }
            }
            if ui.button("⟳ Refresh").clicked() {
                self.refresh_local_files();
            }
        });
        ui.add_space(8.0);

        if self.local_files.is_empty() {
            empty_state(
                ui,
                "🗀",
                "No firmware downloaded yet",
                "Downloaded firmware files appear here.",
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
            .column(Column::remainder().at_least(300.0))
            .column(Column::exact(100.0))
            .column(Column::exact(230.0))
            .header(26.0, |mut h| {
                for t in ["File", "Size", ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).strong());
                    });
                }
            })
            .body(|mut body| {
                for (path, size) in &files {
                    body.row(36.0, |mut row| {
                        row.col(|ui| {
                            ui.label(file_name(path));
                        });
                        row.col(|ui| {
                            ui.label(HumanBytes(*size).to_string());
                        });
                        row.col(|ui| {
                            if ui.button("Use for flashing").clicked() {
                                select = Some(path.clone());
                            }
                            if ui.button("🗑 Delete").clicked() {
                                delete = Some(path.clone());
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
            match std::fs::remove_file(&p) {
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
                                    (ACCENT, "◉")
                                } else {
                                    (Color32::GRAY, "○")
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
                                ui.label(RichText::new("○ Not signed").weak());
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
                                            .small_button("✖")
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
                                ui.label(RichText::new("✔ Downloaded").color(GREEN));
                            } else if ui.button("⬇ Download").clicked() {
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
                        "✔",
                        egui::FontId::proportional(24.0),
                        Color32::WHITE,
                    );
                }
                Some(Err(_)) => {
                    ui.painter().circle_filled(badge, 22.0, DANGER);
                    ui.painter().text(
                        badge,
                        egui::Align2::CENTER_CENTER,
                        "✖",
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
                    ui.label(RichText::new("Congratulations!").size(18.0).strong());
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
                    ui.label(
                        RichText::new("Flashing failed")
                            .size(18.0)
                            .strong()
                            .color(RED),
                    );
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
        self.ensure_firmwares(&dev.product);
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
            ui.label(RichText::new(&model).size(17.0).strong());
        }
        ui.label(RichText::new(&dev.name).weak());
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
                "⚡  Flash"
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
                "📱",
                "No device connected",
                &format!(
                    "Connect an iPhone or iPad via USB and {}.",
                    device::no_device_hint()
                ),
                self.scan_error.as_deref(),
            );
            return;
        };
        self.ensure_firmwares(&dev.product);
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
                ui.label(RichText::new(&model).size(26.0).strong());
                ui.label(RichText::new(&dev.name).weak().size(15.0));
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
                            row("Product type", dev.product.clone());
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
                        .add_enabled(!busy && normal, egui::Button::new("💾  Backup"))
                        .clicked()
                    {
                        self.start_backup(dev.clone());
                    }
                    if ui
                        .add_enabled(!busy && normal, egui::Button::new("🔧  Enter recovery"))
                        .clicked()
                    {
                        self.start_enter_recovery(dev.clone());
                    }
                    if ui
                        .add_enabled(!busy && !normal, egui::Button::new("↩  Exit recovery"))
                        .clicked()
                    {
                        self.start_exit_recovery();
                    }
                    if ui
                        .add(theme::accent_button("⚡  Flash", ACCENT, true))
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
            if tool_tile(ui, "💾", "Backup", "Back up apps and data", !busy && normal) {
                if let Some(d) = dev.clone() {
                    self.start_backup(d);
                }
            }
            if tool_tile(
                ui,
                "🔧",
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
                "↩",
                "Exit Recovery",
                "Reboot into iOS / iPadOS",
                !busy && in_recovery,
            ) {
                self.start_exit_recovery();
            }
            if tool_tile(ui, "🗀", "Backups", "Open the backup folder", true) {
                let dir = device::default_backup_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
            if tool_tile(ui, "⬇", "Firmware", "Open the firmware folder", true) {
                if let Ok(dir) = firmware::cache_dir() {
                    open_path(&dir);
                }
            }
            if tool_tile(ui, "☰", "Log", "Show the activity log", true) {
                self.show_log = true;
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
                    RichText::new(format!("⚠ Required tools missing: {}", self.missing_tools.join(", ")))
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
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(self.log.join("\n"));
                    }
                    if ui.small_button("Clear").clicked() {
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
                            "⚠ All data on the device will be deleted. Create a backup first and make sure you know \
                             the Apple ID password (Activation Lock).",
                        )
                        .color(ORANGE),
                    );
                } else {
                    ui.label(RichText::new("Apps and data are kept. A backup is still recommended.").weak());
                }
                ui.label(RichText::new("Do not disconnect the device until it has rebooted.").weak());
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
        self.flash_dialog(&ctx);
    }
}

/// Selector-bar style tab: plain text, accent pill under the active tab.
fn tab_button(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let text = if active {
        RichText::new(label).size(15.0).strong()
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

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak().strong());
    ui.add_space(2.0);
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(20.0).strong());
    ui.label(RichText::new(subtitle).weak().small());
    ui.add_space(10.0);
}

fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, text: &str, error: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.2);
        ui.label(RichText::new(icon).size(46.0).weak());
        ui.add_space(6.0);
        ui.label(RichText::new(title).size(17.0).strong());
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
