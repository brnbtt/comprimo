use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Layout, Margin, RichText, Stroke, Vec2};

use crate::blocks::{self, BlockGrid};
use crate::config::{Config, MAX_PRESETS, Preset, fmt_mb, human_size};
use crate::encode::{self, Event, JobError, Outcome, Stage};
use crate::paths::APP;
use crate::sound::{self, Cue};
use crate::theme::{self, BG, BORDER_STRONG, TEXT, TEXT_2, TEXT_3, light, semibold};
use crate::{ffmpeg, icon, install, paths, win};

// ---------------------------------------------------------------------------------------------
// Background jobs
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Phase<T> {
    Running,
    Done(T),
    Failed(String),
    Cancelled,
}

#[derive(Clone)]
struct Snapshot<T> {
    status: String,
    detail: String,
    source: String,
    progress: f32,
    stage: Stage,
    attempt: usize,
    phase: Phase<T>,
}

/// A job on a worker thread whose progress the UI polls each frame.
struct Task<T> {
    state: Arc<Mutex<Snapshot<T>>>,
    cancel: Arc<AtomicBool>,
    _worker: JoinHandle<()>,
}

impl<T: Clone + Send + 'static> Task<T> {
    fn spawn(
        ctx: &egui::Context,
        job: impl FnOnce(&mut dyn FnMut(Event), &AtomicBool) -> Result<T, JobError> + Send + 'static,
    ) -> Self {
        let state = Arc::new(Mutex::new(Snapshot {
            status: "starting".into(),
            detail: String::new(),
            source: String::new(),
            progress: 0.0,
            stage: Stage::Preparing,
            attempt: 0,
            phase: Phase::Running,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let (state, cancel, ctx) = (state.clone(), cancel.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut report = |ev: Event| {
                    {
                        let mut s = state.lock().unwrap();
                        match ev {
                            Event::Status(t) => s.status = t,
                            Event::Progress(p) => s.progress = p,
                            Event::Detail(d) => s.detail = d,
                            Event::Source(src) => s.source = src,
                            Event::Stage { stage, attempt } => (s.stage, s.attempt) = (stage, attempt),
                        }
                    }
                    ctx.request_repaint();
                };
                let result = job(&mut report, &cancel);
                state.lock().unwrap().phase = match result {
                    Ok(v) => Phase::Done(v),
                    Err(JobError::Cancelled) => Phase::Cancelled,
                    Err(JobError::Failed(e)) => Phase::Failed(e),
                };
                ctx.request_repaint();
            })
        };
        Task { state, cancel, _worker: worker }
    }

    fn snapshot(&self) -> Snapshot<T> {
        self.state.lock().unwrap().clone()
    }
}

/// Only one video is encoded at a time; extra right-clicks wait their turn.
fn acquire_queue_lock(report: &mut dyn FnMut(Event)) -> Result<File, JobError> {
    std::fs::create_dir_all(paths::install_dir())?;
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(paths::lock_file())?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            report(Event::Status("waiting for other videos".into()));
            file.lock()?;
        }
        Err(TryLockError::Error(e)) => return Err(e.into()),
    }
    Ok(file)
}

// ---------------------------------------------------------------------------------------------
// Window chrome
// ---------------------------------------------------------------------------------------------

fn native_options(title: &str, size: [f32; 2]) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 }),
        centered: true,
        ..Default::default()
    }
}

/// Tints the native title bar to match the window, once.
fn style_chrome(frame: &eframe::Frame, done: &mut bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if *done {
        return;
    }
    *done = true;
    if let Ok(handle) = frame.window_handle()
        && let RawWindowHandle::Win32(h) = handle.as_raw()
    {
        win::style_title_bar(h.hwnd.get() as *mut _, BG.r(), 34, TEXT_3.r());
    }
}

fn panel() -> egui::CentralPanel {
    egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(22)))
}

// ---------------------------------------------------------------------------------------------
// Compress window
// ---------------------------------------------------------------------------------------------

const COMPRESS_SIZE: [f32; 2] = [480.0, 238.0];
/// Width of the column left of the block grid (percentage / final size).
const HERO_WIDTH: f32 = 118.0;

pub fn run_compress(input: PathBuf, target_mb: f64, sounds: bool) -> Result<(), String> {
    sound::init(sounds);
    let name = input.file_name().unwrap_or_default().to_string_lossy().to_string();
    let title = format!("{APP} — {name}");
    eframe::run_native(
        &title,
        native_options(&title, COMPRESS_SIZE),
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let task = Task::spawn(&cc.egui_ctx, move |report, cancel| {
                let _lock = acquire_queue_lock(report)?;
                let tools = ffmpeg::ensure(report, cancel)?;
                encode::compress(&tools, &input, target_mb, report, cancel)
            });
            Ok(Box::new(CompressApp {
                name,
                target_mb,
                task,
                logo: theme::load_logo(&cc.egui_ctx),
                styled: false,
                shown: 0.0,
                eta: Eta::default(),
                cues: CueTracker::default(),
                grid: BlockGrid::default(),
                variant: 0,
                done_played: false,
                done_since: None,
                scan_started: None,
                finished_at: None,
                closing_since: None,
                close_at: None,
                copied: false,
                resized: false,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

struct CompressApp {
    name: String,
    target_mb: f64,
    task: Task<Outcome>,
    logo: egui::TextureHandle,
    styled: bool,
    /// Displayed percentage, eased towards the real value.
    shown: f32,
    eta: Eta,
    cues: CueTracker,
    grid: BlockGrid,
    /// Rotates through the block tick variations.
    variant: u8,
    done_played: bool,
    done_since: Option<Instant>,
    /// When the finishing scan began (egui time).
    scan_started: Option<f64>,
    /// When the finished state was revealed (drives the check animation).
    finished_at: Option<Instant>,
    closing_since: Option<Instant>,
    close_at: Option<Instant>,
    copied: bool,
    resized: bool,
}

fn is_encoding(stage: Stage) -> bool {
    matches!(stage, Stage::Pass1 | Stage::Pass2 | Stage::Single)
}

/// Start / retry / error / cancel cues. Block ticks and "done" come from the grid animation.
#[derive(Default)]
struct CueTracker {
    attempt: Option<usize>,
    ended: bool,
}

impl CueTracker {
    fn update<T>(&mut self, s: &Snapshot<T>) {
        let end_cue = match s.phase {
            Phase::Running | Phase::Done(_) => None,
            Phase::Failed(_) => Some(Cue::Error),
            Phase::Cancelled => Some(Cue::Cancel),
        };
        if let Some(cue) = end_cue {
            if !self.ended {
                self.ended = true;
                sound::play(cue);
            }
            return;
        }
        if !is_encoding(s.stage) || self.attempt == Some(s.attempt) {
            return;
        }
        sound::play(if self.attempt.is_none() { Cue::Start } else { Cue::Retry });
        self.attempt = Some(s.attempt);
    }
}

/// Smoothed "time left" from the rate of progress since the last reset.
#[derive(Default)]
struct Eta {
    anchor: Option<(Instant, f32)>,
    last: f32,
    smooth: Option<f32>,
}

impl Eta {
    fn update(&mut self, p: f32, active: bool) -> Option<f32> {
        if !active {
            self.anchor = None;
            return None;
        }
        let now = Instant::now();
        if self.anchor.is_none() || p + 0.001 < self.last {
            self.anchor = Some((now, p));
            self.smooth = None;
        }
        self.last = p;
        let (t0, p0) = self.anchor?;
        let (dt, dp) = ((now - t0).as_secs_f32(), p - p0);
        if dt < 2.0 || dp < 0.02 {
            return None;
        }
        let estimate = dt / dp * (1.0 - p);
        let s = self.smooth.map_or(estimate, |s| s + (estimate - s) * 0.04);
        self.smooth = Some(s);
        Some(s)
    }
}

fn fmt_eta(secs: f32) -> String {
    match secs {
        s if s < 5.0 => "almost done".into(),
        s if s < 60.0 => format!("about {} s left", ((s / 5.0).round() * 5.0).max(5.0) as u32),
        s if s < 3600.0 => format!("about {} min left", (s / 60.0).round() as u32),
        s => format!("about {:.1} h left", s / 3600.0),
    }
}

/// Big number with an optional caption underneath, left-aligned and vertically centered.
fn hero_text(ui: &mut egui::Ui, big: &str, caption: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(HERO_WIDTH, blocks::HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    let big_font = egui::FontId::new(if big.len() > 5 { 28.0 } else { 34.0 }, light());
    let (big_y, cap_y) =
        if caption.is_empty() { (rect.center().y, 0.0) } else { (rect.center().y - 8.0, rect.center().y + 17.0) };
    painter.text(egui::pos2(rect.left(), big_y), egui::Align2::LEFT_CENTER, big, big_font, TEXT);
    if !caption.is_empty() {
        painter.text(
            egui::pos2(rect.left() + 1.0, cap_y),
            egui::Align2::LEFT_CENTER,
            caption,
            egui::FontId::proportional(12.0),
            TEXT_3,
        );
    }
}

impl CompressApp {
    fn begin_close(&mut self) {
        self.task.cancel.store(true, Ordering::Relaxed);
        self.closing_since.get_or_insert_with(Instant::now);
    }

    fn header(&self, ui: &mut egui::Ui, source: &str) {
        egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                theme::logo(ui, &self.logo, 36.0);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(RichText::new(&self.name).size(15.0).family(semibold()).color(TEXT))
                            .truncate(),
                    );
                    let source = if source.is_empty() { "reading video" } else { source };
                    ui.add(egui::Label::new(RichText::new(source).size(12.0).color(TEXT_3)).truncate());
                });
            },
            |ui| theme::pill(ui, &format!("→ {} MB", fmt_mb(self.target_mb))),
        );
    }

    /// Plays a tick (or a "tock" when a column fills) for blocks that landed this frame.
    fn landing_sounds(&mut self, landings: &[blocks::Landing]) {
        let Some(last) = landings.last() else { return };
        let pan = sound::pan_bucket(last.x);
        if landings.iter().any(|l| l.column_done) {
            sound::play(Cue::Column(pan));
        } else {
            self.variant = (self.variant + 1) % sound::BLOCK_VARIANTS;
            sound::play(Cue::Block(self.variant, pan));
        }
    }

    fn failed(&mut self, ui: &mut egui::Ui, err: &str) {
        let ctx = ui.ctx().clone();
        if !self.resized {
            self.resized = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(COMPRESS_SIZE[0], 300.0)));
        }
        ui.horizontal(|ui| {
            theme::cross_badge(ui, 30.0);
            ui.add_space(4.0);
            ui.label(RichText::new("couldn't compress this video").size(16.0).family(semibold()).color(TEXT));
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(86.0).show(ui, |ui| {
            ui.label(RichText::new(err).size(12.0).color(TEXT_2));
        });
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if ui.add(theme::primary("close")).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if ui.add(theme::secondary("copy error")).clicked() {
                ctx.copy_text(err.to_string());
                sound::play(Cue::Copy);
            }
        });
    }
}

impl eframe::App for CompressApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let s = self.task.snapshot();
        self.cues.update(&s);

        // If the window is hidden the grid can't finish animating; still say "done".
        if let Phase::Done(_) = s.phase {
            let since = *self.done_since.get_or_insert_with(Instant::now);
            if !self.done_played && since.elapsed() > Duration::from_secs(4) {
                self.done_played = true;
                sound::play(Cue::Done);
            }
        }

        // Closing mid-encode: stop ffmpeg and clean up partial files before the window goes away.
        if ctx.input(|i| i.viewport().close_requested()) && matches!(s.phase, Phase::Running) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.begin_close();
        }
        if let Phase::Cancelled = s.phase {
            // Let the cancel cue finish before the process exits.
            self.close_at.get_or_insert_with(|| Instant::now() + Duration::from_millis(350));
        }
        if let Some(since) = self.closing_since
            && since.elapsed() > Duration::from_secs(3)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(at) = self.close_at {
            if Instant::now() >= at {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        style_chrome(frame, &mut self.styled);
        let ctx = ui.ctx().clone();
        let s = self.task.snapshot();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let time = ctx.input(|i| i.time);

        let outcome = match &s.phase {
            Phase::Done(o) => Some(o.clone()),
            _ => None,
        };
        let untouched = outcome.as_ref().is_some_and(|o| o.untouched);
        let progress = match (&outcome, is_encoding(s.stage)) {
            (Some(o), _) => {
                if o.untouched {
                    0.0
                } else {
                    1.0
                }
            }
            (None, true) => s.progress,
            (None, false) => 0.0,
        };
        if progress < self.shown - 0.02 {
            self.shown = progress;
        } else {
            self.shown += (progress - self.shown) * (1.0 - (-dt * 9.0).exp());
        }

        panel().show(ui, |ui| {
            self.header(ui, &s.source);
            ui.add_space(18.0);

            if let Phase::Failed(err) = &s.phase {
                self.failed(ui, err);
                return;
            }

            // Once the file is done and the last block has landed, scan the grid; the result is
            // revealed when the scan finishes.
            if outcome.is_some() && !untouched && self.scan_started.is_none() && self.grid.completed_at().is_some() {
                self.scan_started = Some(time);
                if !self.done_played {
                    self.done_played = true;
                    sound::play(Cue::Finish(self.grid.cols().min(255) as u8));
                }
            }
            let revealed = outcome.is_some()
                && (untouched || self.scan_started.is_some_and(|s| time >= s + self.grid.scan_duration() - 0.1));
            if untouched && !self.done_played {
                self.done_played = true;
                sound::play(Cue::Done);
            }
            if revealed {
                self.finished_at.get_or_insert_with(Instant::now);
            }

            let landings = ui
                .horizontal(|ui| {
                    match (&outcome, revealed) {
                        (Some(o), true) if o.untouched => hero_text(ui, &human_size(o.after), "already small"),
                        (Some(o), true) => {
                            let saved = 100.0 * (1.0 - o.after as f64 / o.before.max(1) as f64);
                            hero_text(ui, &human_size(o.after), &format!("{saved:.0}% smaller"));
                        }
                        _ => hero_text(ui, &format!("{:.0}%", self.shown * 100.0), ""),
                    }
                    self.grid.show(ui, progress, time, self.scan_started)
                })
                .inner;
            self.landing_sounds(&landings);
            ui.add_space(12.0);

            match (&outcome, revealed) {
                (Some(o), true) => {
                    let t = self.finished_at.map_or(1.0, |f| f.elapsed().as_secs_f32() / 0.45);
                    let mut copy_now = false;
                    egui::Sides::new().shrink_left().show(
                        ui,
                        |ui| {
                            theme::check_badge(ui, 18.0, t);
                            let text = if o.untouched {
                                format!("already under {} MB", fmt_mb(self.target_mb))
                            } else {
                                "saved".into()
                            };
                            ui.label(RichText::new(text).size(13.0).color(TEXT));
                        },
                        |ui| {
                            if ui.add(theme::primary("done")).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.add(theme::secondary("show in folder")).clicked() {
                                win::show_in_folder(&o.output);
                            }
                            copy_now = ui.add(theme::secondary("copy file")).clicked();
                        },
                    );
                    if copy_now && win::copy_file_to_clipboard(&o.output) {
                        self.copied = true;
                        sound::play(Cue::Copy);
                    }
                    let note = if self.copied {
                        "copied · paste it anywhere with ctrl+v".to_string()
                    } else if o.untouched {
                        "nothing to do — the original already fits".to_string()
                    } else {
                        format!(
                            "{} · from {}",
                            o.output.file_name().unwrap_or_default().to_string_lossy(),
                            human_size(o.before)
                        )
                    };
                    ui.add(egui::Label::new(RichText::new(note).size(11.5).color(TEXT_3)).truncate());
                    if t < 1.0 {
                        ctx.request_repaint();
                    }
                }
                _ => {
                    let eta = self.eta.update(s.progress, is_encoding(s.stage) && outcome.is_none());
                    let status = if self.closing_since.is_some() {
                        "cancelling"
                    } else if outcome.is_some() {
                        "finishing"
                    } else {
                        s.status.as_str()
                    };
                    egui::Sides::new().shrink_left().show(
                        ui,
                        |ui| {
                            ui.label(RichText::new(status).size(13.0).color(TEXT_2));
                            if let Some(eta) = eta {
                                ui.label(RichText::new(format!("· {}", fmt_eta(eta))).size(13.0).color(TEXT_3));
                            }
                        },
                        |ui| {
                            if outcome.is_none()
                                && ui.add_enabled(self.closing_since.is_none(), theme::secondary("cancel")).clicked()
                            {
                                self.begin_close();
                            }
                        },
                    );
                    ui.add(egui::Label::new(RichText::new(&s.detail).size(11.5).color(TEXT_3)).truncate());
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Settings window
// ---------------------------------------------------------------------------------------------

const SETUP_WIDTH: f32 = 440.0;
const ROW_HEIGHT: f32 = 46.0;

pub fn run_setup() -> Result<(), String> {
    let cfg = Config::load();
    sound::init(cfg.sounds);
    let guess = 420.0 + ROW_HEIGHT * cfg.presets.len().min(6) as f32;
    eframe::run_native(
        APP,
        native_options(APP, [SETUP_WIDTH, guess]),
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(SetupApp {
                saved: cfg.clone(),
                cfg,
                installed: install::is_installed(),
                running_installed: paths::running_from_install_dir(),
                task: None,
                message: None,
                logo: theme::load_logo(&cc.egui_ctx),
                styled: false,
                height: guess,
                changed_at: None,
                saved_at: None,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

struct SetupApp {
    cfg: Config,
    saved: Config,
    installed: bool,
    running_installed: bool,
    task: Option<Task<()>>,
    message: Option<(bool, String)>,
    logo: egui::TextureHandle,
    styled: bool,
    height: f32,
    /// Settings auto-save shortly after the last change.
    changed_at: Option<f64>,
    saved_at: Option<f64>,
}

impl SetupApp {
    fn start_install(&mut self, ctx: &egui::Context) {
        self.cfg = self.cfg.clone().normalized();
        let cfg = self.cfg.clone();
        self.message = None;
        self.task = Some(Task::spawn(ctx, move |report, cancel| {
            report(Event::Status("installing".into()));
            install::install(&cfg)?;
            if ffmpeg::find_local().is_none() {
                let _lock = acquire_queue_lock(report)?;
                ffmpeg::download(report, cancel)?;
            }
            Ok(())
        }));
    }

    fn save(&mut self, now: f64) {
        let cfg = self.cfg.clone().normalized();
        let result = cfg
            .save()
            .map_err(|e| e.to_string())
            .and_then(|_| install::register_menu(&paths::installed_exe(), &cfg).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.saved = self.cfg.clone();
                self.saved_at = Some(now);
                sound::play(Cue::Save);
            }
            Err(e) => {
                sound::play(Cue::Error);
                self.message = Some((false, format!("couldn't save: {e}")));
            }
        }
    }

    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            theme::logo_with_shadow(ui, &self.logo, 52.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(3.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(APP).size(26.0).family(semibold()).color(TEXT));
                ui.label(RichText::new("compress any video to the size you need").size(13.0).color(TEXT_2));
            });
        });
    }

    /// One row of a group: `left` on the left, `right` aligned right, vertically centered.
    fn row(ui: &mut egui::Ui, left: impl FnOnce(&mut egui::Ui), right: impl FnOnce(&mut egui::Ui)) {
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), ROW_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                left(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), right);
            },
        );
    }

    fn titled(ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(title).color(TEXT));
            ui.label(RichText::new(subtitle).size(12.0).color(TEXT_3));
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        theme::caption(ui, "right-click menu");
        theme::group(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            Self::row(
                ui,
                |ui| Self::titled(ui, "default", "the first entry in the menu"),
                |ui| {
                    ui.add_space(30.0);
                    theme::size_stepper(ui, &mut self.cfg.default_mb);
                },
            );
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(ROW_HEIGHT * 6.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, preset) in self.cfg.presets.iter_mut().enumerate() {
                    theme::hairline(ui);
                    let area = egui::Rect::from_min_size(ui.cursor().min, Vec2::new(ui.available_width(), ROW_HEIGHT));
                    let hovered = ui.rect_contains_pointer(area);
                    Self::row(
                        ui,
                        |ui| {
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut preset.name)
                                    .frame(egui::Frame::NONE)
                                    .text_color(TEXT)
                                    .hint_text("name")
                                    .align(egui::Align2::LEFT_CENTER)
                                    .min_size(Vec2::new(0.0, 28.0))
                                    .desired_width(150.0),
                            );
                            if r.hovered() || r.has_focus() {
                                ui.painter().rect_stroke(
                                    r.rect.expand2(Vec2::new(7.0, 0.0)),
                                    6,
                                    Stroke::new(1.0, BORDER_STRONG),
                                    egui::StrokeKind::Outside,
                                );
                            }
                        },
                        |ui| {
                            // The remove button only appears on hover, keeping the list calm.
                            if hovered {
                                if theme::round_button(ui, "×", "remove").clicked() {
                                    remove = Some(i);
                                }
                            } else {
                                ui.add_space(26.0);
                            }
                            ui.add_space(4.0);
                            theme::size_stepper(ui, &mut preset.mb);
                        },
                    );
                }
            });
            if let Some(i) = remove {
                self.cfg.presets.remove(i);
            }
            if self.cfg.presets.len() < MAX_PRESETS {
                theme::hairline(ui);
                Self::row(
                    ui,
                    |ui| {
                        if ui.add(theme::ghost("+   add preset")).clicked() {
                            let name = self.cfg.next_preset_name();
                            let mb = self.cfg.presets.last().map(|p| p.mb * 2.0).unwrap_or(10.0).min(1000.0);
                            self.cfg.presets.push(Preset { name, mb });
                        }
                    },
                    |_| {},
                );
            }
        });

        ui.add_space(14.0);
        theme::caption(ui, "feel");
        theme::group(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            Self::row(
                ui,
                |ui| Self::titled(ui, "haptics", "tactile sounds while compressing"),
                |ui| {
                    if theme::toggle(ui, &mut self.cfg.sounds).changed() {
                        if self.cfg.sounds {
                            sound::set_enabled(true);
                            sound::play(Cue::ToggleOn);
                        } else {
                            sound::play(Cue::ToggleOff);
                            sound::set_enabled(false);
                        }
                    }
                },
            );
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui, snapshot: Option<&Snapshot<()>>, now: f64) {
        let ctx = ui.ctx().clone();

        if let Some(s) = snapshot {
            egui::Sides::new().show(
                ui,
                |ui| {
                    ui.label(RichText::new(&s.status).size(13.0).color(TEXT_2));
                },
                |ui| {
                    ui.label(RichText::new(&s.detail).size(12.0).color(TEXT_3));
                },
            );
            theme::progress_bar(ui, s.progress, Some(now));
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }

        if let Some((ok, msg)) = &self.message {
            ui.horizontal(|ui| {
                if *ok {
                    theme::check_badge(ui, 18.0, 1.0);
                } else {
                    theme::cross_badge(ui, 18.0);
                }
                ui.label(RichText::new(msg).size(12.5).color(if *ok { TEXT_2 } else { TEXT }));
            });
            ui.add_space(10.0);
        }

        if !self.installed {
            if ui.add_sized([ui.available_width(), 40.0], theme::primary(&format!("install {APP}"))).clicked() {
                self.start_install(&ctx);
            }
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("adds it to the right-click menu of your videos · no admin needed")
                        .size(12.0)
                        .color(TEXT_3),
                );
            });
            return;
        }

        let (mut uninstall, mut update) = (false, false);
        egui::Sides::new().show(
            ui,
            |ui| {
                uninstall = ui.add(theme::ghost("uninstall")).clicked();
            },
            |ui| {
                if !self.running_installed {
                    update = ui.add(theme::secondary("update installed copy")).clicked();
                }
                let pending = self.cfg != self.saved;
                let status = match (pending, self.saved_at) {
                    (true, _) => "saving…",
                    (false, Some(t)) if now - t < 2.5 => "saved",
                    _ => "changes save automatically",
                };
                ui.label(RichText::new(status).size(12.0).color(TEXT_3));
            },
        );
        if update {
            self.start_install(&ctx);
        }
        if uninstall && win::confirm(&format!("Remove {APP} from this computer?")) {
            install::uninstall();
            win::info_box(&format!("{APP} has been removed."));
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        style_chrome(frame, &mut self.styled);
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);

        let snapshot = self.task.as_ref().map(|t| t.snapshot());
        if let Some(s) = &snapshot {
            match &s.phase {
                Phase::Running => {}
                Phase::Done(()) => {
                    self.installed = true;
                    self.saved = self.cfg.clone();
                    self.task = None;
                    sound::play(Cue::Done);
                    self.message = Some((
                        true,
                        format!(
                            "installed · right-click any video → {APP}\non windows 11 it's under \"show more options\""
                        ),
                    ));
                }
                Phase::Failed(e) => {
                    sound::play(Cue::Error);
                    self.message = Some((false, e.clone()));
                    self.task = None;
                }
                Phase::Cancelled => self.task = None,
            }
        }
        let busy = self.task.is_some();

        // Auto-save (once installed) a moment after the last change, but not mid-drag.
        if self.installed && !busy && self.cfg != self.saved {
            let since = *self.changed_at.get_or_insert(now);
            if now - since > 0.6 && !ctx.egui_is_using_pointer() {
                self.changed_at = None;
                self.save(now);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        } else {
            self.changed_at = None;
        }
        if self.saved_at.is_some_and(|t| now - t < 2.6) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        let running = if busy { snapshot.as_ref() } else { None };
        let content = panel()
            .show(ui, |ui| {
                theme::top_glow(ui);
                let top = ui.cursor().top();
                ui.add_space(4.0);
                self.header(ui);
                ui.add_space(22.0);
                ui.add_enabled_ui(!busy, |ui| self.settings(ui));
                ui.add_space(22.0);
                self.footer(ui, running, now);
                ui.cursor().top() - top
            })
            .inner;

        // Fit the window to its content (presets can be added and removed).
        let wanted = (content + 2.0 * 22.0 - 8.0).round();
        if (wanted - self.height).abs() > 1.0 {
            self.height = wanted;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(SETUP_WIDTH, wanted)));
        }
    }
}
