#![cfg_attr(windows, windows_subsystem = "windows")] // no console window on Windows

use cleanspace::scan::{Category, Item, human_size};
use cleanspace::telemetry::Telemetry;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CleanSpace",
        options,
        Box::new(|_| Ok(Box::new(App::default()))),
    )
}

struct App {
    root: String,
    items: Vec<Item>,
    tab: Category,
    scanning: Option<(Receiver<Vec<Item>>, Arc<AtomicUsize>)>,
    confirm: bool,
    status: String,
    telemetry: Telemetry,
}

impl Default for App {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            root: home.display().to_string(),
            items: Vec::new(),
            tab: Category::Duplicate,
            scanning: None,
            confirm: false,
            status: String::new(),
            telemetry: Telemetry::load(),
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

    fn delete_selected(&mut self) {
        let (ok, failed, freed) = cleanspace::delete_selected(&mut self.items, false);
        self.telemetry.clean_done("gui", ok, freed);
        self.status = format!("{ok} dosya çöp kutusuna taşındı ({})", human_size(freed));
        if failed > 0 {
            self.status += &format!(", {failed} dosya taşınamadı");
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some((rx, progress)) = &self.scanning {
            if let Ok(items) = rx.try_recv() {
                self.telemetry
                    .scan_done("gui", progress.load(Ordering::Relaxed), &items);
                self.items = items;
                self.scanning = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Klasör:");
                ui.add(egui::TextEdit::singleline(&mut self.root).desired_width(500.0));
                let scan = ui.add_enabled(self.scanning.is_none(), egui::Button::new("Tara"));
                if scan.clicked() {
                    self.start_scan(ctx.clone());
                }
                if let Some((_, progress)) = &self.scanning {
                    ui.spinner();
                    ui.label(format!(
                        "{} dosya tarandı…",
                        progress.load(Ordering::Relaxed)
                    ));
                }
            });
            ui.horizontal(|ui| {
                for cat in Category::ALL {
                    let (count, size) = self
                        .items
                        .iter()
                        .filter(|i| i.category == cat)
                        .fold((0, 0), |(c, s), i| (c + 1, s + i.size));
                    let text = format!("{} ({count}, {})", cat.label(), human_size(size));
                    ui.selectable_value(&mut self.tab, cat, text);
                }
            });
            ui.add_space(4.0);
        });

        let selected: Vec<&Item> = self.items.iter().filter(|i| i.selected).collect();
        let selected_size: u64 = selected.iter().map(|i| i.size).sum();
        let selected_count = selected.len();

        egui::Panel::bottom("bottom").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong(format!(
                    "Seçili: {selected_count} dosya — Kazanılacak alan: {}",
                    human_size(selected_size)
                ));
                let del =
                    ui.add_enabled(selected_count > 0, egui::Button::new("Çöp kutusuna taşı"));
                if del.clicked() {
                    self.confirm = true;
                }
                ui.label(&self.status);
                if cleanspace::telemetry::URL.is_some() {
                    let mut on = self.telemetry.consent == Some(true);
                    if ui.checkbox(&mut on, "Anonim istatistik gönder").changed() {
                        self.telemetry.set_consent(on);
                    }
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let tab = self.tab;
            let idx: Vec<usize> = (0..self.items.len())
                .filter(|&i| self.items[i].category == tab)
                .collect();
            ui.horizontal(|ui| {
                if ui.button("Tümünü seç").clicked() {
                    idx.iter().for_each(|&i| self.items[i].selected = true);
                }
                if ui.button("Seçimi kaldır").clicked() {
                    idx.iter().for_each(|&i| self.items[i].selected = false);
                }
            });
            ui.separator();
            let row_h = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                row_h,
                idx.len(),
                |ui, range| {
                    for &i in &idx[range] {
                        let item = &mut self.items[i];
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut item.selected, "");
                            ui.add_sized([80.0, row_h], egui::Label::new(human_size(item.size)));
                            if item.group > 0 {
                                ui.weak(format!("#{}", item.group));
                            }
                            ui.label(item.path.display().to_string());
                        });
                    }
                },
            );
        });

        if self.confirm {
            let modal = egui::Modal::new(egui::Id::new("confirm")).show(&ctx, |ui| {
                ui.heading("Emin misiniz?");
                ui.label(format!(
                    "{selected_count} dosya ({}) çöp kutusuna taşınacak.",
                    human_size(selected_size)
                ));
                ui.horizontal(|ui| {
                    if ui.button("Taşı").clicked() {
                        self.delete_selected();
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
            egui::Modal::new(egui::Id::new("consent")).show(&ctx, |ui| {
                ui.heading("Anonim kullanım istatistikleri");
                ui.label("CleanSpace'i geliştirmek için tarama ve temizleme sayıları\n(dosya sayısı, boyut, işletim sistemi) gönderilsin mi?\nDosya adları ve yolları asla gönderilmez.");
                ui.horizontal(|ui| {
                    if ui.button("Evet").clicked() {
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
