#![cfg_attr(windows, windows_subsystem = "windows")] // no console window on Windows

use cleanspace::scan::{Category, Item, human_size};
use cleanspace::telemetry::{self, Telemetry};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke,
    Theme, Ui, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const ACCENT: Color32 = Color32::from_rgb(79, 110, 247);
const DANGER: Color32 = Color32::from_rgb(229, 72, 77);
const ROW_H: f32 = 46.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("cleanSpace by pelamx"),
        ..Default::default()
    };
    eframe::run_native(
        "CleanSpace",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            let mut app = App::default();
            // `cleanspace FOLDER` scans that folder right away.
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

enum Row {
    Group(usize),
    Item(usize),
}

struct App {
    root: String,
    items: Vec<Item>,
    tab: Category,
    scanning: Option<(Receiver<Vec<Item>>, Arc<AtomicUsize>)>,
    scanned_files: usize,
    has_scanned: bool,
    confirm: bool,
    deleting: Option<cleanspace::Deletion>,
    status: String,
    telemetry: Telemetry,
    autostart: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            root: dirs::home_dir().unwrap_or_default().display().to_string(),
            items: Vec::new(),
            tab: Category::Duplicate,
            scanning: None,
            scanned_files: 0,
            has_scanned: false,
            confirm: false,
            deleting: None,
            status: String::new(),
            telemetry: Telemetry::load(),
            autostart: false,
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
            let _ = tx.send(cleanspace::scan_default(&root, &counter));
            ctx.request_repaint();
        });
        self.items.clear();
        self.status.clear();
        self.scanning = Some((rx, progress));
    }

    fn delete_selected(&mut self, ctx: egui::Context) {
        self.deleting = Some(cleanspace::Deletion::start(
            &mut self.items,
            false,
            move || ctx.request_repaint(),
        ));
    }

    fn poll_deletion(&mut self) {
        let Some((failed, freed)) = self.deleting.as_ref().and_then(|d| d.poll()) else {
            return;
        };
        let ok = self.deleting.take().unwrap().total - failed.len();
        self.telemetry.clean_done("gui", ok, freed);
        self.status = format!("✔ {ok} öğe temizlendi, {} yer açıldı", human_size(freed));
        if !failed.is_empty() {
            self.status += &format!(" · {} öğe silinemedi (hâlâ seçili)", failed.len());
        }
        self.items.extend(failed);
    }

    fn totals(&self, cat: Option<Category>, only_selected: bool) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| cat.is_none_or(|c| i.category == c) && (!only_selected || i.selected))
            .fold((0, 0), |(n, s), i| (n + 1, s + i.size))
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let (_, _, _, muted) = palette(ui.visuals().dark_mode);
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("✨").size(26.0).color(ACCENT));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.label(RichText::new("cleanSpace").size(19.0).strong());
                    ui.label(RichText::new("by pelamx").size(12.0).color(ACCENT));
                });
                ui.label(
                    RichText::new("Akıllı depolama temizleyici")
                        .small()
                        .color(muted),
                );
            });
        });
        ui.add_space(24.0);
        ui.label(RichText::new("   KATEGORİLER").small().color(muted));
        ui.add_space(2.0);
        for cat in Category::ALL {
            let (n, size) = self.totals(Some(cat), false);
            if nav_item(ui, self.tab == cat, icon(cat), cat.label(), n, size).clicked() {
                self.tab = cat;
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
        if let Some((_, progress)) = &self.scanning {
            let n = progress.load(Ordering::Relaxed);
            empty_state(ui, |ui| {
                ui.add(egui::Spinner::new().size(42.0).color(ACCENT));
                ui.add_space(10.0);
                ui.label(RichText::new("Taranıyor…").size(20.0).strong());
                ui.label(
                    RichText::new(format!(
                        "{n} dosya incelendi. Kopyalar için içerikler karşılaştırılıyor."
                    ))
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
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}  {}", icon(tab), tab.label()))
                    .size(20.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Seçimi kaldır").clicked() {
                    self.items
                        .iter_mut()
                        .filter(|i| i.category == tab)
                        .for_each(|i| i.selected = false);
                }
                if ui.button("Tümünü seç").clicked() {
                    self.items
                        .iter_mut()
                        .filter(|i| i.category == tab)
                        .for_each(|i| i.selected = true);
                }
            });
        });
        ui.label(RichText::new(description(tab)).color(muted));
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
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(format!("{n} dosya seçildi")).strong());
                    let status = if self.status.is_empty() {
                        "Dosyalar çöp kutusuna taşınır, istediğinizde geri alabilirsiniz."
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
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.autostart) {
            self.start_scan(ctx.clone());
        }
        self.poll_deletion();
        if self.deleting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some((rx, progress)) = &self.scanning {
            if let Ok(items) = rx.try_recv() {
                self.scanned_files = progress.load(Ordering::Relaxed);
                self.telemetry.scan_done("gui", self.scanned_files, &items);
                self.items = items;
                self.scanning = None;
                self.has_scanned = true;
                // Open the first category that has something in it.
                if let Some(cat) = Category::ALL
                    .into_iter()
                    .find(|&c| self.items.iter().any(|i| i.category == c))
                {
                    self.tab = cat;
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        let (bg, side, border, _) = palette(ui.visuals().dark_mode);
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
                self.header(ui, &ctx);
                ui.add_space(18.0);
                self.content(ui);
            });

        if self.confirm {
            let (n, size) = self.totals(None, true);
            let modal = egui::Modal::new(egui::Id::new("confirm")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("🗑  Dosyalar temizlensin mi?").size(19.0).strong());
                ui.add_space(4.0);
                ui.label(format!("{n} dosya ({}) çöp kutusuna taşınacak. İsterseniz çöp kutusundan geri alabilirsiniz.", human_size(size)));
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

        if self.telemetry.needs_prompt() {
            egui::Modal::new(egui::Id::new("consent")).frame(egui::Frame::popup(ui.style()).inner_margin(Margin::same(24))).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("📊  Anonim kullanım istatistikleri").size(19.0).strong());
                ui.add_space(4.0);
                ui.label("cleanSpace'i geliştirmek için tarama ve temizleme sayıları (dosya sayısı, boyut, işletim sistemi) gönderilsin mi?");
                ui.label(RichText::new("Dosya adları ve yolları asla gönderilmez.").strong());
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
    size: u64,
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
        human_size(size),
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
    let name = item
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
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
