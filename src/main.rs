mod config;
mod launcher;

use config::{valid_nickname, Config};
use eframe::egui::{
    self, Align2, Color32, FontId, Frame, Margin, RichText, Rounding, Sense, Stroke, Vec2,
};
use launcher::{Progress, Shared, VersionEntry, VersionList};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const BLACK: Color32 = Color32::BLACK;
const WHITE: Color32 = Color32::WHITE;
const GRAY: Color32 = Color32::from_gray(130);
const DIM: Color32 = Color32::from_gray(40);
const SOFT: Color32 = Color32::from_gray(18);

type VersionSlot = Arc<Mutex<Option<Result<VersionList, String>>>>;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Play,
    Settings,
    About,
}

struct App {
    cfg: Config,
    versions: Vec<VersionEntry>,
    latest_release: String,
    version_slot: VersionSlot,
    version_error: Option<String>,
    loading: bool,
    tab: Tab,
    progress: Shared,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_style(&cc.egui_ctx);
        let mut app = App {
            cfg: Config::load(),
            versions: Vec::new(),
            latest_release: String::new(),
            version_slot: Arc::new(Mutex::new(None)),
            version_error: None,
            loading: true,
            tab: Tab::Play,
            progress: Arc::new(Mutex::new(Progress::default())),
        };
        app.fetch_versions();
        app
    }

    fn fetch_versions(&mut self) {
        self.loading = true;
        self.version_error = None;
        let slot = self.version_slot.clone();
        thread::spawn(move || {
            let result = launcher::fetch_versions().map_err(|e| format!("{e:#}"));
            if let Ok(mut s) = slot.lock() {
                *s = Some(result);
            }
        });
    }

    fn poll_versions(&mut self) {
        let taken = self.version_slot.lock().ok().and_then(|mut s| s.take());
        if let Some(result) = taken {
            self.loading = false;
            match result {
                Ok(list) => {
                    self.latest_release = list.latest_release;
                    self.versions = list.versions;
                    if !self.versions.iter().any(|v| v.id == self.cfg.version) {
                        self.cfg.version = self.latest_release.clone();
                    }
                }
                Err(e) => self.version_error = Some(e),
            }
        }
    }

    fn visible_versions(&self) -> Vec<&VersionEntry> {
        self.versions
            .iter()
            .filter(|v| self.cfg.show_all_versions || v.kind == "release")
            .collect()
    }

    fn start(&mut self) {
        let Some(entry) = self
            .versions
            .iter()
            .find(|v| v.id == self.cfg.version)
            .cloned()
        else {
            return;
        };
        self.cfg.save();
        if let Ok(mut p) = self.progress.lock() {
            *p = Progress {
                busy: true,
                message: "Starting".into(),
                ..Default::default()
            };
        }
        let cfg = self.cfg.clone();
        let progress = self.progress.clone();
        thread::spawn(move || {
            let result = launcher::run(&cfg, &entry, &progress);
            if let Ok(mut p) = progress.lock() {
                p.busy = false;
                p.done = 0;
                p.total = 0;
                match result {
                    Ok(()) => p.message = "Game closed".into(),
                    Err(e) => {
                        p.message.clear();
                        p.error = Some(format!("{e:#}"));
                    }
                }
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(
            RichText::new("NIX")
                .font(FontId::monospace(46.0))
                .strong()
                .color(WHITE),
        );
        ui.label(RichText::new("L A U N C H E R").size(11.0).color(GRAY));
        ui.add_space(48.0);
        for (tab, label) in [
            (Tab::Play, "PLAY"),
            (Tab::Settings, "SETTINGS"),
            (Tab::About, "ABOUT"),
        ] {
            let selected = self.tab == tab;
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
            let color = if selected || response.hovered() {
                WHITE
            } else {
                GRAY
            };
            if selected {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        rect.left_center() - Vec2::new(0.0, 8.0),
                        Vec2::new(3.0, 16.0),
                    ),
                    Rounding::ZERO,
                    WHITE,
                );
            }
            ui.painter().text(
                rect.left_center() + Vec2::new(16.0, 0.0),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(14.0),
                color,
            );
            if response.clicked() {
                self.tab = tab;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.0)
                    .color(GRAY),
            );
        });
    }

    fn play_page(&mut self, ui: &mut egui::Ui, snap: &Progress) {
        section_title(ui, "PLAY");
        ui.add_space(28.0);

        field_label(ui, "NICKNAME");
        ui.add_enabled(
            !snap.busy,
            egui::TextEdit::singleline(&mut self.cfg.nickname)
                .hint_text(RichText::new("Steve").color(GRAY))
                .char_limit(16)
                .desired_width(340.0)
                .font(FontId::proportional(18.0))
                .margin(Margin::symmetric(12.0, 10.0)),
        );
        let nick_ok = valid_nickname(&self.cfg.nickname);
        if !self.cfg.nickname.is_empty() && !nick_ok {
            ui.label(
                RichText::new("3 to 16 characters: letters, numbers and underscore")
                    .size(11.0)
                    .color(GRAY),
            );
        }
        ui.add_space(22.0);

        field_label(ui, "VERSION");
        let visible: Vec<String> = self
            .visible_versions()
            .iter()
            .map(|v| v.id.clone())
            .collect();
        ui.add_enabled_ui(!snap.busy, |ui| {
            egui::ComboBox::from_id_salt("version")
                .width(340.0)
                .selected_text(if self.loading {
                    "Loading...".to_string()
                } else {
                    self.cfg.version.clone()
                })
                .show_ui(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for id in &visible {
                                ui.selectable_value(&mut self.cfg.version, id.clone(), id);
                            }
                        });
                });
        });
        ui.add_space(8.0);
        let before = self.cfg.show_all_versions;
        ui.add_enabled(
            !snap.busy,
            egui::Checkbox::new(
                &mut self.cfg.show_all_versions,
                RichText::new("Show snapshots and old versions")
                    .size(12.0)
                    .color(GRAY),
            ),
        );
        if before != self.cfg.show_all_versions && !self.cfg.show_all_versions {
            let list = self.visible_versions();
            if !list.iter().any(|v| v.id == self.cfg.version) {
                self.cfg.version = self.latest_release.clone();
            }
        }
        if let Some(err) = self.version_error.clone() {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Could not load versions: {err}"))
                    .size(12.0)
                    .color(WHITE),
            );
            if big_button(ui, "RETRY", true, 120.0, 34.0).clicked() {
                self.fetch_versions();
            }
        }

        ui.add_space(32.0);
        let can_play = nick_ok && !snap.busy && !self.loading && self.version_error.is_none();
        let label = if snap.busy { "RUNNING" } else { "PLAY" };
        if big_button(ui, label, can_play, 340.0, 56.0).clicked() && can_play {
            self.start();
        }

        ui.add_space(24.0);
        if snap.total > 0 {
            progress_bar(ui, snap.done as f32 / snap.total as f32, 340.0);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{}  {}/{}", snap.message, snap.done, snap.total))
                    .size(12.0)
                    .color(GRAY),
            );
        } else if snap.busy {
            progress_bar(ui, -1.0, 340.0);
            ui.add_space(8.0);
            ui.label(RichText::new(&snap.message).size(12.0).color(GRAY));
        } else if !snap.message.is_empty() {
            ui.label(RichText::new(&snap.message).size(12.0).color(GRAY));
        }
        if let Some(err) = &snap.error {
            ui.add_space(8.0);
            ui.label(RichText::new("ERROR").size(11.0).color(GRAY));
            ui.add(egui::Label::new(RichText::new(err).size(12.0).color(WHITE)).wrap());
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "SETTINGS");
        ui.add_space(28.0);
        field_label(ui, "MEMORY");
        let mut ram = self.cfg.ram_mb;
        ui.add(
            egui::Slider::new(&mut ram, 1024..=16384)
                .step_by(512.0)
                .suffix(" MB"),
        );
        self.cfg.ram_mb = ram;
        ui.add_space(22.0);
        field_label(ui, "JAVA EXECUTABLE");
        ui.add(
            egui::TextEdit::singleline(&mut self.cfg.java_path)
                .hint_text(RichText::new("Automatic").color(GRAY))
                .desired_width(340.0)
                .margin(Margin::symmetric(12.0, 8.0)),
        );
        ui.label(
            RichText::new(
                "Leave empty to use a system Java or download the right one automatically",
            )
            .size(11.0)
            .color(GRAY),
        );
        ui.add_space(22.0);
        field_label(ui, "GAME FOLDER");
        ui.label(
            RichText::new(config::data_dir().display().to_string())
                .size(12.0)
                .color(GRAY),
        );
        ui.add_space(8.0);
        if big_button(ui, "OPEN FOLDER", true, 160.0, 34.0).clicked() {
            let dir = config::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
        self.cfg.save();
    }

    fn about_page(&self, ui: &mut egui::Ui) {
        section_title(ui, "ABOUT");
        ui.add_space(28.0);
        ui.label(RichText::new("Nix Launcher").size(20.0).color(WHITE));
        ui.add_space(6.0);
        ui.label(
            RichText::new("A minimalist black and white Minecraft launcher for Linux.").color(GRAY),
        );
        ui.add_space(18.0);
        ui.label(
            RichText::new("Offline mode only for now. Microsoft account sign-in is planned.")
                .color(GRAY),
        );
        ui.add_space(18.0);
        ui.hyperlink_to(
            "github.com/Bobistuca/-nix-Launchear",
            "https://github.com/Bobistuca/-nix-Launchear",
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("Open source under the MIT license.")
                .size(12.0)
                .color(GRAY),
        );
        ui.add_space(18.0);
        ui.label(
            RichText::new("Not affiliated with Mojang or Microsoft.")
                .size(11.0)
                .color(GRAY),
        );
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_versions();
        let snap = self.progress.lock().map(|p| p.clone()).unwrap_or_default();
        if snap.busy || self.loading {
            ctx.request_repaint_after(Duration::from_millis(60));
        }

        egui::SidePanel::left("sidebar")
            .exact_width(230.0)
            .resizable(false)
            .frame(Frame::none().fill(BLACK).inner_margin(Margin::same(28.0)))
            .show(ctx, |ui| {
                let rect = ui.clip_rect();
                ui.painter().vline(
                    rect.right() - 0.5,
                    rect.y_range(),
                    Stroke::new(1.0_f32, DIM),
                );
                self.sidebar(ui);
            });

        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(BLACK)
                    .inner_margin(Margin::symmetric(56.0, 44.0)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Play => self.play_page(ui, &snap),
                Tab::Settings => self.settings_page(ui),
                Tab::About => self.about_page(ui),
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cfg.save();
    }
}

fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(FontId::monospace(30.0))
            .strong()
            .color(WHITE),
    );
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(GRAY));
    ui.add_space(4.0);
}

fn big_button(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    let hovered = enabled && response.hovered();
    let (fill, fg, stroke) = if !enabled {
        (BLACK, Color32::from_gray(70), Color32::from_gray(50))
    } else if hovered {
        (BLACK, WHITE, WHITE)
    } else {
        (WHITE, BLACK, WHITE)
    };
    ui.painter()
        .rect(rect, Rounding::ZERO, fill, Stroke::new(1.0_f32, stroke));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(height * 0.28),
        fg,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

fn progress_bar(ui: &mut egui::Ui, fraction: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 3.0), Sense::hover());
    ui.painter().rect_filled(rect, Rounding::ZERO, DIM);
    let bar = if fraction < 0.0 {
        let t = ui.input(|i| i.time) as f32;
        let w = width * 0.25;
        let x = rect.left() + (t * 0.8 % 1.0) * (width - w);
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(w, 3.0))
    } else {
        egui::Rect::from_min_size(rect.min, Vec2::new(width * fraction.clamp(0.0, 1.0), 3.0))
    };
    ui.painter().rect_filled(bar, Rounding::ZERO, WHITE);
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(WHITE);
    v.panel_fill = BLACK;
    v.window_fill = BLACK;
    v.extreme_bg_color = BLACK;
    v.faint_bg_color = SOFT;
    v.window_stroke = Stroke::new(1.0_f32, DIM);
    v.window_rounding = Rounding::ZERO;
    v.menu_rounding = Rounding::ZERO;
    v.selection.bg_fill = WHITE;
    v.selection.stroke = Stroke::new(1.0_f32, BLACK);
    v.hyperlink_color = WHITE;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::ZERO;
        w.bg_fill = BLACK;
        w.weak_bg_fill = BLACK;
        w.bg_stroke = Stroke::new(1.0_f32, Color32::from_gray(90));
        w.fg_stroke = Stroke::new(1.0_f32, WHITE);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, DIM);
    v.widgets.hovered.bg_fill = SOFT;
    v.widgets.hovered.weak_bg_fill = SOFT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, WHITE);
    v.widgets.active.bg_fill = DIM;
    v.widgets.active.weak_bg_fill = DIM;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, WHITE);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.interact_size.y = 28.0;
    });
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Nix Launcher")
            .with_app_id("nix-launcher")
            .with_inner_size([900.0, 580.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Nix Launcher",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
