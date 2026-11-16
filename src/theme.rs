//! Visual language: near-black layered surfaces, hairline borders, white as the only accent,
//! Segoe UI in three weights.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui,
    Vec2,
};

pub const BG: Color32 = Color32::from_gray(12);
pub const SURFACE: Color32 = Color32::from_gray(19);
pub const SURFACE_2: Color32 = Color32::from_gray(28);
pub const BORDER: Color32 = Color32::from_gray(35);
pub const BORDER_STRONG: Color32 = Color32::from_gray(58);
pub const TEXT: Color32 = Color32::from_gray(238);
pub const TEXT_2: Color32 = Color32::from_gray(156);
pub const TEXT_3: Color32 = Color32::from_gray(102);
pub const ACCENT: Color32 = Color32::from_gray(246);
pub const ON_ACCENT: Color32 = Color32::from_gray(14);

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn light() -> FontFamily {
    FontFamily::Name("light".into())
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.faint_bg_color = SURFACE;
    v.extreme_bg_color = Color32::from_gray(9);
    v.hyperlink_color = TEXT;
    v.selection.bg_fill = Color32::from_gray(72);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.text_cursor.stroke = Stroke::new(1.5, TEXT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    for (w, bg, stroke) in [
        (&mut v.widgets.inactive, SURFACE_2, BORDER),
        (&mut v.widgets.hovered, Color32::from_gray(36), BORDER_STRONG),
        (&mut v.widgets.active, Color32::from_gray(44), Color32::from_gray(90)),
        (&mut v.widgets.open, Color32::from_gray(36), BORDER_STRONG),
    ] {
        w.bg_fill = bg;
        w.weak_bg_fill = bg;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.corner_radius = CornerRadius::same(8);
        w.expansion = 0.0;
    }
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.global_style_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles = [
            (Small, FontId::new(12.0, FontFamily::Proportional)),
            (Body, FontId::new(14.0, FontFamily::Proportional)),
            (Button, FontId::new(14.0, FontFamily::Proportional)),
            (Heading, FontId::new(22.0, semibold())),
            (Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        s.spacing.interact_size.y = 30.0;
    });
}

/// Windows' own Segoe UI in regular, semibold and semilight, with Segoe UI Symbol for glyphs.
fn install_fonts(ctx: &egui::Context) {
    let dir =
        std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts");
    let mut defs = egui::FontDefinitions::default();
    let mut load = |name: &str, file: &str| -> Option<String> {
        let bytes = std::fs::read(dir.join(file)).ok()?;
        defs.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
        Some(name.to_string())
    };
    let regular = load("segoe", "segoeui.ttf");
    let bold = load("segoe-semibold", "seguisb.ttf");
    let thin = load("segoe-semilight", "segoeuisl.ttf");
    let symbol = load("segoe-symbol", "seguisym.ttf");

    let fallback = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |first: &Option<String>| -> Vec<String> {
        [first, &regular, &symbol].into_iter().flatten().cloned().chain(fallback.iter().cloned()).collect()
    };
    defs.families.insert(FontFamily::Proportional, family(&None));
    defs.families.insert(semibold(), family(&bold));
    defs.families.insert(light(), family(&thin));
    ctx.set_fonts(defs);
}

// ---------------------------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------------------------

/// A 1px divider spanning the available width.
pub fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, BORDER));
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(ON_ACCENT).family(semibold()))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .corner_radius(8)
        .min_size(Vec2::new(84.0, 32.0))
}

pub fn secondary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(TEXT))
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(8)
        .min_size(Vec2::new(0.0, 32.0))
}

pub fn ghost(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(TEXT_2)).frame(false)
}

/// "20 MB"-style editor: drag or click to type.
pub fn mb_input(value: &mut f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .range(1.0..=100_000.0)
        .speed(0.5)
        .custom_formatter(|v, _| crate::config::fmt_mb((v * 10.0).round() / 10.0))
        .suffix(" MB")
}

/// Sizes the − / + buttons step through.
const NICE_SIZES: [f64; 30] = [
    1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 75.0, 100.0, 125.0, 150.0,
    200.0, 250.0, 300.0, 400.0, 500.0, 750.0, 1000.0, 2000.0, 4000.0, 8000.0,
];

/// Small round button with a single glyph; only shows a background on hover.
pub fn round_button(ui: &mut Ui, glyph: &str, tip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(glyph).size(15.0).color(TEXT_2))
            .frame_when_inactive(false)
            .corner_radius(13)
            .min_size(Vec2::splat(26.0)),
    )
    .on_hover_text(tip)
}

/// `−  20 MB  +`: steps through sensible sizes; the number can also be dragged or typed.
/// Returns true when the value changed.
pub fn size_stepper(ui: &mut Ui, value: &mut f64) -> bool {
    let before = *value;
    // Inside right-aligned layouts egui adds widgets right to left; keep "− value +" reading order.
    let reversed = ui.layout().prefer_right_to_left();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let minus = |ui: &mut Ui, value: &mut f64| {
            if round_button(ui, "−", "smaller").clicked() {
                *value = NICE_SIZES.iter().rev().copied().find(|&s| s < *value - 0.01).unwrap_or(1.0);
            }
        };
        let number = |ui: &mut Ui, value: &mut f64| {
            ui.scope(|ui| {
                let w = &mut ui.visuals_mut().widgets;
                w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                w.inactive.bg_fill = Color32::TRANSPARENT;
                w.inactive.bg_stroke = Stroke::NONE;
                ui.spacing_mut().interact_size.x = 66.0;
                ui.add(mb_input(value));
            });
        };
        let plus = |ui: &mut Ui, value: &mut f64| {
            if round_button(ui, "+", "bigger").clicked() {
                *value = NICE_SIZES.iter().copied().find(|&s| s > *value + 0.01).unwrap_or(*value);
            }
        };
        if reversed {
            plus(ui, value);
            number(ui, value);
            minus(ui, value);
        } else {
            minus(ui, value);
            number(ui, value);
            plus(ui, value);
        }
    });
    *value != before
}

/// A rounded group of rows, iOS-settings style.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, Color32::from_gray(27)))
        .corner_radius(14)
        .inner_margin(Margin::symmetric(16, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Quiet lowercase caption above a group.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.0).color(TEXT_3));
}

/// A soft glow behind the top of the window.
pub fn top_glow(ui: &Ui) {
    let full = ui.clip_rect();
    let rect = Rect::from_min_size(full.min, Vec2::new(full.width(), 220.0));
    let mut mesh = egui::Mesh::default();
    let (top, bottom) = (Color32::from_gray(26), BG);
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    ui.painter().with_clip_rect(full).add(Shape::mesh(mesh));
}

/// The logo with a soft drop shadow.
pub fn logo_with_shadow(ui: &mut Ui, tex: &egui::TextureHandle, size: f32) {
    egui::Frame::new()
        .corner_radius((size * 0.23) as u8)
        .shadow(egui::Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(160) })
        .show(ui, |ui| logo(ui, tex, size));
}

/// Rounded label like "→ 20 MB".
pub fn pill(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(255)
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).family(semibold()).color(TEXT_2));
        });
}

pub fn logo(ui: &mut Ui, tex: &egui::TextureHandle, size: f32) {
    ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), Vec2::splat(size))));
}

pub fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let px = 128;
    let image = egui::ColorImage::from_rgba_unmultiplied([px, px], &crate::icon::rgba(px as u32));
    ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = Vec2::new(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.14);
    let track = lerp_color(SURFACE_2, ACCENT, t);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(11),
        track,
        Stroke::new(1.0, lerp_color(BORDER_STRONG, ACCENT, t)),
        egui::StrokeKind::Inside,
    );
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    painter.circle_filled(Pos2::new(x, rect.center().y), 8.0, lerp_color(TEXT_2, ON_ACCENT, t));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Thin rounded bar. With `shimmer`, a soft highlight sweeps across the filled part.
pub fn progress_bar(ui: &mut Ui, value: f32, shimmer: Option<f64>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), SURFACE_2);
    let value = value.clamp(0.0, 1.0);
    if value <= 0.0 {
        return;
    }
    let fill = Rect::from_min_max(rect.min, Pos2::new(rect.left() + rect.width() * value, rect.bottom()));
    let fill = fill.intersect(rect).expand2(Vec2::new(0.0, 0.0));
    painter.rect_filled(
        Rect::from_min_size(fill.min, Vec2::new(fill.width().max(6.0), fill.height())),
        CornerRadius::same(3),
        Color32::from_gray(214),
    );

    if let Some(time) = shimmer {
        let band = 90.0;
        let travel = fill.width() + band * 2.0;
        let x = fill.left() - band + ((time * 0.55).fract() as f32) * travel;
        let clip = painter.with_clip_rect(fill);
        let mut mesh = egui::Mesh::default();
        let stops = [(x - band, 0u8), (x, 150u8), (x + band, 0u8)];
        for (sx, alpha) in stops {
            let c = Color32::from_white_alpha(alpha);
            mesh.colored_vertex(Pos2::new(sx, fill.top()), c);
            mesh.colored_vertex(Pos2::new(sx, fill.bottom()), c);
        }
        for i in 0..2u32 {
            let b = i * 2;
            mesh.add_triangle(b, b + 1, b + 2);
            mesh.add_triangle(b + 1, b + 2, b + 3);
        }
        clip.add(Shape::mesh(mesh));
    }
}

/// White disc with a check mark that draws itself in as `t` goes 0 → 1.
pub fn check_badge(ui: &mut Ui, size: f32, t: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    let ease = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
    painter.circle_filled(c, size / 2.0 * (0.85 + 0.15 * ease), ACCENT);
    let s = size / 2.0;
    let pts =
        [c + Vec2::new(-0.42 * s, 0.02 * s), c + Vec2::new(-0.12 * s, 0.32 * s), c + Vec2::new(0.44 * s, -0.30 * s)];
    let seg1 = (pts[1] - pts[0]).length();
    let seg2 = (pts[2] - pts[1]).length();
    let drawn = ease * (seg1 + seg2);
    let stroke = Stroke::new(size * 0.09, ON_ACCENT);
    if drawn <= seg1 {
        painter.line_segment([pts[0], pts[0] + (pts[1] - pts[0]) * (drawn / seg1)], stroke);
    } else {
        painter.line_segment([pts[0], pts[1]], stroke);
        painter.line_segment([pts[1], pts[1] + (pts[2] - pts[1]) * ((drawn - seg1) / seg2)], stroke);
    }
}

/// Outlined disc with a cross.
pub fn cross_badge(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    painter.circle_stroke(c, size / 2.0 - 1.0, Stroke::new(1.5, TEXT_2));
    let d = size * 0.17;
    let stroke = Stroke::new(1.8, TEXT);
    painter.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], stroke);
    painter.line_segment([c + Vec2::new(-d, d), c + Vec2::new(d, -d)], stroke);
}
