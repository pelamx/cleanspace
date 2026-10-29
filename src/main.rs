#![cfg_attr(windows, windows_subsystem = "windows")] // no console window on Windows

use deskmonk::applock;
use deskmonk::diskmap::{self, Node};
use deskmonk::hardware::{self, Component};
use deskmonk::scan::{Category, Item, Phase, human_size};
use deskmonk::telemetry::{self, Telemetry};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke,
    Theme, Ui, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const ACCENT: Color32 = Color32::from_rgb(79, 110, 247);
const DANGER: Color32 = Color32::from_rgb(229, 72, 77);
const ROW_H: f32 = 46.0;
/// Size of the disk speed test file.
const SPEED_TEST: u64 = 1 << 30;
/// Terminal greens of the hardware view.
const TERM: Color32 = Color32::from_rgb(57, 255, 20);
const TERM_DIM: Color32 = Color32::from_rgb(31, 158, 69);
const TERM_BG: Color32 = Color32::from_rgb(7, 11, 8);
/// Box colors of the disk map, neon on the terminal background.
const NEON: [Color32; 8] = [
    TERM,
    Color32::from_rgb(0, 229, 255),
    Color32::from_rgb(255, 0, 170),
    Color32::from_rgb(255, 214, 0),
    Color32::from_rgb(255, 120, 0),
    Color32::from_rgb(170, 90, 255),
    Color32::from_rgb(60, 140, 255),
    Color32::from_rgb(255, 70, 70),
];

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("bundled icon is a valid PNG");
    // On macOS this is the Dock icon, which needs Apple's padded icon grid; elsewhere the full-bleed logo fits.
    let window_icon = if cfg!(target_os = "macos") {
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/dock-icon.png"))
            .expect("bundled Dock icon is a valid PNG")
    } else {
        icon.clone()
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("deskMonk by pelamx")
            // Wayland compositors take the icon from the .desktop file matching this id.
            .with_app_id("deskmonk")
            .with_icon(Arc::new(window_icon)),
        ..Default::default()
    };
    eframe::run_native(
        "DeskMonk",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            let mut app = App::default();
            let (tx, rx) = channel();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(hardware::detect());
                ctx.request_repaint();
            });
            app.hw_rx = Some(rx);
            if cfg!(target_os = "macos") {
                let (tx, rx) = channel();
                let ctx = cc.egui_ctx.clone();
                app.watcher = Some(applock::Watcher::start(app.lock.apps.clone(), move |blocked| {
                    let _ = tx.send(blocked);
                    ctx.request_repaint();
                }));
                app.blocked = Some(rx);
            }
            let logo = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            app.logo = Some(
                cc.egui_ctx
                    .load_texture("logo", logo, egui::TextureOptions::LINEAR),
            );
            // `deskmonk FOLDER` scans that folder right away.
            if let Some(dir) = std::env::args().nth(1) {
                app.root = dir;
                app.autostart = true;
            }
            Ok(Box::new(app))
        }),
    )
}

/// Palette per theme: (window bg, sidebar/card bg, border, muted text).
fn palette(dark: bool) -> (Color32, Color32, Color32, Color32) {
    if dark {
        (
            Color32::from_rgb(20, 21, 26),
            Color32::from_rgb(29, 31, 38),
            Color32::from_rgb(44, 47, 58),
            Color32::from_rgb(140, 145, 160),
        )
    } else {
        (
            Color32::from_rgb(246, 247, 250),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(226, 229, 236),
            Color32::from_rgb(110, 115, 130),
        )
    }
}

fn setup_style(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let dark = theme == Theme::Dark;
        let (bg, card, border, _) = palette(dark);
        ctx.style_mut_of(theme, |style| {
            use egui::{FontFamily::Proportional, TextStyle};
            style.text_styles = [
                (TextStyle::Heading, FontId::new(22.0, Proportional)),
                (TextStyle::Body, FontId::new(14.5, Proportional)),
                (TextStyle::Button, FontId::new(14.5, Proportional)),
                (TextStyle::Small, FontId::new(12.0, Proportional)),
                (
                    TextStyle::Monospace,
                    FontId::new(13.5, egui::FontFamily::Monospace),
                ),
            ]
            .into();
            style.spacing.item_spacing = vec2(8.0, 8.0);
            style.spacing.button_padding = vec2(14.0, 7.0);
            style.spacing.interact_size.y = 32.0;
            let v = &mut style.visuals;
            v.panel_fill = bg;
            v.window_fill = card;
            v.extreme_bg_color = if dark {
                Color32::from_rgb(16, 17, 21)
            } else {
                Color32::WHITE
            };
            v.faint_bg_color = card;
            v.window_corner_radius = CornerRadius::same(14);
            v.window_stroke = Stroke::new(1.0, border);
            v.selection.bg_fill = ACCENT;
            v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
            v.hyperlink_color = ACCENT;
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.noninteractive,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CornerRadius::same(8);
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
            v.widgets.inactive.weak_bg_fill = if dark {
                Color32::from_rgb(40, 43, 53)
            } else {
                Color32::from_rgb(236, 238, 243)
            };
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.hovered.weak_bg_fill = if dark {
                Color32::from_rgb(50, 54, 66)
            } else {
                Color32::from_rgb(226, 229, 237)
            };
            v.widgets.hovered.bg_stroke = Stroke::NONE;
        });
    }
}

fn icon(cat: Category) -> &'static str {
    match cat {
        Category::Duplicate => "📋",
        Category::Junk => "🗑",
        Category::OldDownload => "⬇",
        Category::Large => "📦",
    }
}

fn description(cat: Category) -> &'static str {
    match cat {
        Category::Duplicate => {
            "İçeriği birebir aynı dosyalar. Her grubun ilk kopyası korunur, fazlası seçilir."
        }
        Category::Junk => {
            "Uygulama önbellekleri, çöp kutusu, geçici dosyalar, yedekler ve loglar. Önbellekler gerektiğinde yeniden oluşturulur."
        }
        Category::OldDownload => {
            "İndirilenler klasöründe 90 günden uzun süredir değişmemiş dosyalar."
        }
        Category::Large => {
            "500 MB ve üzeri dosyalar. Gerçekten gerekli olup olmadığını kontrol edin."
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Clean,
    Hardware,
    Map,
    Lock,
}

enum Row {
    Group(usize),
    Item(usize),
}

struct App {
    root: String,
    items: Vec<Item>,
    tab: Category,
    /// Messages are (phase, items): partial lists without duplicates, then the final one with phase None.
    scanning: Option<(Receiver<(Option<Phase>, Vec<Item>)>, Arc<AtomicUsize>)>,
    /// Phase of the partial results on screen.
    phase: Option<Phase>,
    scanned_files: usize,
    has_scanned: bool,
    confirm: bool,
    deleting: Option<deskmonk::Deletion>,
    status: String,
    telemetry: Telemetry,
    autostart: bool,
    logo: Option<egui::TextureHandle>,
    page: Page,
    /// When the hardware view was opened, for its typing animation.
    hw_since: f64,
    hardware: Option<Vec<Component>>,
    hw_rx: Option<Receiver<Vec<Component>>>,
    hw_sent: bool,
    /// Folder tree of `map_root`, and the child indexes of the folder being looked at.
    map: Option<Node>,
    map_root: PathBuf,
    map_path: Vec<usize>,
    map_scan: Option<(Receiver<Node>, Arc<AtomicUsize>)>,
    speed: Option<Result<deskmonk::speedtest::Speed, String>>,
    /// Running speed test and its bytes done (write then read, up to 2 × SPEED_TEST).
    speed_run: Option<(Receiver<std::io::Result<deskmonk::speedtest::Speed>>, Arc<AtomicU64>)>,
    lock: applock::Config,
    watcher: Option<applock::Watcher>,
    /// Locked apps that were stopped and wait for the password.
    blocked: Option<Receiver<PathBuf>>,
    /// The app the password prompt is for.
    lock_prompt: Option<PathBuf>,
    /// Typed password (prompt, settings unlock or new password) and its confirmation.
    lock_pw: String,
    lock_pw2: String,
    lock_error: String,
    /// The lock settings were opened with the password; closes again when leaving the page.
    lock_open: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().display().to_string(),
            items: Vec::new(),
            tab: Category::Duplicate,
            scanning: None,
            phase: None,
            scanned_files: 0,
            has_scanned: false,
            confirm: false,
            deleting: None,
            status: String::new(),
            telemetry: Telemetry::load(),
            autostart: false,
            logo: None,
            page: Page::Clean,
            hw_since: 0.0,
            hardware: None,
            hw_rx: None,
            hw_sent: false,
            map: None,
            map_root: PathBuf::new(),
            map_path: Vec::new(),
            map_scan: None,
            speed: None,
            speed_run: None,
            lock: applock::Config::load(),
            watcher: None,
            blocked: None,
            lock_prompt: None,
            lock_pw: String::new(),
            lock_pw2: String::new(),
            lock_error: String::new(),
            lock_open: false,
        }
    }
}

impl App {
    fn start_scan(&mut self, ctx: egui::Context) {
        let root = PathBuf::from(self.root.trim());
        if !root.is_dir() {
            self.status = format!("Klasör bulunamadı: {}", root.display());
            return;
        }
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let counter = progress.clone();
        std::thread::spawn(move || {
            let (partial_tx, partial_ctx) = (tx.clone(), ctx.clone());
            let items = deskmonk::scan_default(&root, &counter, move |items, phase| {
                let _ = partial_tx.send((Some(phase), items));
                partial_ctx.request_repaint();
            });
            let _ = tx.send((None, items));
            ctx.request_repaint();
        });
        self.items.clear();
        self.status.clear();
        self.phase = None;
        self.scanning = Some((rx, progress));
    }

    fn start_map(&mut self, ctx: egui::Context) {
        let root = PathBuf::from(self.root.trim());
        if !root.is_dir() {
            self.status = format!("Klasör bulunamadı: {}", root.display());
            return;
        }
        let files = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let counter = files.clone();
        self.map_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(diskmap::build(&root, &counter));
            ctx.request_repaint();
        });
        self.map_scan = Some((rx, files));
    }

    fn delete_selected(&mut self, ctx: egui::Context) {
        self.deleting = Some(deskmonk::Deletion::start(
            &mut self.items,
            false,
            move || ctx.request_repaint(),
        ));
    }

    fn poll_deletion(&mut self) {
        let Some(done) = self.deleting.as_ref().and_then(|d| d.poll()) else {
            return;
        };
        let ok = self.deleting.take().unwrap().total - done.failed.len();
        self.telemetry.clean_done("gui", ok, done.freed);
        self.status = format!(
            "✔ {ok} öğe temizlendi, {} yer açıldı",
            human_size(done.freed)
        );
        if let Some(err) = done.first_error {
            self.status += &format!(" · {} öğe tamamen silinemedi: {err}", done.failed.len());
        }
        self.items.extend(done.failed);
    }

    fn totals(&self, cat: Option<Category>, only_selected: bool) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| cat.is_none_or(|c| i.category == c) && (!only_selected || i.selected))
            .fold((0, 0), |(n, s), i| (n + 1, s + i.size))
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        ui.add_space(18.0);
        ui.label(RichText::new("   KATEGORİLER").small().color(muted));
        ui.add_space(2.0);
        for cat in Category::ALL {
            let (n, size) = self.totals(Some(cat), false);
            let selected = self.page == Page::Clean && self.tab == cat;
            if nav_item(ui, selected, icon(cat), cat.label(), n, &human_size(size)).clicked() {
                self.tab = cat;
                self.page = Page::Clean;
            }
        }
        ui.add_space(14.0);
        ui.label(RichText::new("   SİSTEM").small().color(muted));
        ui.add_space(2.0);
        let cpu = self.hardware.iter().flatten().find(|c| c.kind == "İşlemci");
        let sub = cpu.map_or("Bileşenler ve markalar", |c| c.name.as_str());
        if nav_item(ui, self.page == Page::Hardware, "🖥", "Donanım", 0, sub).clicked()
            && self.page != Page::Hardware
        {
            self.page = Page::Hardware;
            self.hw_since = ui.input(|i| i.time);
        }
        let sub = self.map.as_ref().map_or("Klasör boyutları".into(), |m| human_size(m.size));
        if nav_item(ui, self.page == Page::Map, "🗺", "Disk haritası", 0, &sub).clicked() {
            self.page = Page::Map;
            if self.map.is_none() && self.map_scan.is_none() {
                self.start_map(ui.ctx().clone());
            }
        }
        if self.watcher.is_some() {
            let n = self.lock.apps.len();
            let sub = if n == 0 { "Kapalı".to_string() } else { format!("{n} uygulama kilitli") };
            if nav_item(ui, self.page == Page::Lock, "🔒", "Uygulama kilidi", 0, &sub).clicked() {
                self.page = Page::Lock;
            }
        }
        if telemetry::URL.is_some() {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.add_space(14.0);
                let mut on = self.telemetry.consent == Some(true);
                if ui
                    .checkbox(
                        &mut on,
                        RichText::new("Anonim istatistik gönder")
                            .small()
                            .color(muted),
                    )
                    .changed()
                {
                    self.telemetry.set_consent(on);
                }
            });
        }
    }

    fn header(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        // Disables the controls; the deletion would otherwise race with a new scan.
        let scanning = self.scanning.is_some() || self.deleting.is_some();
        card(ui, |ui| {
            let edit = egui::TextEdit::singleline(&mut self.root)
                .hint_text("Taranacak klasör")
                .margin(Margin::symmetric(10, 8));
            ui.add_sized([ui.available_width(), 34.0], edit);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let label = if self.scanning.is_some() {
                        "Taranıyor…"
                    } else {
                        "🔍  Tara"
                    };
                    let scan =
                        egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                            .fill(ACCENT)
                            .min_size(vec2(110.0, 32.0));
                    if ui.add_enabled(!scanning, scan).clicked() {
                        self.start_scan(ctx.clone());
                    }
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new("Gözat…").min_size(vec2(0.0, 32.0)),
                        )
                        .clicked()
                        && let Some(dir) = rfd::FileDialog::new()
                            .set_directory(self.root.trim())
                            .pick_folder()
                    {
                        self.root = dir.display().to_string();
                    }
                    ui.add_space(8.0);
                    ui.with_layout(
                        egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
                        |ui| {
                            let quick = [
                                ("Ana klasör", dirs::home_dir()),
                                ("İndirilenler", dirs::download_dir()),
                                ("Masaüstü", dirs::desktop_dir()),
                                ("Belgeler", dirs::document_dir()),
                            ];
                            for (name, dir) in quick {
                                if let Some(dir) = dir
                                    && ui
                                        .add_enabled(
                                            !scanning,
                                            egui::Button::new(RichText::new(name).small())
                                                .min_size(vec2(0.0, 32.0)),
                                        )
                                        .clicked()
                                {
                                    self.root = dir.display().to_string();
                                }
                            }
                        },
                    );
                });
            });
        });
        ui.add_space(12.0);

        let (found_n, found) = self.totals(None, false);
        let (sel_n, sel) = self.totals(None, true);
        let scanned = match &self.scanning {
            Some((_, p)) => p.load(Ordering::Relaxed),
            None => self.scanned_files,
        };
        ui.columns(3, |cols| {
            stat(
                &mut cols[0],
                "Bulunan",
                &human_size(found),
                &format!("{found_n} dosya"),
                None,
            );
            stat(
                &mut cols[1],
                "Kazanılacak alan",
                &human_size(sel),
                &format!("{sel_n} dosya seçili"),
                Some(ACCENT),
            );
            stat(
                &mut cols[2],
                "Taranan dosya",
                &scanned.to_string(),
                if self.scanning.is_some() {
                    "tarama sürüyor…"
                } else {
                    "toplam"
                },
                None,
            );
        });
    }

    fn content(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        if let Some((_, progress)) = &self.scanning
            && self.phase.is_none()
        {
            let n = progress.load(Ordering::Relaxed);
            empty_state(ui, |ui| {
                ui.add(egui::Spinner::new().size(42.0).color(ACCENT));
                ui.add_space(10.0);
                ui.label(RichText::new("Taranıyor…").size(20.0).strong());
                ui.label(
                    RichText::new(format!("{n} dosya incelendi. Sonuçlar birazdan görünecek."))
                        .color(muted),
                );
            });
            return;
        }
        if !self.has_scanned {
            empty_state(ui, |ui| {
                ui.label(RichText::new("✨").size(56.0).color(ACCENT));
                ui.add_space(6.0);
                ui.label(RichText::new("Diskinizde yer açın").size(22.0).strong());
                ui.label(RichText::new("Bir klasör seçip Tara'ya basın. Silmeden önce ne kadar yer kazanacağınızı göreceksiniz.").color(muted));
            });
            return;
        }

        let tab = self.tab;
        // Buttons first (right to left), so the title gets only the space left and truncates instead of overlapping.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 36.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if ui.button("Seçimi kaldır").clicked() {
                    deskmonk::set_selection(&mut self.items, Some(tab), false);
                }
                if ui.button("Tümünü seç").clicked() {
                    deskmonk::set_selection(&mut self.items, Some(tab), true);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let title = RichText::new(format!("{}  {}", icon(tab), tab.label()))
                        .size(20.0)
                        .strong();
                    ui.add(egui::Label::new(title).truncate());
                });
            },
        );
        ui.label(RichText::new(description(tab)).color(muted));
        if let Some(phase) = self.phase {
            let text = match phase {
                Phase::Walking => {
                    "Taranıyor… Bulunanlar geldikçe listeye ekleniyor. Temizlik tarama bitince açılır."
                }
                Phase::Comparing => {
                    "Dosyalar listelendi, kopyalar karşılaştırılıyor… Temizlik tarama bitince açılır."
                }
            };
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
                ui.label(RichText::new(text).small().color(ACCENT));
            });
        }
        ui.add_space(6.0);

        // Duplicate items arrive sorted by group, so a header goes wherever the group number changes.
        let mut rows = Vec::new();
        let mut last_group = 0;
        for (i, item) in self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.category == tab)
        {
            if item.group > 0 && item.group != last_group {
                rows.push(Row::Group(item.group));
                last_group = item.group;
            }
            rows.push(Row::Item(i));
        }
        if rows.is_empty() && self.phase.is_some() {
            let (title, text) = match (self.phase, tab) {
                (_, Category::Duplicate) => (
                    "Kopyalar aranıyor…",
                    "Dosya listesi bitince aynı boyuttaki dosyaların içerikleri karşılaştırılır. Diğer kategorilere şimdiden bakabilirsiniz.",
                ),
                _ => (
                    "Taranıyor…",
                    "Bu kategoride henüz bir şey bulunmadı, tarama sürüyor.",
                ),
            };
            empty_state(ui, |ui| {
                ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                ui.label(RichText::new(title).size(17.0).strong());
                ui.label(RichText::new(text).color(muted));
            });
            return;
        }
        if rows.is_empty() {
            empty_state(ui, |ui| {
                ui.label(
                    RichText::new("✔")
                        .size(44.0)
                        .color(Color32::from_rgb(52, 199, 89)),
                );
                ui.label(
                    RichText::new("Bu kategoride temizlenecek dosya yok")
                        .size(17.0)
                        .strong(),
                );
            });
            return;
        }

        card(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                ROW_H,
                rows.len(),
                |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for row in &rows[range] {
                        match *row {
                            Row::Group(g) => group_row(ui, g, &self.items),
                            Row::Item(i) => item_row(ui, &mut self.items[i], i),
                        }
                    }
                },
            );
        });
    }

    /// Hardware as a terminal listing that types itself out.
    fn hardware_view(&mut self, ui: &mut Ui) {
        let mono = FontId::monospace(14.0);
        let mut lines: Vec<Vec<(String, Color32)>> = vec![vec![
            ("root@deskmonk:~$ ".into(), TERM_DIM),
            ("./hwinfo --brands".into(), TERM),
        ]];
        match &self.hardware {
            None => lines.push(vec![("[....] donanım taranıyor".into(), TERM_DIM)]),
            Some(parts) if parts.is_empty() => {
                lines.push(vec![("[FAIL] donanım bilgisi okunamadı".into(), Color32::from_rgb(255, 80, 80))])
            }
            Some(parts) => {
                lines.push(vec![(format!("[ OK ] {} bileşen bulundu", parts.len()), TERM_DIM)]);
                lines.push(Vec::new());
                for c in parts {
                    let mut line = vec![(format!("{:<12} ", c.kind.replace('i', "İ").to_uppercase()), TERM_DIM)];
                    if !c.brand.is_empty() {
                        let color = hardware::brand_of(&c.brand).map_or(TERM, |(_, [r, g, b])| Color32::from_rgb(r, g, b));
                        line.push((format!("[{}] ", c.brand.to_uppercase()), color));
                    }
                    line.push((c.name.clone(), TERM));
                    lines.push(line);
                    if !c.detail.is_empty() {
                        lines.push(vec![(format!("{:12} └─ {}", "", c.detail), TERM_DIM)]);
                    }
                }
            }
        }
        let speed_label = format!("{:<12} ", "DİSK HIZI");
        match (&self.speed_run, &self.speed) {
            (Some((_, done)), _) => {
                let done = done.load(Ordering::Relaxed);
                let (step, part) = if done < SPEED_TEST { ("yazılıyor", done) } else { ("okunuyor", done - SPEED_TEST) };
                let pct = part * 100 / SPEED_TEST;
                let bar = format!("[{}{}]", "#".repeat(pct as usize / 5), ".".repeat(20 - pct as usize / 5));
                lines.push(vec![(speed_label, TERM_DIM), (format!("{bar} {step} %{pct}"), TERM)]);
            }
            (None, Some(Ok(s))) => {
                let rate = |b: f64| format!("{} MB/s", (b / 1e6).round());
                lines.push(vec![(speed_label, TERM_DIM), (format!("yazma {} · okuma {}", rate(s.write), rate(s.read)), TERM)]);
                lines.push(vec![(format!("{:12} └─ 1 GB sıralı yazma/okuma, sistem diski", ""), TERM_DIM)]);
            }
            (None, Some(Err(e))) => {
                lines.push(vec![(speed_label, TERM_DIM), (format!("[FAIL] {e}"), Color32::from_rgb(255, 80, 80))]);
            }
            (None, None) => {}
        }
        lines.push(vec![("root@deskmonk:~$ ".into(), TERM_DIM)]);

        let now = ui.input(|i| i.time);
        let shown = ((now - self.hw_since) * 25.0) as usize + 1;
        // Blinking cursor; also keeps the typing animation and the "scanning" line moving.
        ui.ctx().request_repaint_after(Duration::from_millis(if shown < lines.len() { 16 } else { 500 }));
        let cursor = if (now * 2.0) as i64 % 2 == 0 { "█" } else { " " };
        egui::Frame::new()
            .fill(TERM_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(30, 92, 43)))
            .corner_radius(12)
            .inner_margin(Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
                    let count = shown.min(lines.len());
                    for (i, line) in lines.iter().take(count).enumerate() {
                        let mut job = egui::text::LayoutJob::default();
                        for (text, color) in line {
                            job.append(text, 0.0, egui::TextFormat::simple(mono.clone(), *color));
                        }
                        if i + 1 == count {
                            job.append(cursor, 0.0, egui::TextFormat::simple(mono.clone(), TERM));
                        }
                        ui.label(job);
                    }
                    if shown >= lines.len() {
                        ui.add_space(14.0);
                        let label = if self.speed.is_some() { "▶ Hız testini tekrarla" } else { "▶ Disk hız testi" };
                        let button = egui::Button::new(RichText::new(label).font(mono.clone()).color(TERM_BG)).fill(TERM);
                        if ui.add_enabled(self.speed_run.is_none(), button).clicked() {
                            let (tx, rx) = channel();
                            let done = Arc::new(AtomicU64::new(0));
                            let (counter, ctx) = (done.clone(), ui.ctx().clone());
                            std::thread::spawn(move || {
                                let _ = tx.send(deskmonk::speedtest::run(&std::env::temp_dir(), SPEED_TEST, &counter));
                                ctx.request_repaint();
                            });
                            self.speed_run = Some((rx, done));
                        }
                    }
                });
            });
    }

    /// Treemap of folder sizes: click a folder to go inside, right click to reveal it.
    fn map_view(&mut self, ui: &mut Ui) {
        let mono = FontId::monospace(13.0);
        let busy = self.map_scan.is_some();
        egui::Frame::new()
            .fill(TERM_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(30, 92, 43)))
            .corner_radius(12)
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("root@deskmonk:~$ du --map").font(mono.clone()).color(TERM_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let go = egui::Button::new(RichText::new("▶ Haritala").font(mono.clone()).color(TERM_BG)).fill(TERM);
                        if ui.add_enabled(!busy, go).clicked() {
                            self.start_map(ui.ctx().clone());
                        }
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new("Gözat…").font(mono.clone()))).clicked()
                            && let Some(dir) = rfd::FileDialog::new().set_directory(self.root.trim()).pick_folder()
                        {
                            self.root = dir.display().to_string();
                            self.start_map(ui.ctx().clone());
                        }
                        let edit = egui::TextEdit::singleline(&mut self.root).font(mono.clone()).text_color(TERM);
                        ui.add_sized([ui.available_width(), 26.0], edit);
                    });
                });
                ui.add_space(6.0);
                if let Some((_, files)) = &self.map_scan {
                    let n = files.load(Ordering::Relaxed);
                    let line = format!("[....] {n} dosya ölçüldü {}", if (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0 { "█" } else { " " });
                    ui.label(RichText::new(line).font(mono.clone()).color(TERM));
                    return;
                }
                let Some(tree) = &self.map else {
                    ui.label(RichText::new(format!("[FAIL] {}", self.status)).font(mono.clone()).color(DANGER));
                    return;
                };

                // Folder being looked at, and the chain of folders above it.
                let mut chain = vec![tree];
                for &i in &self.map_path {
                    chain.push(&chain[chain.len() - 1].children[i]);
                }
                let node = chain[chain.len() - 1];
                let mut go_up = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (depth, n) in chain.iter().enumerate() {
                        let last = depth + 1 == chain.len();
                        let text = RichText::new(&n.name).font(mono.clone()).color(if last { TERM } else { TERM_DIM });
                        if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            go_up = Some(depth);
                        }
                        if !last {
                            ui.label(RichText::new(" / ").font(mono.clone()).color(TERM_DIM));
                        }
                    }
                    ui.label(RichText::new(format!("  [{}]", human_size(node.size))).font(mono.clone()).color(TERM));
                });
                ui.add_space(8.0);

                let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                let sizes: Vec<u64> = node.children.iter().map(|c| c.size).collect();
                let boxes = diskmap::treemap(&sizes, [area.min.x, area.min.y, area.width(), area.height()]);
                let folder = chain.iter().skip(1).fold(self.map_root.clone(), |p, n| p.join(&n.name));
                let mut go_in = None;
                for (i, (child, b)) in node.children.iter().zip(boxes).enumerate() {
                    let r = Rect::from_min_size(Pos2::new(b[0], b[1]), vec2(b[2], b[3])).shrink(1.5);
                    if r.width() < 1.0 || r.height() < 1.0 {
                        continue;
                    }
                    let resp = ui.interact(r, ui.id().with(("map", self.map_path.len(), i)), Sense::click());
                    let color = NEON[i % NEON.len()];
                    let fill = color.gamma_multiply(if resp.hovered() { 0.45 } else { 0.22 });
                    ui.painter().rect(r, 4.0, fill, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                    if r.width() > 50.0 && r.height() > 34.0 {
                        let p = ui.painter().with_clip_rect(r.shrink(4.0));
                        p.text(r.min + vec2(7.0, 6.0), Align2::LEFT_TOP, &child.name, mono.clone(), Color32::WHITE);
                        p.text(r.min + vec2(7.0, 22.0), Align2::LEFT_TOP, human_size(child.size), FontId::monospace(11.5), color);
                    }
                    let can_enter = child.dir && !child.children.is_empty();
                    let pct = child.size as f64 / node.size.max(1) as f64 * 100.0;
                    let hint = if can_enter { "Tıkla: içine gir · Sağ tık: klasörde göster" } else { "Sağ tık: klasörde göster" };
                    let resp = resp.on_hover_text(format!("{}\n{} · %{pct:.1}\n{hint}", child.name, human_size(child.size)));
                    let resp = if can_enter { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
                    if resp.clicked() && can_enter {
                        go_in = Some(i);
                    }
                    if resp.secondary_clicked() {
                        // The merged "küçük öğe" box has no path of its own.
                        let path = folder.join(&child.name);
                        reveal_in_folder(if path.exists() { &path } else { &folder });
                    }
                }
                if let Some(depth) = go_up {
                    self.map_path.truncate(depth);
                } else if let Some(i) = go_in {
                    self.map_path.push(i);
                }
            });
    }

    /// Password setup, then the list of locked apps (behind the same password).
    fn lock_view(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        ui.label(RichText::new("🔒  Uygulama kilidi").size(20.0).strong());
        ui.label(RichText::new("Kilitli bir uygulama açılınca deskMonk onu durdurur ve şifre sorar. Doğru şifreyle uygulama açılır, kapatılınca yeniden kilitlenir.").color(muted));
        ui.add_space(14.0);
        let password = |ui: &mut Ui, text: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(text).password(true).hint_text(hint).desired_width(280.0))
        };
        if !self.lock.has_password() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Önce bir şifre belirleyin").strong());
                password(ui, &mut self.lock_pw, "Yeni şifre (en az 4 karakter)");
                password(ui, &mut self.lock_pw2, "Şifre tekrar");
                if ui.add(egui::Button::new(RichText::new("Şifreyi kaydet").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    if self.lock_pw.chars().count() < 4 {
                        self.lock_error = "Şifre en az 4 karakter olmalı".into();
                    } else if self.lock_pw != self.lock_pw2 {
                        self.lock_error = "Şifreler aynı değil".into();
                    } else {
                        self.lock.set_password(&self.lock_pw);
                        self.lock_error = self.lock.save().err().map(|e| format!("Kaydedilemedi: {e}")).unwrap_or_default();
                        self.lock_open = true;
                        self.lock_pw.clear();
                        self.lock_pw2.clear();
                    }
                }
                ui.label(RichText::new("Şifreyi unutursanız kilitler kaldırılamaz; bir yere not edin.").small().color(muted));
            });
        } else if !self.lock_open {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Ayarları açmak için şifre gerekli").strong());
                let field = password(ui, &mut self.lock_pw, "Şifre");
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Kilidi aç").clicked() || enter {
                    self.lock_open = self.lock.check(&self.lock_pw);
                    self.lock_error = if self.lock_open { String::new() } else { "Yanlış şifre".into() };
                    self.lock_pw.clear();
                }
            });
        } else {
            let mut changed = false;
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Kilitli uygulamalar ({})", self.lock.apps.len())).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🔒 Ayarları kilitle").clicked() {
                            self.lock_open = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("+ Uygulama ekle…").color(Color32::WHITE)).fill(ACCENT)).clicked()
                            && let Some(app) = rfd::FileDialog::new().set_directory("/Applications").add_filter("Uygulama", &["app"]).pick_file()
                        {
                            if !applock::can_lock(&app) {
                                self.lock_error = format!("{} kilitlenemez (sistem uygulaması veya deskMonk)", app.display());
                            } else if !self.lock.apps.contains(&app) {
                                self.lock.apps.push(app);
                                changed = true;
                            }
                        }
                    });
                });
                ui.add_space(6.0);
                if self.lock.apps.is_empty() {
                    ui.label(RichText::new("Henüz kilitli uygulama yok.").color(muted));
                }
                let mut remove = None;
                for (i, app) in self.lock.apps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("🔒  {}", app.file_stem().unwrap_or_default().to_string_lossy())).size(15.0));
                        ui.label(RichText::new(app.display().to_string()).small().color(muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Kilidi kaldır").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    self.lock.apps.remove(i);
                    changed = true;
                }
            });
            if changed {
                self.lock_error = self.lock.save().err().map(|e| format!("Kaydedilemedi: {e}")).unwrap_or_default();
                if let Some(w) = &self.watcher {
                    w.set_apps(self.lock.apps.clone());
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("deskMonk açık olduğu sürece çalışır: pencereyi kapatmak sorun değil, ama Cmd+Q ile çıkılırsa kilit kalkar. Zaten açık olan bir uygulama, kapatılana kadar kilitlenmez.").small().color(muted));
        }
        if !self.lock_error.is_empty() && self.lock_prompt.is_none() {
            ui.label(RichText::new(&self.lock_error).color(DANGER));
        }
    }

    fn footer(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        let (n, size) = self.totals(None, true);
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 44.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let text = RichText::new(format!("🗑  {} temizle", human_size(size)))
                    .strong()
                    .size(15.0)
                    .color(Color32::WHITE);
                let button = egui::Button::new(text)
                    .fill(DANGER)
                    .min_size(vec2(170.0, 40.0))
                    .corner_radius(10);
                if let Some(d) = &self.deleting {
                    let done = d.done.load(Ordering::Relaxed);
                    let bar = egui::ProgressBar::new(done as f32 / d.total.max(1) as f32)
                        .desired_width(220.0)
                        .desired_height(40.0)
                        .fill(ACCENT)
                        .text(
                            RichText::new(format!("Siliniyor… {done}/{}", d.total))
                                .color(Color32::WHITE),
                        )
                        .animate(true);
                    ui.add(bar);
                } else if ui
                    .add_enabled(n > 0 && self.scanning.is_none(), button)
                    .clicked()
                {
                    self.confirm = true;
                }
                ui.add_space(8.0);
                // Every category at once; the header's "Tümünü seç" covers only the open one.
                let all = egui::Button::new(RichText::new("☑  Hepsini seç").size(15.0))
                    .min_size(vec2(130.0, 40.0))
                    .corner_radius(10);
                if ui.add_enabled(!self.items.is_empty() && self.deleting.is_none(), all).clicked() {
                    deskmonk::set_selection(&mut self.items, None, true);
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(format!("{n} dosya seçildi")).strong());
                    let status = if self.status.is_empty() {
                        "Gereksiz dosyalar kalıcı silinir, diğerleri çöp kutusuna taşınır."
                    } else {
                        &self.status
                    };
                    ui.label(RichText::new(status).small().color(muted));
                });
            },
        );
        ui.add_space(10.0);
    }
}

impl eframe::App for App {
    // macOS: the close button hides the app instead of quitting, so a scan or cleaning keeps running and
    // clicking the Dock icon brings the window back. Cmd+Q / Dock → Quit still quit.
    // In `logic`, not `ui`: eframe skips `ui` while the window is hidden, and an uncancelled close quits.
    #[cfg(target_os = "macos")]
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Runs while the window is hidden too, so a locked app's password prompt can bring deskMonk back.
        while let Some(app) = self.blocked.as_ref().and_then(|rx| rx.try_recv().ok()) {
            if self.lock_prompt.is_some() {
                // One prompt at a time; this app asks again on its next launch.
                if let Some(w) = &self.watcher {
                    w.dismiss(&app);
                }
                continue;
            }
            self.lock_prompt = Some(app);
            self.lock_error.clear();
            self.lock_pw.clear();
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                ns_app.unhide(None);
                #[allow(deprecated)] // `activate` needs macOS 14; this app supports 11+.
                ns_app.activateIgnoringOtherApps(true);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                objc2_app_kit::NSApplication::sharedApplication(mtm).hide(None);
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.autostart) {
            self.start_scan(ctx.clone());
        }
        self.poll_deletion();
        if let Some(rx) = &self.hw_rx
            && let Ok(parts) = rx.try_recv()
        {
            self.hardware = Some(parts);
            self.hw_rx = None;
        }
        // Waits for consent: the user may say yes only after the hardware is read.
        if !self.hw_sent
            && self.telemetry.consent == Some(true)
            && let Some(parts) = &self.hardware
        {
            self.telemetry.opened("gui", parts);
            self.hw_sent = true;
        }
        if self.deleting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        while let Some((rx, progress)) = &self.scanning
            && let Ok((phase, mut items)) = rx.try_recv()
        {
            let files = progress.load(Ordering::Relaxed);
            deskmonk::keep_selection(&self.items, &mut items);
            self.items = items;
            self.has_scanned = true;
            self.phase = phase;
            if phase.is_none() {
                self.scanned_files = files;
                self.telemetry.scan_done("gui", files, &self.items);
                self.scanning = None;
            }
            // Stay on the current category unless it is empty.
            if !self.items.iter().any(|i| i.category == self.tab)
                && let Some(cat) = Category::ALL
                    .into_iter()
                    .find(|&c| self.items.iter().any(|i| i.category == c))
            {
                self.tab = cat;
            }
        }
        if let Some((rx, _)) = &self.map_scan
            && let Ok(tree) = rx.try_recv()
        {
            self.map = Some(tree);
            self.map_path.clear();
            self.map_scan = None;
        }
        if let Some((rx, _)) = &self.speed_run
            && let Ok(result) = rx.try_recv()
        {
            self.speed = Some(result.map_err(|e| e.to_string()));
            self.speed_run = None;
        }
        if self.scanning.is_some() || self.map_scan.is_some() || self.speed_run.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.page != Page::Lock && self.lock_open {
            self.lock_open = false;
            self.lock_pw.clear();
        }
        let (bg, side, border, muted) = palette(ui.visuals().dark_mode);
        egui::Panel::top("topbar")
            .exact_size(58.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(18, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(32.0, 32.0)));
                    }
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new("deskMonk").size(19.0).strong());
                    ui.label(RichText::new("by pelamx").size(13.0).color(ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        ui.label(
                            RichText::new("Akıllı depolama temizleyici")
                                .small()
                                .color(muted),
                        );
                    });
                });
            });
        egui::Panel::left("sidebar")
            .exact_size(230.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(12, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("footer")
            .frame(
                egui::Frame::new()
                    .fill(side)
                    .inner_margin(Margin::symmetric(24, 0))
                    .stroke(Stroke::new(1.0, border)),
            )
            .show(ui, |ui| self.footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin::same(24)))
            .show(ui, |ui| {
                match self.page {
                    Page::Hardware => self.hardware_view(ui),
                    Page::Map => self.map_view(ui),
                    Page::Lock => self.lock_view(ui),
                    Page::Clean => {
                        self.header(ui, &ctx);
                        ui.add_space(18.0);
                        self.content(ui);
                    }
                }
            });

        if self.confirm {
            let (n, size) = self.totals(None, true);
            let modal = egui::Modal::new(egui::Id::new("confirm")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("🗑  Dosyalar temizlensin mi?").size(19.0).strong());
                ui.add_space(4.0);
                let (perm_n, perm_size) = self.items.iter().filter(|i| i.selected && deskmonk::is_permanent(i, false)).fold((0, 0), |(c, s), i| (c + 1, s + i.size));
                ui.label(format!("{n} öğe, toplam {}.", human_size(size)));
                if perm_n > 0 {
                    ui.label(RichText::new(format!("• {perm_n} gereksiz öğe ({}) kalıcı olarak silinecek. Önbellekler gerektiğinde yeniden oluşur.", human_size(perm_size))).color(DANGER));
                }
                if n > perm_n {
                    ui.label(format!("• {} dosya ({}) çöp kutusuna taşınacak, geri alınabilir.", n - perm_n, human_size(size - perm_size)));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let go = egui::Button::new(RichText::new("Temizle").strong().color(Color32::WHITE)).fill(DANGER);
                    if ui.add(go).clicked() {
                        self.delete_selected(ctx.clone());
                        self.confirm = false;
                    }
                    if ui.button("Vazgeç").clicked() {
                        self.confirm = false;
                    }
                });
            });
            if modal.should_close() {
                self.confirm = false;
            }
        }

        if let Some(app) = self.lock_prompt.clone() {
            let name = app.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let mut close = false;
            egui::Modal::new(egui::Id::new("applock")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new(format!("🔒  {name} kilitli")).size(19.0).strong());
                ui.label("Açmak için deskMonk şifresini girin.");
                ui.add_space(8.0);
                let field = ui.add(egui::TextEdit::singleline(&mut self.lock_pw).password(true).hint_text("Şifre").desired_width(f32::INFINITY));
                field.request_focus();
                if !self.lock_error.is_empty() {
                    ui.label(RichText::new(&self.lock_error).color(DANGER));
                }
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let open = ui.add(egui::Button::new(RichText::new("Aç").strong().color(Color32::WHITE)).fill(ACCENT)).clicked()
                        // The field keeps focus every frame, so Enter is read directly rather than via lost_focus.
                        || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if open {
                        if self.lock.check(&self.lock_pw) {
                            if let Some(w) = &self.watcher {
                                w.unlock(&app);
                            }
                            close = true;
                        } else {
                            self.lock_error = "Yanlış şifre".into();
                        }
                        self.lock_pw.clear();
                    }
                    if ui.button("Vazgeç").clicked() {
                        if let Some(w) = &self.watcher {
                            w.dismiss(&app);
                        }
                        close = true;
                    }
                });
            });
            if close {
                self.lock_prompt = None;
                self.lock_error.clear();
                self.lock_pw.clear();
            }
        }

        if self.telemetry.needs_prompt() {
            egui::Modal::new(egui::Id::new("consent")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("📊  Anonim kullanım istatistikleri").size(19.0).strong());
                ui.add_space(4.0);
                ui.label("deskMonk'u geliştirmek için tarama ve temizleme sayıları (dosya sayısı, boyut, işletim sistemi) ve donanım bilgileri (işlemci, bellek, disk, ekran kartı, pil: marka, model, doluluk) gönderilsin mi?");
                ui.label(RichText::new("Dosya adları, yollar, seri numaraları ve kullanıcı adı asla gönderilmez.").strong());
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("İzin ver").strong().color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        self.telemetry.set_consent(true);
                    }
                    if ui.button("Hayır").clicked() {
                        self.telemetry.set_consent(false);
                    }
                });
            });
        }
    }
}

// ---------- widgets ----------

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (_, card, border, _) = palette(ui.visuals().dark_mode);
    egui::Frame::new()
        .fill(card)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(12)
        .inner_margin(Margin::same(14))
        .show(ui, add)
        .inner
}

fn empty_state(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space((ui.available_height() * 0.25).max(20.0));
    ui.vertical_centered(add);
}

fn stat(ui: &mut Ui, title: &str, value: &str, sub: &str, color: Option<Color32>) {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.label(RichText::new(title).small().color(muted));
        let size = if ui.available_width() > 150.0 {
            26.0
        } else {
            19.0
        };
        let value = RichText::new(value).size(size).strong();
        let value = egui::Label::new(match color {
            Some(c) => value.color(c),
            None => value,
        });
        ui.add(value.wrap_mode(egui::TextWrapMode::Extend));
        ui.label(RichText::new(sub).small().color(muted));
    });
}

fn nav_item(
    ui: &mut Ui,
    selected: bool,
    icon: &str,
    label: &str,
    count: usize,
    sub: &str,
) -> egui::Response {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        ACCENT.gamma_multiply(0.16)
    } else if resp.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 10.0, bg);
    if selected {
        p.rect_filled(
            Rect::from_min_size(rect.min + vec2(0.0, 14.0), vec2(3.0, rect.height() - 28.0)),
            2.0,
            ACCENT,
        );
    }
    let text = ui.visuals().strong_text_color();
    p.text(
        rect.left_center() + vec2(18.0, 0.0),
        Align2::LEFT_CENTER,
        icon,
        FontId::proportional(19.0),
        if selected { ACCENT } else { text },
    );
    p.text(
        rect.min + vec2(50.0, 10.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(14.5),
        text,
    );
    p.text(
        rect.min + vec2(50.0, 30.0),
        Align2::LEFT_TOP,
        sub,
        FontId::proportional(12.0),
        muted,
    );
    if count > 0 {
        let color = if selected { Color32::WHITE } else { text };
        let galley = p.layout_no_wrap(count.to_string(), FontId::proportional(12.0), color);
        let pill = Rect::from_center_size(
            rect.right_center() - vec2(20.0 + galley.size().x / 2.0, 0.0),
            galley.size() + vec2(16.0, 6.0),
        );
        p.rect_filled(
            pill,
            10.0,
            if selected {
                ACCENT
            } else {
                ui.visuals().widgets.inactive.weak_bg_fill
            },
        );
        p.galley(pill.center() - galley.size() / 2.0, galley, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn single_line(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        ..Default::default()
    };
    ui.painter().layout_job(job)
}

/// Single line that drops characters from the start ("…/end/of/path") so the most specific part stays visible.
fn tail_line(ui: &Ui, text: String, font: FontId, color: Color32, width: f32) -> Arc<egui::Galley> {
    let p = ui.painter();
    let mut galley = p.layout_no_wrap(text.clone(), font.clone(), color);
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while galley.size().x > width && keep > 1 {
        keep = (keep as f32 * width / galley.size().x) as usize - 1;
        let tail: String = chars[chars.len() - keep.max(1)..].iter().collect();
        galley = p.layout_no_wrap(format!("…{tail}"), font.clone(), color);
    }
    galley
}

fn group_row(ui: &mut Ui, group: usize, items: &[Item]) {
    let (_, _, _, muted) = palette(ui.visuals().dark_mode);
    let members: Vec<&Item> = items.iter().filter(|i| i.group == group).collect();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let each = members[0].size;
    let text = format!(
        "GRUP {group}  ·  {} kopya  ·  her biri {}  ·  {} kazanılabilir",
        members.len(),
        human_size(each),
        human_size(each * (members.len() as u64 - 1))
    );
    ui.painter().text(
        rect.left_bottom() + vec2(4.0, -10.0),
        Align2::LEFT_BOTTOM,
        text,
        FontId::proportional(11.5),
        muted,
    );
}

fn item_row(ui: &mut Ui, item: &mut Item, idx: usize) {
    let dark = ui.visuals().dark_mode;
    let (_, _, border, muted) = palette(dark);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    // Reveal-in-folder button, shown on hover. Registered after the row so it wins the click.
    let reveal_rect =
        Rect::from_center_size(rect.right_center() - vec2(118.0, 0.0), vec2(30.0, 30.0));
    let reveal = ui
        .interact(reveal_rect, ui.id().with(("reveal", idx)), Sense::click())
        .on_hover_text("Klasörde göster");
    if reveal.clicked() {
        reveal_in_folder(&item.path);
    } else if resp.clicked() {
        item.selected ^= true;
    }

    let p = ui.painter();
    let hovered = resp.hovered() || reveal.hovered();
    if item.selected {
        p.rect_filled(
            rect,
            8.0,
            ACCENT.gamma_multiply(if dark { 0.10 } else { 0.07 }),
        );
    } else if hovered {
        p.rect_filled(
            rect,
            8.0,
            ui.visuals()
                .widgets
                .hovered
                .weak_bg_fill
                .gamma_multiply(0.6),
        );
    }
    p.hline(
        rect.x_range().shrink(6.0),
        rect.bottom(),
        Stroke::new(1.0, border.gamma_multiply(0.6)),
    );

    let cb = Rect::from_center_size(rect.left_center() + vec2(20.0, 0.0), vec2(18.0, 18.0));
    if item.selected {
        p.rect_filled(cb, 5.0, ACCENT);
        p.text(
            cb.center(),
            Align2::CENTER_CENTER,
            "✔",
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    } else {
        p.rect_stroke(cb, 5.0, Stroke::new(1.5, muted), egui::StrokeKind::Inside);
    }

    let text_x = rect.left() + 44.0;
    let text_w = (rect.right() - 150.0 - text_x).max(40.0);
    let name = item.name();
    let dir = item
        .path
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let name_g = single_line(
        ui,
        name,
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
        text_w,
    );
    let dir_g = tail_line(ui, dir, FontId::proportional(11.5), muted, text_w);
    p.galley(Pos2::new(text_x, rect.top() + 6.0), name_g, muted);
    p.galley(Pos2::new(text_x, rect.top() + 25.0), dir_g, muted);

    if hovered {
        let fill = if reveal.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().widgets.inactive.weak_bg_fill
        };
        p.rect_filled(reveal_rect, 7.0, fill);
        p.text(
            reveal_rect.center(),
            Align2::CENTER_CENTER,
            "📂",
            FontId::proportional(14.0),
            ui.visuals().text_color(),
        );
    }
    p.text(
        rect.right_center() - vec2(16.0, 0.0),
        Align2::RIGHT_CENTER,
        human_size(item.size),
        FontId::proportional(14.0),
        ui.visuals().strong_text_color(),
    );
}

fn reveal_in_folder(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()
    };
    if let Err(e) = result {
        eprintln!("could not open folder: {e}");
    }
}
