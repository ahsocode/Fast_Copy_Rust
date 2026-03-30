use egui::{Color32, RichText, Visuals};
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::{CopyEngine, CopyJob, CopyMode, CopyProgress};

// ─── Browser types ────────────────────────────────────────────────────────────

struct BrowserEntry {
    name: String,
    path: PathBuf,
    size: u64,
    is_dir: bool,
    checked: bool,
}

struct BrowserState {
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    /// Deferred navigation target — set during iteration, applied after borrow ends.
    navigate_to: Option<PathBuf>,
    err: Option<String>,
}

impl BrowserState {
    fn new(start: PathBuf) -> Self {
        let mut s = BrowserState {
            current_dir: start.clone(),
            entries: Vec::new(),
            navigate_to: None,
            err: None,
        };
        s.load_dir(&start.clone());
        s
    }

    fn load_dir(&mut self, path: &PathBuf) {
        self.err = None;
        self.entries.clear();
        self.current_dir = path.clone();
        let rd = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(e) => {
                self.err = Some(e.to_string());
                return;
            }
        };

        let mut entries: Vec<BrowserEntry> = rd
            .filter_map(|r| r.ok())
            .map(|de| {
                let name = de.file_name().to_string_lossy().to_string();
                let path = de.path();
                let meta = de.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = if is_dir {
                    0
                } else {
                    meta.map(|m| m.len()).unwrap_or(0)
                };
                BrowserEntry { name, path, size, is_dir, checked: false }
            })
            .collect();

        // Folders first, then alphabetical
        entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
    }
}

// ─── App struct ───────────────────────────────────────────────────────────────

pub struct FastCopyApp {
    sources: Vec<PathBuf>,
    destination: String,
    mode: CopyMode,
    engine: Option<Arc<CopyEngine>>,
    progress_rx: Option<crossbeam_channel::Receiver<CopyProgress>>,
    progress: CopyProgress,
    last_pct: f32,
    copy_running: bool,
    selected_source: Option<usize>,
    show_error_window: bool,
    status_message: String,
    browser_open: bool,
    browser: BrowserState,
}

// ─── Catppuccin Mocha palette ────────────────────────────────────────────────

const BASE: Color32 = Color32::from_rgb(30, 30, 46);
const MANTLE: Color32 = Color32::from_rgb(24, 24, 37);
const SURFACE0: Color32 = Color32::from_rgb(49, 50, 68);
const SURFACE1: Color32 = Color32::from_rgb(69, 71, 90);
const TEXT: Color32 = Color32::from_rgb(205, 214, 244);
const SUBTEXT: Color32 = Color32::from_rgb(166, 173, 200);
const BLUE: Color32 = Color32::from_rgb(137, 180, 250);
const RED: Color32 = Color32::from_rgb(243, 139, 168);
const GREEN: Color32 = Color32::from_rgb(166, 227, 161);
const YELLOW: Color32 = Color32::from_rgb(249, 226, 175);
const OVERLAY0: Color32 = Color32::from_rgb(108, 112, 134);

// ─── impl ─────────────────────────────────────────────────────────────────────

impl FastCopyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Configure dark Catppuccin Mocha visuals
        let mut visuals = Visuals::dark();
        visuals.panel_fill = BASE;
        visuals.window_fill = BASE;
        visuals.extreme_bg_color = MANTLE;
        visuals.faint_bg_color = SURFACE0;
        visuals.widgets.noninteractive.bg_fill = SURFACE0;
        visuals.widgets.inactive.bg_fill = SURFACE0;
        visuals.widgets.hovered.bg_fill = SURFACE1;
        visuals.widgets.active.bg_fill = SURFACE1;
        visuals.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0, TEXT);
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
        visuals.override_text_color = Some(TEXT);
        cc.egui_ctx.set_visuals(visuals);

        // Slightly larger default font
        let mut style = (*cc.egui_ctx.style()).clone();
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        cc.egui_ctx.set_style(style);

        let home = dirs_home();
        FastCopyApp {
            sources: Vec::new(),
            destination: String::new(),
            mode: CopyMode::Auto,
            engine: None,
            progress_rx: None,
            progress: CopyProgress::default(),
            last_pct: 0.0,
            copy_running: false,
            selected_source: None,
            show_error_window: false,
            status_message: String::new(),
            browser_open: false,
            browser: BrowserState::new(home),
        }
    }

    fn add_source(&mut self, path: PathBuf) {
        if !self.sources.contains(&path) {
            self.sources.push(path);
        }
    }

    fn start_copy(&mut self) {
        if self.sources.is_empty() || self.destination.is_empty() || self.copy_running {
            return;
        }

        let job = CopyJob {
            sources: self.sources.clone(),
            destination: std::path::PathBuf::from(&self.destination),
            mode: self.mode.clone(),
        };

        let engine = Arc::new(CopyEngine::new());
        let rx = engine.start(job);
        self.engine = Some(engine);
        self.progress_rx = Some(rx);
        self.progress = CopyProgress::default();
        self.last_pct = 0.0;
        self.copy_running = true;
        self.status_message = "Copying…".to_string();
        self.show_error_window = false;
    }

    fn cancel_copy(&mut self) {
        if let Some(eng) = &self.engine {
            eng.cancel();
        }
    }
}

// ─── eframe::App impl ─────────────────────────────────────────────────────────

impl eframe::App for FastCopyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Poll progress receiver
        if let Some(rx) = &self.progress_rx {
            while let Ok(p) = rx.try_recv() {
                if p.finished {
                    self.copy_running = false;
                    self.status_message = if p.errors.is_empty() {
                        format!(
                            "Done — {} files, {}",
                            p.files_done,
                            fmt_size(p.bytes_done)
                        )
                    } else {
                        format!("Done with {} error(s)", p.errors.len())
                    };
                    if !p.errors.is_empty() {
                        self.show_error_window = true;
                    }
                    self.progress = p;
                } else if p.cancelled {
                    self.copy_running = false;
                    self.status_message = "Cancelled.".to_string();
                    self.progress = p;
                } else {
                    self.progress = p;
                }
            }
            // Keep repainting while running
            if self.copy_running {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }

        // 2. Handle drag & drop
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                self.add_source(path);
            }
        }

        // ── Top panel: mode radio buttons ────────────────────────────────────
        egui::TopBottomPanel::top("mode_panel")
            .frame(
                egui::Frame::default()
                    .fill(MANTLE)
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Mode:").color(SUBTEXT));
                    ui.add_space(8.0);
                    ui.radio_value(
                        &mut self.mode,
                        CopyMode::Auto,
                        RichText::new("Auto").color(TEXT),
                    );
                    ui.radio_value(
                        &mut self.mode,
                        CopyMode::Large,
                        RichText::new("Large File").color(TEXT),
                    );
                    ui.radio_value(
                        &mut self.mode,
                        CopyMode::Small,
                        RichText::new("Many Files").color(TEXT),
                    );
                });
            });

        // ── Bottom panel: progress + controls ────────────────────────────────
        egui::TopBottomPanel::bottom("bottom_panel")
            .frame(
                egui::Frame::default()
                    .fill(MANTLE)
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
            )
            .show(ctx, |ui| {
                // Progress bar with monotonic guard
                let pct = if self.progress.bytes_total > 0 {
                    (self.progress.bytes_done as f32
                        / self.progress.bytes_total as f32)
                        .min(1.0)
                } else {
                    0.0
                };
                if pct > self.last_pct {
                    self.last_pct = pct;
                }
                let bar =
                    egui::ProgressBar::new(self.last_pct).desired_width(f32::INFINITY);
                ui.add(bar);

                ui.add_space(6.0);

                // Stats row
                ui.horizontal(|ui| {
                    let speed_str = fmt_speed(self.progress.speed_bps);
                    let size_str = format!(
                        "{} / {}",
                        fmt_size(self.progress.bytes_done),
                        fmt_size(self.progress.bytes_total)
                    );
                    ui.label(
                        RichText::new(format!("{}  |  {}", speed_str, size_str))
                            .color(SUBTEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let elapsed = fmt_time(self.progress.elapsed_secs);
                        let eta = fmt_time(self.progress.eta_secs);
                        ui.label(
                            RichText::new(format!("Elapsed: {}  ETA: {}", elapsed, eta))
                                .color(SUBTEXT),
                        );
                    });
                });

                ui.add_space(2.0);

                // Current file
                if !self.progress.current_file.is_empty() {
                    let truncated = truncate_str(&self.progress.current_file, 60);
                    ui.label(
                        RichText::new(format!("  {}", truncated))
                            .color(OVERLAY0)
                            .size(12.5),
                    );
                    ui.add_space(2.0);
                }

                // Files counter
                if self.progress.files_total > 0 {
                    ui.label(
                        RichText::new(format!(
                            "Files: {} / {}",
                            self.progress.files_done, self.progress.files_total
                        ))
                        .color(SUBTEXT)
                        .size(12.5),
                    );
                    ui.add_space(2.0);
                }

                // Status message
                if !self.status_message.is_empty() {
                    let color = if self.status_message.contains("error") {
                        RED
                    } else if self.status_message.starts_with("Done") {
                        GREEN
                    } else {
                        YELLOW
                    };
                    ui.label(RichText::new(&self.status_message).color(color).size(13.0));
                    ui.add_space(4.0);
                }

                // Buttons row
                ui.horizontal(|ui| {
                    let can_start = !self.copy_running
                        && !self.sources.is_empty()
                        && !self.destination.is_empty();

                    // START COPY button
                    let start_btn = egui::Button::new(
                        RichText::new("  ▶  START COPY  ")
                            .color(MANTLE)
                            .strong(),
                    )
                    .fill(BLUE)
                    .min_size(egui::vec2(140.0, 32.0));

                    let resp = ui.add_enabled(can_start, start_btn);
                    if resp.clicked() {
                        self.start_copy();
                    }

                    ui.add_space(10.0);

                    // CANCEL button
                    let cancel_btn = egui::Button::new(
                        RichText::new("  ✕  CANCEL  ")
                            .color(MANTLE)
                            .strong(),
                    )
                    .fill(RED)
                    .min_size(egui::vec2(110.0, 32.0));

                    let resp = ui.add_enabled(self.copy_running, cancel_btn);
                    if resp.clicked() {
                        self.cancel_copy();
                    }
                });
            });

        // ── Central panel: sources (left) + destination (right) ───────────────
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(BASE)
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    // ── Left: sources ────────────────────────────────────────
                    let ui = &mut cols[0];
                    ui.label(RichText::new("Sources").color(SUBTEXT).strong());
                    ui.add_space(4.0);

                    egui::ScrollArea::vertical()
                        .id_source("sources_scroll")
                        .max_height(ui.available_height() - 80.0)
                        .show(ui, |ui: &mut egui::Ui| {
                            for (i, src) in self.sources.iter().enumerate() {
                                let label = src
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| src.to_string_lossy().to_string());
                                let full = src.to_string_lossy().to_string();
                                let is_selected = self.selected_source == Some(i);
                                let rt = if is_selected {
                                    RichText::new(&label).color(BLUE).strong()
                                } else {
                                    RichText::new(&label).color(TEXT)
                                };
                                let resp = ui
                                    .selectable_label(is_selected, rt)
                                    .on_hover_text(&full);
                                if resp.clicked() {
                                    self.selected_source =
                                        if is_selected { None } else { Some(i) };
                                }
                            }
                        });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        // Add Files
                        if ui
                            .add(
                                egui::Button::new(RichText::new("+ Files").color(TEXT))
                                    .fill(SURFACE0),
                            )
                            .clicked()
                        {
                            if let Some(paths) = rfd::FileDialog::new().pick_files() {
                                for p in paths {
                                    self.add_source(p);
                                }
                            }
                        }

                        // Add Folder
                        if ui
                            .add(
                                egui::Button::new(RichText::new("+ Folder").color(TEXT))
                                    .fill(SURFACE0),
                            )
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.add_source(path);
                            }
                        }

                        // Browse & Select
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Browse & Select…").color(MANTLE).strong(),
                                )
                                .fill(GREEN),
                            )
                            .clicked()
                        {
                            self.browser_open = true;
                        }

                        // Remove Selected
                        let can_remove = self.selected_source.is_some();
                        let remove_btn = egui::Button::new(
                            RichText::new("✕ Remove").color(if can_remove { RED } else { OVERLAY0 }),
                        )
                        .fill(SURFACE0);
                        if ui.add_enabled(can_remove, remove_btn).clicked() {
                            if let Some(idx) = self.selected_source.take() {
                                if idx < self.sources.len() {
                                    self.sources.remove(idx);
                                }
                            }
                        }
                    });

                    // ── Right: destination ───────────────────────────────────
                    let ui = &mut cols[1];
                    ui.label(RichText::new("Destination").color(SUBTEXT).strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        let text_edit = egui::TextEdit::singleline(&mut self.destination)
                            .hint_text("Choose destination folder…")
                            .desired_width(ui.available_width() - 72.0)
                            .text_color(TEXT);
                        ui.add(text_edit);

                        if ui
                            .add(
                                egui::Button::new(RichText::new("Browse").color(TEXT))
                                    .fill(SURFACE0),
                            )
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                self.destination = path.to_string_lossy().to_string();
                            }
                        }
                    });

                    ui.add_space(8.0);
                    if !self.destination.is_empty() {
                        ui.label(
                            RichText::new(truncate_str(&self.destination, 50))
                                .color(OVERLAY0)
                                .size(12.0),
                        );
                    }
                });
            });

        // ── Browser window ────────────────────────────────────────────────────
        if self.browser_open {
            // Apply deferred navigation from previous frame
            if let Some(nav_path) = self.browser.navigate_to.take() {
                self.browser.load_dir(&nav_path);
            }

            let mut open = self.browser_open;
            let mut paths_to_add: Vec<PathBuf> = Vec::new();
            let mut do_close = false;

            egui::Window::new("Browse & Select Files / Folders")
                .open(&mut open)
                .resizable(true)
                .default_size([700.0, 480.0])
                .min_size([500.0, 340.0])
                .show(ctx, |ui| {
                    // Path bar
                    ui.horizontal(|ui| {
                        let up_btn = egui::Button::new(RichText::new("↑ Up").color(TEXT))
                            .fill(SURFACE0)
                            .min_size(egui::vec2(50.0, 24.0));
                        if ui.add(up_btn).clicked() {
                            let parent = self.browser.current_dir.parent()
                                .map(|p| p.to_path_buf());
                            if let Some(p) = parent {
                                if p != self.browser.current_dir {
                                    self.browser.navigate_to = Some(p);
                                }
                            }
                        }
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                truncate_str(&self.browser.current_dir.to_string_lossy(), 60)
                            )
                            .color(OVERLAY0)
                            .size(12.5),
                        );
                    });

                    if let Some(ref err) = self.browser.err.clone() {
                        ui.label(RichText::new(format!("Error: {}", err)).color(RED));
                    }

                    ui.add_space(4.0);

                    // File listing
                    let checked_count = self.browser.entries.iter().filter(|e| e.checked).count();
                    egui::ScrollArea::vertical()
                        .id_source("browser_scroll")
                        .max_height(ui.available_height() - 80.0)
                        .show(ui, |ui| {
                            let n = self.browser.entries.len();
                            for i in 0..n {
                                let entry = &self.browser.entries[i];
                                let name = entry.name.clone();
                                let is_dir = entry.is_dir;
                                let size_str = if is_dir {
                                    "Folder".to_string()
                                } else {
                                    fmt_size(entry.size)
                                };
                                let entry_path = entry.path.clone();

                                ui.horizontal(|ui| {
                                    // Checkbox
                                    let mut checked = self.browser.entries[i].checked;
                                    if ui.checkbox(&mut checked, "").changed() {
                                        self.browser.entries[i].checked = checked;
                                    }

                                    // Name — click folder to navigate
                                    let name_color = if is_dir { BLUE } else { TEXT };
                                    let name_rt = RichText::new(&name)
                                        .color(name_color)
                                        .size(13.5);
                                    let resp = ui.add(
                                        egui::Label::new(name_rt)
                                            .sense(egui::Sense::click()),
                                    );
                                    if resp.double_clicked() && is_dir {
                                        self.browser.navigate_to = Some(entry_path.clone());
                                    }
                                    resp.on_hover_text(entry_path.to_string_lossy().as_ref());

                                    // Size
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(&size_str)
                                                    .color(OVERLAY0)
                                                    .size(12.0),
                                            );
                                        },
                                    );
                                });
                                if i + 1 < n {
                                    ui.add(egui::Separator::default().spacing(1.0));
                                }
                            }
                        });

                    ui.add_space(4.0);

                    // Bulk controls
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("Select All").color(TEXT)).fill(SURFACE0))
                            .clicked()
                        {
                            for e in self.browser.entries.iter_mut() {
                                e.checked = true;
                            }
                        }
                        if ui
                            .add(egui::Button::new(RichText::new("Clear All").color(TEXT)).fill(SURFACE0))
                            .clicked()
                        {
                            for e in self.browser.entries.iter_mut() {
                                e.checked = false;
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Cancel
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("Cancel").color(TEXT))
                                        .fill(SURFACE0),
                                )
                                .clicked()
                            {
                                do_close = true;
                            }
                            ui.add_space(6.0);
                            // Add Selected
                            let label = if checked_count > 0 {
                                format!("Add {} Selected", checked_count)
                            } else {
                                "Add Selected".to_string()
                            };
                            let add_btn = egui::Button::new(
                                RichText::new(&label).color(MANTLE).strong(),
                            )
                            .fill(BLUE)
                            .min_size(egui::vec2(130.0, 28.0));
                            if ui.add_enabled(checked_count > 0, add_btn).clicked() {
                                for e in self.browser.entries.iter() {
                                    if e.checked {
                                        paths_to_add.push(e.path.clone());
                                    }
                                }
                                do_close = true;
                            }
                        });
                    });
                });

            // Add checked paths to sources
            for p in paths_to_add {
                self.add_source(p);
            }
            if do_close {
                open = false;
            }
            self.browser_open = open;
        }

        // ── Error window ──────────────────────────────────────────────────────
        if self.show_error_window && !self.progress.errors.is_empty() {
            let errors_clone = self.progress.errors.clone();
            let mut open = self.show_error_window;
            egui::Window::new("Copy Errors")
                .open(&mut open)
                .resizable(true)
                .default_size([500.0, 300.0])
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} error(s) occurred:",
                            errors_clone.len()
                        ))
                        .color(RED),
                    );
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (file, err) in &errors_clone {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(truncate_str(file, 40)).color(YELLOW),
                                );
                                ui.label(RichText::new(" — ").color(OVERLAY0));
                                ui.label(RichText::new(err).color(RED).size(12.5));
                            });
                            ui.separator();
                        }
                    });
                });
            self.show_error_window = open;
        }
    }
}

// ─── Format helpers ───────────────────────────────────────────────────────────

pub fn fmt_size(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if n >= TB {
        format!("{:.2} TB", n as f64 / TB as f64)
    } else if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.0} KB", n as f64 / KB as f64)
    } else {
        format!("{} B", n)
    }
}

pub fn fmt_speed(bps: f64) -> String {
    if bps < 1.0 {
        return "– /s".to_string();
    }
    format!("{}/s", fmt_size(bps as u64))
}

pub fn fmt_time(secs: f64) -> String {
    if secs <= 0.0 {
        return "–".to_string();
    }
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let half = max_chars / 2 - 1;
        let start: String = s.chars().take(half).collect();
        let end: String = s.chars().rev().take(half).collect();
        let end: String = end.chars().rev().collect();
        format!("{}…{}", start, end)
    }
}
