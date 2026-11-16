//! Pixie dust (`cursor_trail_particles: "pixiedust"`): with each cursor move
//! that starts a trail, glittering particles shed along the path from the old
//! cursor's middle to the new one's, after kitty's pixiedust trail (written
//! from a description of its behaviour, not its GPLv3 source). Each move's
//! particles are a `Burst`, a pure function of its ends, the cursor's size and
//! the time since it started; its randomness comes from a seed made of its
//! ends, so a frame is the same however often it is drawn. Particles have
//! their own lifetime, not `cursor_trail_duration_ms`, and the last few moves'
//! stay alive together (`Dust`).

use super::cursor_shapes::{Layer, P, Tone};
use gpui::{Bounds, Pixels};
use std::collections::VecDeque;
use std::time::Instant;

/// Over how long a move's particles leave the path, s.
const EMIT: f32 = 0.08;
/// Particles a move sheds: three a cursor size along the path, within these.
const PER_SIZE: f32 = 3.0;
const COUNT: (usize, usize) = (4, 48);
/// How far from the path a particle starts, against the cursor's size, either way.
const SCATTER: f32 = 0.25;
/// Launch speed, cursor sizes a second, times a random share within `SPEED_SHARE`.
const SPEED: f32 = 0.4 * 20.0;
const SPEED_SHARE: (f32, f32) = (0.2, 1.0);
/// Drag (the rate its speed falls off) and gravity (cursor sizes a second²).
const DRAG: f32 = 2.5;
const GRAVITY: f32 = 8.0;
/// A particle's life, s, times a random share within `LIFE_SHARE`.
const LIFE: f32 = 0.8;
const LIFE_SHARE: (f32, f32) = (0.6, 1.0);
/// Its radius against the cursor's size (no less than `RADIUS_MIN` px), and
/// how far it shrinks as it fades.
const RADIUS: f32 = 0.12;
const RADIUS_MIN: f32 = 0.75;
const SHRINK: f32 = 0.5;
/// Glitter: brightness `GLITTER.0 + GLITTER.1 × sin(GLITTER_RATE × t + phase)`.
const GLITTER: (f32, f32) = (0.55, 0.45);
const GLITTER_RATE: f32 = 40.0;
/// How many moves' particles stay alive together.
const BURSTS: usize = 8;

/// One particle as drawn: its middle, radius (px) and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mote {
    pub(super) at: P,
    pub(super) radius: f32,
    pub(super) alpha: f32,
}

/// A move's particles: from the old cursor's middle to the new one's, for a
/// cursor `size` px (its larger side).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Burst {
    pub(super) from: P,
    pub(super) to: P,
    pub(super) size: f32,
}

/// The seed a move's randomness comes from: its ends.
fn seed(from: P, to: P) -> u64 {
    [from.0, from.1, to.0, to.1]
        .iter()
        .fold(0x9e37_79b9_7f4a_7c15, |h, v| mix(h ^ u64::from(v.to_bits())))
}

/// splitmix64's finaliser: a well-spread 64 bits from any 64.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The `k`th random number, 0 to 1, of particle `i` of the move seeded `seed`.
fn rand(seed: u64, i: usize, k: u64) -> f32 {
    (mix(seed ^ mix((i as u64) << 8 | k)) >> 40) as f32 / (1u64 << 24) as f32
}

fn within((lo, hi): (f32, f32), r: f32) -> f32 {
    lo + (hi - lo) * r
}

impl Burst {
    /// How many particles it sheds.
    pub(super) fn count(&self) -> usize {
        let path = (self.to.0 - self.from.0).hypot(self.to.1 - self.from.1);
        ((path / self.size.max(1e-3) * PER_SIZE).ceil() as usize).clamp(COUNT.0, COUNT.1)
    }

    /// When particle `i` leaves the path, s after the move, and how far
    /// along it (0 at the old cursor, 1 at the new): in order along the path,
    /// over `EMIT`.
    pub(super) fn launch(&self, i: usize) -> (f32, f32) {
        let u = (i as f32 + rand(seed(self.from, self.to), i, 0)) / self.count() as f32;
        (EMIT * u, u)
    }

    /// Its particles `t` s after the move: those launched and not yet faded.
    pub(super) fn motes(&self, t: f32) -> Vec<Mote> {
        let s = seed(self.from, self.to);
        let size = self.size;
        (0..self.count())
            .filter_map(|i| {
                let (start, u) = self.launch(i);
                let age = t - start;
                let life = LIFE * within(LIFE_SHARE, rand(s, i, 1));
                if !(0.0..life).contains(&age) {
                    return None;
                }
                let off = (
                    (rand(s, i, 2) * 2.0 - 1.0) * SCATTER * size,
                    (rand(s, i, 3) * 2.0 - 1.0) * SCATTER * size,
                );
                let angle = rand(s, i, 4) * std::f32::consts::TAU;
                let speed = SPEED * size * within(SPEED_SHARE, rand(s, i, 5));
                let gone = (1.0 - (-DRAG * age).exp()) / DRAG * speed;
                let at = (
                    self.from.0 + (self.to.0 - self.from.0) * u + off.0 + angle.cos() * gone,
                    self.from.1 + (self.to.1 - self.from.1) * u + off.1 + angle.sin() * gone
                        + 0.5 * GRAVITY * size * age * age,
                );
                let left = 1.0 - age / life;
                let glitter = GLITTER.0 + GLITTER.1 * (GLITTER_RATE * t + rand(s, i, 6) * std::f32::consts::TAU).sin();
                Some(Mote {
                    at,
                    radius: (RADIUS * size).max(RADIUS_MIN) * (1.0 - SHRINK * (1.0 - left)),
                    alpha: (left * glitter).clamp(0.0, 1.0),
                })
            })
            .collect()
    }

    /// Whether any of its particles can still show at `t`.
    pub(super) fn alive(&self, t: f32) -> bool {
        t < EMIT + LIFE * LIFE_SHARE.1
    }
}

/// The particles alive: the last `BURSTS` moves', each from when it started.
#[derive(Default)]
pub(super) struct Dust {
    bursts: VecDeque<(Burst, Instant)>,
}

impl Dust {
    pub(super) fn add(&mut self, burst: Burst, at: Instant) {
        if self.bursts.len() == BURSTS {
            self.bursts.pop_front();
        }
        self.bursts.push_back((burst, at));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.bursts.is_empty()
    }

    /// The same dust, started `by` earlier.
    #[cfg(test)]
    pub(super) fn age(&mut self, by: std::time::Duration) {
        for (_, at) in &mut self.bursts {
            *at -= by;
        }
    }

    /// Every live particle at `now` as a layer to paint, none over `cursor`:
    /// each fades out over its radius as it nears it, so one that flies
    /// past it dims and comes back rather than blinking; moves whose
    /// particles have all faded are dropped.
    pub(super) fn layers(&mut self, now: Instant, cursor: Option<Bounds<Pixels>>) -> Vec<Layer> {
        self.bursts
            .retain(|(b, at)| b.alive(now.saturating_duration_since(*at).as_secs_f32()));
        // How far it may show beside the cursor: none touching it, all from
        // a radius clear of it, smoothly between.
        let shown = |m: &Mote| {
            cursor.map_or(1.0, |c| {
                let (x0, y0) = (f32::from(c.origin.x), f32::from(c.origin.y));
                let (x1, y1) = (x0 + f32::from(c.size.width), y0 + f32::from(c.size.height));
                let gap = ((x0 - m.at.0).max(m.at.0 - x1).max(0.0)).hypot((y0 - m.at.1).max(m.at.1 - y1).max(0.0));
                let k = ((gap - m.radius) / m.radius).clamp(0.0, 1.0);
                k * k * (3.0 - 2.0 * k)
            })
        };
        self.bursts
            .iter()
            .flat_map(|(b, at)| b.motes(now.saturating_duration_since(*at).as_secs_f32()))
            .filter_map(|m| {
                let alpha = m.alpha * shown(&m);
                (alpha > 0.0).then(|| Layer::disc(m.at, m.radius, alpha, Tone::Effect))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(len: f32) -> Burst {
        Burst { from: (4.0, 8.0), to: (4.0 + len, 8.0), size: 16.0 }
    }

    #[test]
    fn a_move_sheds_three_particles_a_cursor_size_within_its_bounds() {
        assert_eq!(burst(0.0).count(), 4);
        assert_eq!(burst(16.0).count(), 4);
        assert_eq!(burst(160.0).count(), 30);
        assert_eq!(burst(4000.0).count(), 48);
    }

    #[test]
    fn particles_leave_in_order_along_the_path_over_the_emit_time() {
        let b = burst(400.0);
        let launches: Vec<(f32, f32)> = (0..b.count()).map(|i| b.launch(i)).collect();
        assert!(launches.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 > w[0].1), "in order: {launches:?}");
        assert!(launches.iter().all(|&(t, u)| (0.0..EMIT).contains(&t) && (0.0..1.0).contains(&u)));
        assert!(b.motes(0.0).len() <= 1, "nothing much at once");
        assert_eq!(b.motes(EMIT + 0.01).len(), b.count(), "all out by the emit time");
        // Just out, each is near the path: its scatter and a little flight.
        let near = 16.0 * SCATTER + SPEED * 16.0 * EMIT + 0.5 * GRAVITY * 16.0 * EMIT * EMIT;
        for m in b.motes(0.079) {
            assert!((m.at.1 - 8.0).abs() <= near, "{m:?}");
        }
    }

    #[test]
    fn the_same_move_draws_the_same_particles_and_another_move_others() {
        let (a, b) = (burst(300.0), burst(300.0));
        for t in [0.03, 0.2, 0.5] {
            assert_eq!(a.motes(t), b.motes(t), "{t}");
        }
        let other = Burst { to: (304.0, 9.0), ..burst(300.0) };
        assert_ne!(a.motes(0.2), other.motes(0.2));
    }

    #[test]
    fn particles_fly_fall_shrink_glitter_and_fade() {
        let b = burst(400.0);
        let early = b.motes(0.1);
        let late = b.motes(0.45);
        let mean = |m: &[Mote]| m.iter().map(|m| m.at.1).sum::<f32>() / m.len().max(1) as f32;
        assert!(mean(&late) > mean(&early), "falling: {} then {}", mean(&early), mean(&late));
        let biggest = (RADIUS * 16.0).max(RADIUS_MIN);
        for m in early.iter().chain(&late) {
            assert!(m.radius <= biggest + 1e-6 && m.radius >= biggest * SHRINK - 1e-6);
            assert!((0.0..=1.0).contains(&m.alpha) && m.at.0.is_finite() && m.at.1.is_finite());
        }
        // Speed: at most 0.4 × 20 cursor sizes a second, less drag.
        let reach = SPEED * 16.0 / DRAG + 16.0 * SCATTER * 1.5 + 0.5 * GRAVITY * 16.0;
        for m in b.motes(0.85) {
            assert!(m.at.0 > -reach && m.at.0 < 404.0 + reach, "{m:?}");
        }
        // A tiny cursor's particles are no smaller than RADIUS_MIN as they start.
        let tiny = Burst { size: 2.0, ..burst(40.0) };
        assert!(tiny.motes(0.05).iter().all(|m| m.radius >= RADIUS_MIN * SHRINK));
    }

    #[test]
    fn a_moves_particles_are_all_gone_by_their_longest_life() {
        let b = burst(400.0);
        let end = EMIT + LIFE * LIFE_SHARE.1;
        assert!(b.alive(end - 0.01) && !b.alive(end));
        assert!(b.motes(end).is_empty(), "none after it");
        let mut dust = Dust::default();
        let start = Instant::now();
        dust.add(b, start);
        assert!(!dust.layers(start + std::time::Duration::from_millis(100), None).is_empty());
        assert!(dust.layers(start + std::time::Duration::from_millis(900), None).is_empty());
        assert!(dust.is_empty(), "the move dropped once its particles faded: no more frames");
    }

    #[test]
    fn the_last_eight_moves_stay_alive_together_and_none_over_the_cursor() {
        let mut dust = Dust::default();
        let start = Instant::now();
        for k in 0..10 {
            dust.add(Burst { from: (0.0, k as f32 * 20.0), to: (300.0, k as f32 * 20.0), size: 16.0 }, start);
        }
        assert_eq!(dust.bursts.len(), BURSTS, "the oldest let go");
        let cursor = Bounds::new(gpui::point(gpui::px(100.0), gpui::px(0.0)), gpui::size(gpui::px(150.0), gpui::px(200.0)));
        let layers = dust.layers(start + std::time::Duration::from_millis(200), Some(cursor));
        assert!(!layers.is_empty());
        for l in &layers {
            let ((x, y), r) = l.disc.expect("discs");
            let gap = ((100.0 - x).max(x - 250.0).max(0.0)).hypot((0.0 - y).max(y - 200.0).max(0.0));
            assert!(gap >= r, "over the cursor: {x} {y}");
        }
    }

    /// Nearing the cursor, a particle dims smoothly to nothing as it reaches
    /// it, rather than blinking out whole.
    #[test]
    fn a_particle_fades_as_it_nears_the_cursor_rather_than_blinking() {
        let start = Instant::now();
        let b = burst(400.0);
        let t = 0.3;
        let free: Vec<Mote> = b.motes(t);
        let m = free[free.len() / 2];
        let alpha_with = |gap: f32| {
            let mut dust = Dust::default();
            dust.add(b.clone(), start);
            let left = m.at.0 + gap;
            let cursor = Bounds::new(gpui::point(gpui::px(left), gpui::px(-1000.0)), gpui::size(gpui::px(1.0), gpui::px(3000.0)));
            dust.layers(start + std::time::Duration::from_secs_f32(t), Some(cursor))
                .iter()
                .find(|l| l.disc.is_some_and(|(at, _)| at == m.at))
                .map_or(0.0, |l| l.alpha)
        };
        let alphas: Vec<f32> = [3.0, 2.0, 1.5, 1.0, 0.5].iter().map(|k| alpha_with(m.radius * k)).collect();
        assert!((alphas[0] - m.alpha).abs() < 1e-5 && (alphas[1] - m.alpha).abs() < 1e-5, "clear of it, as it is: {alphas:?}");
        assert!(alphas[2] > 0.0 && alphas[2] < m.alpha, "dimmer nearer: {alphas:?}");
        assert_eq!((alphas[3], alphas[4]), (0.0, 0.0), "none touching it");
    }
}
