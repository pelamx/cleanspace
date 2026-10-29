//! Terminal UI for servers / cloud machines: `deskmonk-tui [FOLDER]` (default: home).

use deskmonk::scan::{Category, Item, Phase, human_size};
use deskmonk::telemetry::{self, Telemetry};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Tabs};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum Prompt {
    None,
    Consent,
    Trash,
    Delete,
}

struct App {
    root: PathBuf,
    items: Vec<Item>,
    tab: usize,
    list: ListState,
    /// Messages are (final, items): first the partial list without duplicates, then the final one.
    scanning: Option<(Receiver<(Option<Phase>, Vec<Item>)>, Arc<AtomicUsize>)>,
    /// Phase of the partial results on screen.
    phase: Option<Phase>,
    prompt: Prompt,
    deleting: Option<deskmonk::Deletion>,
    status: String,
    telemetry: Telemetry,
}

fn main() -> io::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    if !root.is_dir() {
        eprintln!("Klasör bulunamadı: {}", root.display());
        std::process::exit(1);
    }
    let telemetry = Telemetry::load();
    let prompt = if telemetry.needs_prompt() {
        Prompt::Consent
    } else {
        Prompt::None
    };
    let mut app = App {
        root,
        items: Vec::new(),
        tab: 0,
        list: ListState::default(),
        scanning: None,
        phase: None,
        prompt,
        deleting: None,
        status: String::new(),
        telemetry,
    };
    app.start_scan();
    app.send_opened();
    ratatui::run(|t| app.run(t))
}

impl App {
    /// Hardware detection spawns system tools, so it runs off the UI thread.
    fn send_opened(&self) {
        if self.telemetry.consent == Some(true) {
            let telemetry = self.telemetry.clone();
            std::thread::spawn(move || telemetry.opened("tui", &deskmonk::hardware::detect()));
        }
    }

    fn start_scan(&mut self) {
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (root, counter) = (self.root.clone(), progress.clone());
        std::thread::spawn(move || {
            let partial_tx = tx.clone();
            let items = deskmonk::scan_default(&root, &counter, move |items, phase| {
                let _ = partial_tx.send((Some(phase), items));
            });
            tx.send((None, items))
        });
        self.items.clear();
        self.scanning = Some((rx, progress));
    }

    fn visible(&self) -> Vec<usize> {
        let cat = Category::ALL[self.tab];
        (0..self.items.len())
            .filter(|&i| self.items[i].category == cat)
            .collect()
    }

    fn selected_totals(&self) -> (usize, u64) {
        self.items
            .iter()
            .filter(|i| i.selected)
            .fold((0, 0), |(c, s), i| (c + 1, s + i.size))
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            while let Some((rx, progress)) = &self.scanning
                && let Ok((phase, mut items)) = rx.try_recv()
            {
                self.phase = phase;
                if phase.is_none() {
                    self.telemetry
                        .scan_done("tui", progress.load(Ordering::Relaxed), &items);
                    self.scanning = None;
                }
                deskmonk::keep_selection(&self.items, &mut items);
                if self.items.is_empty() {
                    self.list.select(Some(0));
                }
                self.items = items;
            }
            if let Some(done) = self.deleting.as_ref().and_then(|d| d.poll()) {
                let ok = self.deleting.take().unwrap().total - done.failed.len();
                self.telemetry.clean_done("tui", ok, done.freed);
                self.status = format!("{ok} öğe temizlendi ({})", human_size(done.freed));
                if let Some(err) = done.first_error {
                    self.status += &format!(", {} tamamen silinemedi: {err}", done.failed.len());
                }
                self.items.extend(done.failed);
            }
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let yes = matches!(key.code, KeyCode::Char('e' | 'E' | 'y' | 'Y'));
            match std::mem::replace(&mut self.prompt, Prompt::None) {
                Prompt::Consent => match key.code {
                    _ if yes => {
                        self.telemetry.set_consent(true);
                        self.send_opened();
                    }
                    KeyCode::Char('h' | 'H' | 'n' | 'N') | KeyCode::Esc => {
                        self.telemetry.set_consent(false)
                    }
                    _ => self.prompt = Prompt::Consent,
                },
                p @ (Prompt::Trash | Prompt::Delete) => {
                    if yes {
                        // Runs in the background; the screen keeps updating with progress.
                        self.deleting = Some(deskmonk::Deletion::start(
                            &mut self.items,
                            p == Prompt::Delete,
                            || {},
                        ));
                        self.status.clear();
                    } else {
                        self.status = "Vazgeçildi".into();
                    }
                }
                // Quitting, rescanning or a second deletion must wait for the running one.
                Prompt::None
                    if self.deleting.is_some()
                        && matches!(
                            key.code,
                            KeyCode::Char('q' | 'd' | 'D' | 'r') | KeyCode::Esc
                        ) =>
                {
                    self.status = "Silme sürüyor, lütfen bekleyin…".into();
                }
                Prompt::None => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Tab | KeyCode::Right => {
                        self.tab = (self.tab + 1) % Category::ALL.len();
                        self.list.select(Some(0));
                    }
                    KeyCode::BackTab | KeyCode::Left => {
                        self.tab = (self.tab + Category::ALL.len() - 1) % Category::ALL.len();
                        self.list.select(Some(0));
                    }
                    KeyCode::Down | KeyCode::Char('j') => self.list.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => self.list.select_previous(),
                    KeyCode::PageDown => self.list.scroll_down_by(20),
                    KeyCode::PageUp => self.list.scroll_up_by(20),
                    KeyCode::Char(' ') => {
                        if let Some(i) = self
                            .list
                            .selected()
                            .and_then(|s| self.visible().get(s).copied())
                        {
                            self.items[i].selected ^= true;
                            self.list.select_next();
                        }
                    }
                    KeyCode::Char(c @ ('a' | 'n')) => deskmonk::set_selection(
                        &mut self.items,
                        Some(Category::ALL[self.tab]),
                        c == 'a',
                    ),
                    // The final scan list would bring deleted items back, so deleting waits for it.
                    KeyCode::Char('d' | 'D') if self.scanning.is_some() => {
                        self.status =
                            "Kopyalar hâlâ karşılaştırılıyor, tarama bitince silebilirsiniz.".into()
                    }
                    KeyCode::Char('d') if self.selected_totals().0 > 0 => {
                        self.prompt = Prompt::Trash
                    }
                    KeyCode::Char('D') if self.selected_totals().0 > 0 => {
                        self.prompt = Prompt::Delete
                    }
                    KeyCode::Char('r') if self.scanning.is_none() => self.start_scan(),
                    KeyCode::Char('t') if telemetry::URL.is_some() => {
                        let on = self.telemetry.consent != Some(true);
                        self.telemetry.set_consent(on);
                        self.status =
                            format!("Anonim istatistik: {}", if on { "açık" } else { "kapalı" });
                    }
                    _ => {}
                },
            }
        }
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let [header, tabs, body, footer, help] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(f.area());

        let scanning = match (&self.scanning, &self.deleting) {
            (Some(_), _) if self.phase == Some(Phase::Comparing) => {
                "  — kopyalar karşılaştırılıyor…".to_string()
            }
            (Some((_, p)), _) => format!("  — taranıyor… {} dosya", p.load(Ordering::Relaxed)),
            (_, Some(d)) => format!(
                "  — siliniyor… {}/{}",
                d.done.load(Ordering::Relaxed),
                d.total
            ),
            _ => String::new(),
        };
        f.render_widget(
            Paragraph::new(format!(
                "deskMonk by pelamx  {}{scanning}",
                self.root.display()
            ))
            .bold(),
            header,
        );

        let titles = Category::ALL.map(|cat| {
            let (n, size) = self
                .items
                .iter()
                .filter(|i| i.category == cat)
                .fold((0, 0), |(c, s), i| (c + 1, s + i.size));
            format!("{} ({n}, {})", cat.label(), human_size(size))
        });
        f.render_widget(
            Tabs::new(titles)
                .select(self.tab)
                .highlight_style(Style::new().reversed()),
            tabs,
        );

        // shortcut: builds every row each frame; fine up to ~100k items, switch to a windowed render if lists get bigger.
        let rows: Vec<ListItem> = self
            .visible()
            .into_iter()
            .map(|i| {
                let it = &self.items[i];
                let group = if it.group > 0 {
                    format!("#{:<4} ", it.group)
                } else {
                    String::new()
                };
                ListItem::new(format!(
                    "[{}] {:>10}  {group}{}",
                    if it.selected { "x" } else { " " },
                    human_size(it.size),
                    it.path.display()
                ))
            })
            .collect();
        let list = List::new(rows)
            .block(Block::bordered())
            .highlight_style(Style::new().reversed());
        f.render_stateful_widget(list, body, &mut self.list);

        let (n, size) = self.selected_totals();
        f.render_widget(
            Paragraph::new(format!(
                "Seçili: {n} dosya — Kazanılacak alan: {}   {}",
                human_size(size),
                self.status
            ))
            .bold(),
            footer,
        );
        let tkey = if telemetry::URL.is_some() {
            "  t istatistik"
        } else {
            ""
        };
        f.render_widget(Paragraph::new(format!("↑↓ gezin  boşluk seç  a/n tümü/hiçbiri  ←→ kategori  d temizle  D hepsini kalıcı sil  r yeniden tara{tkey}  q çık")).dim(), help);

        let text = match self.prompt {
            Prompt::None => return,
            Prompt::Consent => "Anonim kullanım istatistikleri (dosya sayısı, boyut, işletim sistemi) ve donanım bilgileri\n(işlemci, bellek, disk, ekran kartı, pil: marka, model, doluluk) gönderilsin mi?\nDosya adları, yollar, seri numaraları ve kullanıcı adı asla gönderilmez.  [e]vet / [h]ayır".to_string(),
            Prompt::Trash => format!("{n} öğe ({}) temizlenecek: gereksiz dosyalar kalıcı silinir, diğerleri çöp kutusuna taşınır. Emin misiniz? [e/h]", human_size(size)),
            Prompt::Delete => format!("{n} dosya ({}) KALICI olarak silinecek, geri alınamaz! Emin misiniz? [e/h]", human_size(size)),
        };
        let area = centered(f.area(), 70, 5);
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .block(Block::bordered().title(" Onay ")),
            area,
        );
    }
}

fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, mid, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Fill(1),
    ])
    .areas(mid);
    mid
}
