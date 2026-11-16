//! The progress "builder": small blocks are launched from the bottom-left corner, arc across
//! with a little spin and stack into a 4-row grid that fills from right to left, so the file is
//! visibly built as it compresses.

use std::f32::consts::PI;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

use crate::theme::{BORDER, SURFACE};

pub const ROWS: usize = 4;
const BLOCK: f32 = 9.0;
const GAP: f32 = 3.0;
const PAD: f32 = 10.0;
pub const HEIGHT: f32 = PAD * 2.0 + ROWS as f32 * BLOCK + (ROWS - 1) as f32 * GAP;

const EMPTY: Color32 = Color32::from_gray(26);
const FLYING: Color32 = Color32::from_gray(250);

/// A block that finished its flight this frame.
pub struct Landing {
    /// It filled the top of its column.
    pub column_done: bool,
    /// Where it landed, -1 (left) to 1 (right).
    pub x: f32,
}

#[derive(Default)]
pub struct BlockGrid {
    cols: usize,
    /// When each slot's block landed.
    landed: Vec<Option<f64>>,
    /// Blocks in the air: (slot, launch time, flight duration).
    flights: Vec<(usize, f64, f64)>,
    /// Blocks falling away after a retry: (slot, time).
    leaving: Vec<(usize, f64)>,
    /// Slots below this have been launched (or landed).
    next: usize,
    last_launch: f64,
    completed_at: Option<f64>,
}

impl BlockGrid {
    /// When the last block landed with the grid full.
    pub fn completed_at(&self) -> Option<f64> {
        self.completed_at
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    /// How long the finishing scan takes, including the last column's flash.
    pub fn scan_duration(&self) -> f64 {
        (self.cols as f32 * crate::sound::scan_step(self.cols)) as f64 + 0.25
    }

    /// Advances the animation towards `progress` (0..1), draws it and returns this frame's landings.
    /// `scan_from` starts the finishing scan (left to right, one column per sound click).
    pub fn show(&mut self, ui: &mut Ui, progress: f32, now: f64, scan_from: Option<f64>) -> Vec<Landing> {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
        let pitch = BLOCK + GAP;
        let cols = (((rect.width() - 2.0 * PAD + GAP) / pitch).floor() as usize).max(1);
        if cols != self.cols {
            *self = BlockGrid { cols, landed: vec![None; cols * ROWS], ..Default::default() };
        }
        let total = cols * ROWS;
        let grid_w = cols as f32 * pitch - GAP;
        let origin = Pos2::new(rect.center().x - grid_w / 2.0, rect.top() + PAD);
        // Columns fill right to left; each column stacks bottom to top.
        let slot = |i: usize| {
            let col = cols - 1 - i / ROWS;
            let row = ROWS - 1 - i % ROWS;
            Rect::from_min_size(origin + Vec2::new(col as f32 * pitch, row as f32 * pitch), Vec2::splat(BLOCK))
        };
        let launch = slot(total - ROWS).center();
        let target = ((progress.clamp(0.0, 1.0) * total as f32).floor() as usize).min(total);

        // Progress went backwards (a retry): extra blocks fall away.
        if target < self.next {
            for i in target..total {
                if self.landed[i].take().is_some() {
                    self.leaving.push((i, now));
                }
            }
            self.flights.retain(|f| f.0 < target);
            self.next = target;
        }

        // Launch at most one block per frame; faster when falling behind.
        if self.next < target {
            let backlog = (target - self.next) as f64;
            let interval = (0.085 / (1.0 + backlog * 0.35)).max(0.02);
            if now - self.last_launch >= interval {
                let distance = (slot(self.next).center() - launch).length() / rect.width();
                self.flights.push((self.next, now, 0.22 + 0.34 * distance as f64));
                self.last_launch = now;
                self.next += 1;
            }
        }

        let mut landings = Vec::new();
        let landed = &mut self.landed;
        self.flights.retain(|&(i, start, duration)| {
            if now - start < duration {
                return true;
            }
            landed[i] = Some(now);
            let x = (slot(i).center().x - rect.center().x) / (rect.width() / 2.0);
            landings.push(Landing { column_done: i % ROWS == ROWS - 1, x });
            false
        });
        self.leaving.retain(|&(_, t)| now - t < 0.35);
        if target == total && self.next == total && self.flights.is_empty() {
            self.completed_at.get_or_insert(now);
        } else {
            self.completed_at = None;
        }

        // ---- paint ----
        let painter = ui.painter_at(rect);
        painter.rect(rect, CornerRadius::same(10), SURFACE, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

        let step = crate::sound::scan_step(cols) as f64;
        // The scan: a soft light behind the column currently being "checked".
        if let Some(start) = scan_from {
            let k = ((now - start) / step).floor();
            if (0.0..cols as f64).contains(&k) {
                let x = origin.x + k as f32 * pitch - GAP;
                let band = Rect::from_min_size(
                    Pos2::new(x, rect.top() + 4.0),
                    Vec2::new(BLOCK + GAP * 2.0, rect.height() - 8.0),
                );
                painter.rect_filled(band, CornerRadius::same(4), Color32::from_white_alpha(22));
            }
        }

        for i in 0..total {
            let r = slot(i);
            let Some(t) = self.landed[i] else {
                painter.rect_filled(r, CornerRadius::same(2), EMPTY);
                continue;
            };
            // Landing: a small "pop", then settle, with a brief brightness flash.
            let mut age = (now - t) as f32;
            if let Some(start) = scan_from {
                // Each column re-pops as the scan passes over it, in time with its click.
                let col_from_left = cols - 1 - i / ROWS;
                let hit = start + col_from_left as f64 * step;
                if now >= hit {
                    age = age.min((now - hit) as f32);
                }
            }
            let pop = 1.0 + 0.32 * (1.0 - (age / 0.16).min(1.0)).powi(2);
            let gray = 208.0 + 47.0 * (1.0 - age / 0.3).clamp(0.0, 1.0);
            painter.rect_filled(
                Rect::from_center_size(r.center(), r.size() * pop),
                CornerRadius::same(2),
                Color32::from_gray(gray.min(255.0) as u8),
            );
        }

        for &(i, t) in &self.leaving {
            let age = (now - t) as f32;
            let r = slot(i).translate(Vec2::new(0.0, age * age * 160.0));
            let alpha = (1.0 - age / 0.35).clamp(0.0, 1.0);
            painter.rect_filled(r, CornerRadius::same(2), Color32::from_white_alpha((alpha * 200.0) as u8));
        }

        for &(i, start, duration) in &self.flights {
            let t = (((now - start) / duration) as f32).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - t).powi(3);
            let end = slot(i).center();
            let ctrl = Pos2::new((launch.x + end.x) / 2.0, rect.top() + PAD * 0.4);
            let p = launch.lerp(ctrl, e).lerp(ctrl.lerp(end, e), e);
            let size = BLOCK * (t / 0.12).min(1.0);
            let angle = (1.0 - e) * PI * 0.75;
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y): (f32, f32)| {
                let (s, c) = angle.sin_cos();
                p + Vec2::new(x * c - y * s, x * s + y * c) * (size / 2.0)
            });
            painter.add(Shape::convex_polygon(corners.to_vec(), FLYING, Stroke::NONE));
        }

        landings
    }
}
