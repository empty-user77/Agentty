//! The night sky behind the start page — the same twinkling stars and shooting stars as
//! agentty.run, painted natively.
//!
//! Everything is a pure function of the elapsed time and the star's index, so there is no state to
//! keep between frames: each frame recomputes the sky and asks for the next one. Stars are quads
//! with a full corner radius (circles); a shooting star is a stroked tail whose segments fade out,
//! since GPUI has no gradient strokes.

use crate::theme::{hex_alpha, Chrome};
use gpui::{fill, px, Bounds, IntoElement, PathBuilder, Pixels, Styled};

/// One star per this many square pixels — matches the website's density.
const AREA_PER_STAR: f32 = 2600.;
const MAX_STARS: usize = 420;
/// Seconds between shooting stars, and how long one stays visible.
const METEOR_EVERY: f32 = 4.5;
const METEOR_LIFE: f32 = 1.5;

/// Deterministic pseudo-random values in `0.0..1.0` for star `index`, stream `salt`.
fn noise(index: usize, salt: u32) -> f32 {
    let mut h = (index as u32).wrapping_mul(0x9e37_79b9) ^ salt.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_f491);
    h ^= h >> 13;
    (h % 100_000) as f32 / 100_000.
}

/// White, or one of the two tints the website uses, as `(r, g, b)`.
fn star_color(hue: f32) -> u32 {
    if hue > 0.93 {
        0xffd666
    } else if hue > 0.85 {
        0xa0beff
    } else {
        0xffffff
    }
}

/// A full-bleed sky. Place it as the first child of a `relative()` container.
pub fn starfield() -> impl IntoElement {
    let started = std::time::Instant::now();
    gpui::canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            if w < 1. || h < 1. {
                return;
            }
            let t = started.elapsed().as_secs_f32();
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let count = ((w * h / AREA_PER_STAR) as usize).min(MAX_STARS);

            for i in 0..count {
                let depth = noise(i, 1);
                let x = ox + noise(i, 2) * w;
                let y = oy + noise(i, 3) * h;
                // Near stars are bigger and brighter; far ones stay pinpricks.
                let radius = 0.25 + depth * depth * 1.35;
                let speed = 0.6 + noise(i, 4) * 2.2;
                let twinkle = 0.55 + 0.45 * (t * speed + noise(i, 5) * std::f32::consts::TAU).sin();
                let alpha = (0.25 + depth * 0.75) * twinkle;
                let color = star_color(noise(i, 6));
                let mut dot = |r: f32, alpha: f32| {
                    let rect = Bounds::new(gpui::point(px(x - r), px(y - r)), gpui::size(px(r * 2.), px(r * 2.)));
                    window.paint_quad(fill(rect, hex_alpha(color, alpha)).corner_radii(px(r)));
                };
                dot(radius, alpha);
                // The brightest moment of a big star blooms, the way it does on the site.
                if radius > 1.2 && twinkle > 0.9 {
                    dot(radius * 3.2, alpha * 0.18);
                }
            }

            // Shooting stars arrive on a fixed cadence; which one is in flight follows from the
            // clock, so no frame has to remember the last.
            let index = (t / METEOR_EVERY) as usize;
            for k in index.saturating_sub(1)..=index {
                let age = t - k as f32 * METEOR_EVERY;
                if !(0. ..METEOR_LIFE).contains(&age) {
                    continue;
                }
                let from_left = noise(k, 11) < 0.5;
                let dir = if from_left { 1. } else { -1. };
                let (vx, vy) = (dir * (420. + noise(k, 12) * 300.), 180. + noise(k, 13) * 150.);
                let start_x = if from_left { noise(k, 14) * w * 0.5 } else { w * 0.5 + noise(k, 14) * w * 0.5 };
                let (x, y) = (ox + start_x + vx * age, oy + noise(k, 15) * h * 0.4 + vy * age);
                let life = 1. - age / METEOR_LIFE;
                // The tail: short segments, each dimmer than the one ahead of it.
                const SEGMENTS: usize = 8;
                for s in 0..SEGMENTS {
                    let (near, far) = (s as f32 / SEGMENTS as f32, (s + 1) as f32 / SEGMENTS as f32);
                    let tail = 0.2;
                    let mut path = PathBuilder::stroke(px(1.4));
                    path.move_to(gpui::point(px(x - vx * tail * near), px(y - vy * tail * near)));
                    path.line_to(gpui::point(px(x - vx * tail * far), px(y - vy * tail * far)));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, hex_alpha(Chrome::METEOR, life * (1. - near)));
                    }
                }
            }
            // Keep the sky moving — but not while the window is in the background, where nobody is
            // watching and the frames would only cost battery.
            if window.is_window_active() {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .size_full()
}
