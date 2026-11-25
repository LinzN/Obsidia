//! Device tab: backups, apps, photos & media and the live console.

use super::*;
use crate::apps;
use crate::backup::{self, BackupInfo};
use crate::media;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Lines kept in the console.
const CONSOLE_LIMIT: usize = 5000;

#[derive(PartialEq, Clone, Copy)]
pub(super) enum DevicePage {
    Overview,
    Backups,
    Apps,
    Media,
    Console,
}

pub(super) struct AppsState {
    pub list: Option<Result<Vec<apps::App>, String>>,
    pub loading: bool,
}

pub(super) struct RestoreDialog {
    pub backup: BackupInfo,
    pub password: String,
}

pub(super) struct EncryptionDialog {
    pub enable: bool,
    pub password: String,
    pub repeat: String,
}

pub(super) struct Console {
    pub device_key: String,
    pub log: Option<media::SysLog>,
    pub lines: Arc<Mutex<VecDeque<String>>>,
    pub filter: String,
    pub paused: Option<Vec<String>>,
}

impl App {
    pub(super) fn device_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        section_label(ui, tr("DEVICE"));
        let pages = [
            (DevicePage::Overview, tr("\u{e1e0}  Overview")),
            (DevicePage::Backups, tr("\u{e248}  Backups")),
            (DevicePage::Apps, tr("\u{e464}  Apps")),
            (DevicePage::Media, tr("\u{e2ca}  Photos & Media")),
            (DevicePage::Console, tr("\u{eae8}  Console")),
        ];
        for (page, label) in pages {
            if nav_button(ui, label, self.device_page == page).clicked() {
                self.device_page = page;
                if page == DevicePage::Backups {
                    self.load_backups();
                }
            }
        }
        const CARD_HEIGHT: f32 = 212.0;
        let free = ui.available_height() - CARD_HEIGHT;
        if free > 0.0 {
            ui.add_space(free);
        }
        project_card(ui);
        ui.add_space(6.0);
        self.version_status(ui);
    }

    pub(super) fn device_content(&mut self, ui: &mut egui::Ui) {
        match self.device_page {
            DevicePage::Overview => {
                self.device_tab(ui);
            }
            DevicePage::Backups => self.backups_page(ui),
            DevicePage::Apps => self.apps_page(ui),
            DevicePage::Media => self.media_page(ui),
            DevicePage::Console => self.console_page(ui),
        }
    }

    /// Selected device if it runs normally, otherwise shows a hint.
    fn normal_device(&self, ui: &mut egui::Ui) -> Option<Device> {
        match self.device() {
            Some(d) if d.mode == Mode::Normal => Some(d.clone()),
            Some(_) => {
                empty_state(
                    ui,
                    "\u{e1e0}",
                    tr("Device is in recovery or DFU mode"),
                    tr("This needs the device started normally and trusting this computer."),
                    None,
                );
                None
            }
            None => {
                empty_state(
                    ui,
                    "\u{e1e0}",
                    tr("No device connected"),
                    &trf(
                        "Connect an iPhone or iPad via USB and {}.",
                        &[&tr(device::no_device_hint())],
                    ),
                    self.scan_error.as_deref(),
                );
                None
            }
        }
    }

    // ----- Backups -------------------------------------------------------

    pub(super) fn load_backups(&mut self) {
        if self.backups_loading {
            return;
        }
        self.backups_loading = true;
        self.spawn(|out| {
            let list = backup::list(&device::default_backup_dir());
            out.send(Msg::Backups(list));
        });
    }

    fn backups_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            tr("Backups"),
            tr("Back up a device, restore a backup or manage the backups on this computer."),
        );
        if self.backups.is_none() && !self.backups_loading {
            self.load_backups();
        }
        let dev = self.device().filter(|d| d.mode == Mode::Normal).cloned();
        let busy = self.busy();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    dev.is_some() && !busy,
                    theme::accent_button(
                        tr("\u{e248}  Back up now"),
                        ACCENT,
                        dev.is_some() && !busy,
                    ),
                )
                .on_disabled_hover_text(tr("Connect a device that is started normally"))
                .clicked()
            {
                if let Some(d) = dev.clone() {
                    self.start_backup(d);
                }
            }
            if ui.button(tr("\u{e256}  Open folder")).clicked() {
                let dir = device::default_backup_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
            if ui
                .add_enabled(
                    !self.backups_loading,
                    egui::Button::new(tr("\u{e094}  Refresh")),
                )
                .clicked()
            {
                self.load_backups();
            }
            if self.backups_loading {
                ui.spinner();
            }
        });

        // backup encryption of the connected device
        if let Some(d) = &dev {
            let key = d.key().to_string();
            if !self.encryption.contains_key(&key) {
                self.encryption.insert(key.clone(), None);
                let d2 = d.clone();
                self.spawn(move |out| {
                    out.send(Msg::Encryption(key, backup::encryption_enabled(&d2)));
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                match self.encryption.get(d.key()).copied().flatten() {
                    Some(true) => {
                        ui.label(RichText::new(tr("\u{e2fe}  Backups of this device are encrypted")).color(GREEN));
                        if ui.add_enabled(!busy, egui::Button::new(tr("Turn off"))).clicked() {
                            self.enc_dialog = Some(EncryptionDialog {
                                enable: false,
                                password: String::new(),
                                repeat: String::new(),
                            });
                        }
                    }
                    Some(false) => {
                        ui.label(
                            RichText::new(tr("Backups are not encrypted - passwords, Health and Wi-Fi data are left out."))
                                .weak(),
                        );
                        if ui.add_enabled(!busy, egui::Button::new(tr("Encrypt backups"))).clicked() {
                            self.enc_dialog = Some(EncryptionDialog {
                                enable: true,
                                password: String::new(),
                                repeat: String::new(),
                            });
                        }
                    }
                    None => {
                        ui.label(RichText::new(tr("Checking backup encryption ...")).weak());
                    }
                }
            });
        }
        ui.add_space(10.0);

        let Some(list) = self.backups.clone() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Reading backups ..."));
            });
            return;
        };
        if list.is_empty() {
            empty_state(
                ui,
                "\u{e248}",
                tr("No backups yet"),
                &trf(
                    "Backups are stored in {}.",
                    &[&device::default_backup_dir().display()],
                ),
                None,
            );
            return;
        }

        let mut restore = None;
        let mut delete = None;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::remainder().at_least(160.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(130.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(220.0))
            .header(26.0, |mut h| {
                for t in [tr("Device"), "iOS", tr("Date (UTC)"), tr("Size"), ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).weak());
                    });
                }
            })
            .body(|body| {
                body.rows(40.0, list.len(), |mut row| {
                    let b = &list[row.index()];
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            let lock = if b.encrypted { "\u{e2fe} " } else { "" };
                            ui.label(format!("{lock}{}", b.device_name));
                            ui.label(
                                RichText::new(format!("{}  {}", b.product, b.serial))
                                    .small()
                                    .weak(),
                            );
                        });
                    });
                    row.col(|ui| {
                        ui.label(&b.version);
                    });
                    row.col(|ui| {
                        ui.label(&b.date);
                    });
                    row.col(|ui| {
                        ui.label(HumanBytes(b.size).to_string());
                    });
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    dev.is_some() && !busy,
                                    egui::Button::new(tr("\u{e08a}  Restore")),
                                )
                                .on_disabled_hover_text(tr("Connect the target device"))
                                .clicked()
                            {
                                restore = Some(b.clone());
                            }
                            if ui
                                .button("\u{e256}")
                                .on_hover_text(tr("Show in folder"))
                                .clicked()
                            {
                                open_path(&b.dir);
                            }
                            if ui
                                .add_enabled(!busy, egui::Button::new("\u{e4a6}"))
                                .on_hover_text(tr("Delete backup"))
                                .clicked()
                            {
                                delete = Some(b.clone());
                            }
                        });
                    });
                });
            });
        if let Some(b) = restore {
            self.backup_restore = Some(RestoreDialog {
                backup: b,
                password: String::new(),
            });
        }
        if let Some(b) = delete {
            self.confirm_delete_backup = Some(b);
        }
    }

    pub(super) fn backup_dialogs(&mut self, ctx: &egui::Context) {
        // restore
        let mut start = None;
        let mut close = false;
        if let Some(dlg) = &mut self.backup_restore {
            let target = self
                .devices
                .iter()
                .find(|d| Some(d.key()) == self.selected_device.as_deref())
                .cloned();
            egui::Modal::new(egui::Id::unique("backup_restore"))
                .backdrop_color(Color32::from_black_alpha(140))
                .frame(theme::surface_frame(Surface::Raised).inner_margin(22))
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.heading(tr("Restore backup"));
                    ui.add_space(8.0);
                    ui.label(format!(
                        "Backup of {} ({}, iOS {}) from {}",
                        dlg.backup.device_name, dlg.backup.product, dlg.backup.version, dlg.backup.date
                    ));
                    if let Some(t) = &target {
                        ui.label(format!(
                            "Target: {} {} {}",
                            t.kind(),
                            t.product,
                            t.serial.clone().unwrap_or_default()
                        ));
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            tr("\u{e4e0} Apps, data and settings on the device are replaced by the backup. Turn off \
                             \"Find My\" on the device first. The device restarts when the restore is done."),
                        )
                        .color(ORANGE),
                    );
                    if dlg.backup.encrypted {
                        ui.add_space(8.0);
                        ui.label(tr("Backup password"));
                        ui.add(egui::TextEdit::singleline(&mut dlg.password).password(true));
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            close = true;
                        }
                        let ok = target.is_some() && (!dlg.backup.encrypted || !dlg.password.is_empty());
                        if ui
                            .add_enabled(ok, theme::accent_button(tr("Restore"), DANGER, ok))
                            .clicked()
                        {
                            start = target.clone().map(|t| (t, dlg.backup.clone(), dlg.password.clone()));
                        }
                    });
                });
        }
        if close {
            self.backup_restore = None;
        }
        if let Some((target, b, pw)) = start {
            self.backup_restore = None;
            self.start_job(JobKind::Other, tr("Restore backup"), move |out| {
                let cmd =
                    backup::restore_cmd(&target, &b, Some(&pw)).map_err(|e| format!("{e:#}"))?;
                let _lock = device::exclusive();
                let o = out.clone();
                device::run_streaming(cmd, move |l| {
                    if l.contains('%') {
                        o.send(Msg::JobStatus(format!("Restoring backup ... {}", l.trim())));
                    }
                    o.send(Msg::Log(l));
                })
                .map_err(|e| format!("{e:#}"))?;
                Ok(tr("Backup restored - the device restarts now.").into())
            });
        }

        // delete
        let mut confirm = None;
        if let Some(b) = &self.confirm_delete_backup {
            egui::Modal::new(egui::Id::unique("backup_delete"))
                .backdrop_color(Color32::from_black_alpha(140))
                .frame(theme::surface_frame(Surface::Raised).inner_margin(22))
                .show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.heading(tr("Delete backup?"));
                    ui.add_space(8.0);
                    ui.label(format!(
                        "The backup of {} from {} ({}) is removed from this computer.",
                        b.device_name,
                        b.date,
                        HumanBytes(b.size)
                    ));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            confirm = Some(false);
                        }
                        if ui
                            .add(theme::accent_button(tr("Delete"), DANGER, true))
                            .clicked()
                        {
                            confirm = Some(true);
                        }
                    });
                });
        }
        match confirm {
            Some(true) => {
                if let Some(b) = self.confirm_delete_backup.take() {
                    match backup::delete(&b) {
                        Ok(()) => self.log(format!("Deleted backup {}", b.dir.display())),
                        Err(e) => {
                            self.notice =
                                Some((format!("Could not delete the backup: {e:#}"), true))
                        }
                    }
                    self.load_backups();
                }
            }
            Some(false) => self.confirm_delete_backup = None,
            None => {}
        }

        // encryption
        let mut apply = None;
        let mut close = false;
        if let Some(dlg) = &mut self.enc_dialog {
            egui::Modal::new(egui::Id::unique("backup_encryption"))
                .backdrop_color(Color32::from_black_alpha(140))
                .frame(theme::surface_frame(Surface::Raised).inner_margin(22))
                .show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.heading(if dlg.enable { tr("Encrypt backups") } else { tr("Turn off encryption") });
                    ui.add_space(8.0);
                    if dlg.enable {
                        ui.label(
                            tr("Encrypted backups also contain saved passwords, Wi-Fi settings and Health data. \
                             Without the password the backup cannot be restored."),
                        );
                        ui.add_space(8.0);
                        ui.label(tr("New password"));
                        ui.add(egui::TextEdit::singleline(&mut dlg.password).password(true));
                        ui.label(tr("Repeat password"));
                        ui.add(egui::TextEdit::singleline(&mut dlg.repeat).password(true));
                    } else {
                        ui.label(tr("Enter the current backup password. The device may ask for its passcode."));
                        ui.add_space(8.0);
                        ui.add(egui::TextEdit::singleline(&mut dlg.password).password(true));
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            close = true;
                        }
                        let ok = !dlg.password.is_empty() && (!dlg.enable || dlg.password == dlg.repeat);
                        if ui.add_enabled(ok, theme::accent_button(tr("Apply"), ACCENT, ok)).clicked() {
                            apply = Some((dlg.enable, dlg.password.clone()));
                        }
                    });
                });
        }
        if close {
            self.enc_dialog = None;
        }
        if let (Some((enable, pw)), Some(dev)) = (apply, self.device().cloned()) {
            self.enc_dialog = None;
            self.encryption.remove(dev.key());
            self.start_job(JobKind::Other, tr("Backup encryption"), move |_| {
                backup::set_encryption(&dev, enable, &pw).map_err(|e| format!("{e:#}"))?;
                Ok(if enable {
                    tr("Backups of this device are now encrypted.").into()
                } else {
                    tr("Backup encryption turned off.").into()
                })
            });
        }
    }

    // ----- Apps ----------------------------------------------------------

    fn load_apps(&mut self, dev: &Device) {
        let key = dev.key().to_string();
        let state = self.apps.entry(key.clone()).or_insert(AppsState {
            list: None,
            loading: false,
        });
        if state.loading {
            return;
        }
        state.loading = true;
        let dev = dev.clone();
        self.spawn(move |out| {
            let res = apps::list(&dev).map_err(|e| format!("{e:#}"));
            out.send(Msg::Apps(key, res));
        });
    }

    fn apps_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, tr("Apps"), tr("Apps installed on the device."));
        let Some(dev) = self.normal_device(ui) else {
            return;
        };
        let busy = self.busy();
        if !self.apps.contains_key(dev.key()) && !busy {
            self.load_apps(&dev);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy,
                    theme::accent_button(tr("\u{e4c0}  Install .ipa"), ACCENT, !busy),
                )
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(tr("iOS app"), &["ipa"])
                    .pick_file()
                {
                    let d = dev.clone();
                    let name = file_name(&path);
                    self.start_job(JobKind::Other, &format!("Install {name}"), move |out| {
                        out.send(Msg::JobStatus(format!("Installing {name} ...")));
                        apps::install(&d, &path).map_err(|e| format!("{e:#}"))?;
                        Ok(trf("{} installed.", &[&name]))
                    });
                }
            }
            let loading = self.apps.get(dev.key()).is_some_and(|a| a.loading);
            if ui
                .add_enabled(
                    !loading && !busy,
                    egui::Button::new(tr("\u{e094}  Refresh")),
                )
                .clicked()
            {
                self.load_apps(&dev);
            }
            if loading {
                ui.spinner();
            }
            ui.add_space(12.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.apps_filter)
                    .hint_text(tr("\u{e30c}  Search apps"))
                    .desired_width(220.0),
            );
        });
        ui.add_space(10.0);

        let list = match self.apps.get(dev.key()).and_then(|a| a.list.clone()) {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Reading apps ..."));
                });
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("\u{e4e0}  {e}")).color(ORANGE));
                return;
            }
            Some(Ok(list)) => list,
        };
        let filter = self.apps_filter.to_lowercase();
        let shown: Vec<&apps::App> = list
            .iter()
            .filter(|a| {
                filter.is_empty()
                    || a.name.to_lowercase().contains(&filter)
                    || a.bundle_id.to_lowercase().contains(&filter)
            })
            .collect();
        let total: u64 = list.iter().filter_map(|a| a.size).sum();
        ui.label(
            RichText::new(trf(
                "{} apps, {} in total",
                &[&list.len(), &HumanBytes(total)],
            ))
            .weak()
            .small(),
        );
        ui.add_space(4.0);
        let mut remove = None;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::remainder().at_least(200.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(150.0))
            .header(26.0, |mut h| {
                for t in [tr("App"), tr("Version"), tr("Size"), ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).weak());
                    });
                }
            })
            .body(|body| {
                body.rows(40.0, shown.len(), |mut row| {
                    let a = shown[row.index()];
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label(&a.name);
                            ui.label(RichText::new(&a.bundle_id).small().weak());
                        });
                    });
                    row.col(|ui| {
                        ui.label(&a.version);
                    });
                    row.col(|ui| {
                        ui.label(
                            a.size
                                .map(|s| HumanBytes(s).to_string())
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new(tr("\u{e4a6}  Uninstall")))
                            .clicked()
                        {
                            remove = Some(a.clone());
                        }
                    });
                });
            });
        if let Some(a) = remove {
            self.confirm_uninstall = Some(a);
        }
    }

    pub(super) fn apps_dialog(&mut self, ctx: &egui::Context) {
        let Some(app) = &self.confirm_uninstall else {
            return;
        };
        let mut confirm = None;
        egui::Modal::new(egui::Id::unique("app_uninstall"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(theme::surface_frame(Surface::Raised).inner_margin(22))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading(trf("Uninstall {}?", &[&app.name]));
                ui.add_space(8.0);
                ui.label(tr("The app and its data are removed from the device."));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        confirm = Some(false);
                    }
                    if ui
                        .add(theme::accent_button(tr("Uninstall"), DANGER, true))
                        .clicked()
                    {
                        confirm = Some(true);
                    }
                });
            });
        match confirm {
            Some(true) => {
                let app = self.confirm_uninstall.take().unwrap();
                if let Some(dev) = self.device().cloned() {
                    self.start_job(
                        JobKind::Other,
                        &format!("Uninstall {}", app.name),
                        move |_| {
                            apps::uninstall(&dev, &app.bundle_id).map_err(|e| format!("{e:#}"))?;
                            Ok(trf("{} uninstalled.", &[&app.name]))
                        },
                    );
                }
            }
            Some(false) => self.confirm_uninstall = None,
            None => {}
        }
    }

    // ----- Photos & media ------------------------------------------------

    fn media_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            tr("Photos & Media"),
            tr("Copy photos and crash reports to this computer or take a screenshot."),
        );
        let Some(dev) = self.normal_device(ui) else {
            return;
        };
        let busy = self.busy();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(14.0, 14.0);
            if tool_tile(
                ui,
                "\u{e2ca}",
                tr("Export photos"),
                tr("Copy the camera roll"),
                !busy,
            ) {
                let d = dev.clone();
                self.start_job(JobKind::Other, tr("Export photos"), move |out| {
                    let dir = media::new_photo_dir(&d).map_err(|e| format!("{e:#}"))?;
                    // report progress from the growing target folder
                    let done = Arc::new(AtomicBool::new(false));
                    {
                        let (done, out, dir) = (done.clone(), out.clone(), dir.clone());
                        std::thread::spawn(move || {
                            while !done.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_secs(1));
                                let (n, size) = media::count_files(&dir);
                                out.send(Msg::JobStatus(format!(
                                    "Copying photos ... {n} files, {}",
                                    HumanBytes(size)
                                )));
                            }
                        });
                    }
                    let result = media::export_photos(&d, &dir);
                    done.store(true, Ordering::Relaxed);
                    result.map_err(|e| format!("{e:#}"))?;
                    let (n, size) = media::count_files(&dir);
                    open_path(&dir);
                    Ok(trf(
                        "{} files ({}) copied to {}",
                        &[&n, &HumanBytes(size), &dir.display()],
                    ))
                });
            }
            if tool_tile(
                ui,
                "\u{e10e}",
                tr("Screenshot"),
                tr("Capture the screen"),
                !busy,
            ) {
                let d = dev.clone();
                self.start_job(JobKind::Other, tr("Screenshot"), move |_| {
                    let path = media::screenshot(&d).map_err(|e| format!("{e:#}"))?;
                    open_path(&path);
                    Ok(trf("Screenshot saved to {}", &[&path.display()]))
                });
            }
            if tool_tile(
                ui,
                "\u{e4e0}",
                tr("Crash reports"),
                tr("Copy app and system crashes"),
                !busy,
            ) {
                let d = dev.clone();
                self.start_job(JobKind::Other, tr("Crash reports"), move |out| {
                    out.send(Msg::JobStatus(tr("Copying crash reports ...").into()));
                    let (dir, n) = media::crash_reports(&d).map_err(|e| format!("{e:#}"))?;
                    open_path(&dir);
                    Ok(trf("{} crash reports copied to {}", &[&n, &dir.display()]))
                });
            }
            if tool_tile(
                ui,
                "\u{e256}",
                tr("Open folder"),
                tr("Photos, screenshots, reports"),
                true,
            ) {
                let dir = firmware::data_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
        });
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                tr("Photos are copied in their original format (HEIC/MOV). Photos that are only stored in iCloud \
                 are not on the device and cannot be copied. Screenshots need Developer Mode and only work \
                 up to iOS 16."),
            )
            .weak()
            .small(),
        );
    }

    // ----- Console -------------------------------------------------------

    fn console_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, tr("Console"), tr("Live system log of the device."));
        let Some(dev) = self.normal_device(ui) else {
            return;
        };
        let running = self
            .console
            .as_ref()
            .is_some_and(|c| c.device_key == dev.key() && c.log.is_some());

        ui.horizontal(|ui| {
            if running {
                if ui.button(tr("\u{e46c}  Stop")).clicked() {
                    if let Some(c) = &mut self.console {
                        c.log = None;
                    }
                }
            } else if ui
                .add_enabled(
                    !self.busy(),
                    theme::accent_button(tr("\u{e3d0}  Start"), ACCENT, !self.busy()),
                )
                .clicked()
            {
                self.start_console(&dev);
            }
            if let Some(c) = &mut self.console {
                let paused = c.paused.is_some();
                if ui
                    .add_enabled(
                        running,
                        egui::Button::new(if paused {
                            tr("\u{e3d0}  Resume")
                        } else {
                            tr("\u{e39e}  Pause")
                        }),
                    )
                    .clicked()
                {
                    c.paused = if paused {
                        None
                    } else {
                        Some(
                            c.lines
                                .lock()
                                .map(|l| l.iter().cloned().collect())
                                .unwrap_or_default(),
                        )
                    };
                }
                if ui.button(tr("\u{e4a6}  Clear")).clicked() {
                    if let Ok(mut l) = c.lines.lock() {
                        l.clear();
                    }
                    c.paused = None;
                }
                if ui.button(tr("\u{eaf0}  Save")).clicked() {
                    let lines: Vec<String> = c
                        .lines
                        .lock()
                        .map(|l| l.iter().cloned().collect())
                        .unwrap_or_default();
                    match media::save_syslog(&dev, &lines) {
                        Ok(p) => {
                            self.notice = Some((format!("Log saved to {}", p.display()), false));
                            if let Some(d) = p.parent() {
                                open_path(d);
                            }
                        }
                        Err(e) => {
                            self.notice = Some((format!("Could not save the log: {e:#}"), true))
                        }
                    }
                    return;
                }
                ui.add_space(12.0);
                ui.add(
                    egui::TextEdit::singleline(&mut c.filter)
                        .hint_text(tr("\u{e30c}  Filter (text or process)"))
                        .desired_width(260.0),
                );
            }
        });
        ui.add_space(8.0);

        let Some(c) = &self.console else {
            ui.label(
                RichText::new(tr("Press Start to show the log of the connected device.")).weak(),
            );
            return;
        };
        let filter = c.filter.to_lowercase();
        let lines: Vec<String> = match &c.paused {
            Some(p) => p.clone(),
            None => c
                .lines
                .lock()
                .map(|l| l.iter().cloned().collect())
                .unwrap_or_default(),
        };
        let shown: Vec<&String> = lines
            .iter()
            .filter(|l| filter.is_empty() || l.to_lowercase().contains(&filter))
            .collect();
        theme::surface(ui, Surface::Card, |ui| {
            ui.set_width(ui.available_width());
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::both()
                .stick_to_bottom(c.paused.is_none())
                .auto_shrink([false, false])
                .show_rows(ui, row_h, shown.len(), |ui, range| {
                    for l in &shown[range] {
                        ui.label(RichText::new(l.as_str()).monospace().size(12.0));
                    }
                });
        });
    }

    fn start_console(&mut self, dev: &Device) {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let (sink, ctx) = (lines.clone(), self.out.ctx.clone());
        match media::SysLog::start(dev, move |line| {
            if let Ok(mut l) = sink.lock() {
                l.push_back(line);
                while l.len() > CONSOLE_LIMIT {
                    l.pop_front();
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }) {
            Ok(log) => {
                let filter = self.console.take().map(|c| c.filter).unwrap_or_default();
                self.console = Some(Console {
                    device_key: dev.key().to_string(),
                    log: Some(log),
                    lines,
                    filter,
                    paused: None,
                });
            }
            Err(e) => self.notice = Some((format!("Could not start the console: {e:#}"), true)),
        }
    }
}
