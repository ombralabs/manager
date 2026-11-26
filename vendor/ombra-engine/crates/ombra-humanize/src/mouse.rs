//! Mouse movement plans (spec 5.4) on top of cursory-rs, which the browser loads at run time (D2).
//!
//! The plan covers rules 1–4, 6 and 13 of spec 5.4: event rate from the caller's frame rate,
//! duration window with regeneration at `frequency · scale`, the physical-pixel grid, the client's
//! destination as the final event (never a step), steps outside the viewport skipped, the seed kept
//! in the plan. Scheduling, acks, event fields and cursor memory (rules 5, 7–12) belong to the
//! browser glue.

use crate::persona::{Movement, Persona, Preset, TrackpadMovement};
use crate::rng::Rng;
use crate::{PlanError, Point};

/// What the planner needs from a trajectory generator.
pub trait TrajectorySource {
    /// Number of recordings: the length `selection_bias` must have.
    fn record_count(&self) -> usize;
    fn generate(&self, start: Point, end: Point, options: &SourceOptions) -> Result<SourceTrajectory, SourceError>;
}

pub struct SourceOptions<'a> {
    pub frequency_hz: f64,
    pub frequency_randomizer_ms: f64,
    pub seed: u64,
    pub directness: f64,
    pub selection_bias: Option<&'a [f64]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceTrajectory {
    pub points: Vec<Point>,
    /// Milliseconds from the first point.
    pub timings_ms: Vec<i64>,
}

/// A cursory-rs error code (`CURSORY_ERR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceError(pub i32);

/// Highest frequency cursory-rs accepts (its `MAX_FREQUENCY_HZ`).
pub const MAX_SOURCE_FREQUENCY_HZ: f64 = 10_000.0;

// The C ABI of cursory-rs, include/cursory.h, ABI version 1.
pub const CURSORY_ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CursoryPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
pub struct CursoryOptions {
    pub frequency_hz: f64,
    pub frequency_randomizer_ms: f64,
    pub seed: u64,
    pub directness: f64,
    pub selection_bias: *const f64,
    pub selection_bias_len: usize,
}

#[repr(C)]
pub struct CursoryTrajectory {
    pub points: *mut CursoryPoint,
    pub timings_ms: *mut i64,
    pub len: usize,
}

pub type CursoryGenerateFn = unsafe extern "C" fn(CursoryPoint, CursoryPoint, *const CursoryOptions, *mut CursoryTrajectory) -> i32;
pub type CursoryFreeFn = unsafe extern "C" fn(*mut CursoryTrajectory);

/// cursory-rs reached through its C ABI: the browser resolves the symbols with dlopen/dlsym and
/// checks `cursory_abi_version() == CURSORY_ABI_VERSION` before building this.
pub struct CApiSource {
    generate: CursoryGenerateFn,
    free: CursoryFreeFn,
    records: usize,
}

impl CApiSource {
    /// # Safety
    /// `generate` and `free` must be `cursory_generate` and `cursory_trajectory_free` of one
    /// cursory-rs library with ABI version 1 that stays loaded while this value lives, and
    /// `records` its `cursory_record_count()`.
    pub unsafe fn new(generate: CursoryGenerateFn, free: CursoryFreeFn, records: usize) -> Self {
        CApiSource { generate, free, records }
    }
}

impl TrajectorySource for CApiSource {
    fn record_count(&self) -> usize {
        self.records
    }

    fn generate(&self, start: Point, end: Point, o: &SourceOptions) -> Result<SourceTrajectory, SourceError> {
        let (bias, bias_len) = match o.selection_bias {
            Some(b) => (b.as_ptr(), b.len()),
            None => (std::ptr::null(), 0),
        };
        let options = CursoryOptions {
            frequency_hz: o.frequency_hz,
            frequency_randomizer_ms: o.frequency_randomizer_ms,
            seed: o.seed,
            directness: o.directness,
            selection_bias: bias,
            selection_bias_len: bias_len,
        };
        let mut out = CursoryTrajectory { points: std::ptr::null_mut(), timings_ms: std::ptr::null_mut(), len: 0 };
        // SAFETY: `self.generate` is the library's `cursory_generate`, per the contract of
        // `CApiSource::new`; `options` and `out` live and stay put for the whole call, and the bias
        // pointer points into `o`, which outlives it.
        let code = unsafe {
            (self.generate)(CursoryPoint { x: start.x, y: start.y }, CursoryPoint { x: end.x, y: end.y }, &options, &mut out)
        };
        if code != 0 {
            return Err(SourceError(code));
        }
        // A compatible replacement library (D2) may answer OK with NULL and length 0 for an empty
        // trajectory, and `from_raw_parts` on NULL is undefined behaviour.
        if out.len == 0 || out.points.is_null() || out.timings_ms.is_null() {
            // SAFETY: `out` is what this library's `generate` left in it, the state its `free` takes.
            unsafe { (self.free)(&mut out) };
            return Ok(SourceTrajectory { points: Vec::new(), timings_ms: Vec::new() });
        }
        // SAFETY: `generate` returned OK, both pointers are non-null and `len` is the library's: each
        // array holds `len` elements until `free` below, and both are copied before that.
        let trajectory = unsafe {
            let points = std::slice::from_raw_parts(out.points, out.len);
            let timings = std::slice::from_raw_parts(out.timings_ms, out.len);
            SourceTrajectory {
                points: points.iter().map(|p| Point { x: p.x, y: p.y }).collect(),
                timings_ms: timings.to_vec(),
            }
        };
        // SAFETY: `out` came from this library's `generate` and is freed once; the copy above holds
        // no borrow of it.
        unsafe { (self.free)(&mut out) };
        Ok(trajectory)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// A mouse's sensor (decision D61): at every poll the device reports the whole counts it moved on
/// each axis since its last report, and nothing when it moved less than one. The frame then takes
/// the latest report, its time and its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sensor {
    pub poll_ms: f64,
    /// One count, CSS px.
    pub count_px: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseContext {
    /// The window's frame rate (spec 5.4 p.1); 60 when unknown (headless). Without a sensor the
    /// steps come at this rate, one per frame.
    pub frame_hz: f64,
    pub device_pixel_ratio: f64,
    pub viewport: Viewport,
    /// Duration window of one movement, ms (spec 5.4 p.2).
    pub min_ms: f64,
    pub max_ms: f64,
    /// Multiplier on the movement's duration, applied with the persona's speed: 1.0, or a drag's
    /// `DragPlan::speed_mult` (spec 5.5).
    pub duration_mult: f64,
    /// A mouse: steps are its reports, at its polls (the browser takes one per frame). None: one
    /// step per frame (a trackpad, which the browser reports on its own clock).
    pub sensor: Option<Sensor>,
    /// The hand may pass the destination on its way and come back (the persona's reach, a trackpad's
    /// overshoot). Off for the captcha module's drag of a slider (captcha plan E4d-6): GeeTest refused
    /// both drags that went past their release point; the hand then never goes past it along the way.
    pub overshoot: bool,
}

impl MouseContext {
    pub fn new(viewport: Viewport) -> Self {
        MouseContext { frame_hz: 60.0, device_pixel_ratio: 1.0, viewport, min_ms: 0.0, max_ms: 1500.0, duration_mult: 1.0, sensor: None, overshoot: true }
    }
}

/// `p` held short of `to` along the way from `from` (unit direction `unit`, `d` long): past the
/// destination along the way it is drawn back onto its line, its sideways place kept.
fn short_of(p: Point, from: Point, unit: (f64, f64), d: f64) -> Point {
    let along = (p.x - from.x) * unit.0 + (p.y - from.y) * unit.1;
    if along <= d {
        return p;
    }
    Point { x: p.x - (along - d) * unit.0, y: p.y - (along - d) * unit.1 }
}

/// With overshoot off: a pixel whose projection on the way passes the destination goes back by one
/// device pixel along the way's dominant axis (twice at most), its other coordinate kept; dropping it
/// would erase a long sideways correction.
fn nudged_back(mut pixel: (f64, f64), from: Point, unit: (f64, f64), d: f64, dpr: f64) -> (f64, f64) {
    let past = |p: (f64, f64)| (p.0 - from.x) * unit.0 + (p.1 - from.y) * unit.1 > d + 1e-9;
    for _ in 0..2 {
        if !past(pixel) {
            break;
        }
        if unit.0.abs() >= unit.1.abs() {
            pixel.0 -= unit.0.signum() / dpr;
        } else {
            pixel.1 -= unit.1.signum() / dpr;
        }
    }
    pixel
}

/// One intermediate `mousemove`, due `at_ms` after the start of the movement.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Step {
    pub x: f64,
    pub y: f64,
    pub at_ms: f64,
    /// The first step after the finger was lifted (a trackpad's clutch): the browser fills no
    /// reports between the step before and this one.
    pub rest: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MousePlan {
    pub steps: Vec<Step>,
    /// When the client's own event (the destination) is due.
    pub end_at_ms: f64,
    /// The per-movement seed, for replay from the debug log.
    pub seed: u64,
}

/// Plan the movement from `from` to `to` (CSS px of the top-level viewport).
pub fn plan_move(
    source: &dyn TrajectorySource,
    persona: &Persona,
    preset: Preset,
    ctx: &MouseContext,
    from: Point,
    to: Point,
    seed: u64,
) -> Result<MousePlan, PlanError> {
    let bias = persona.affinity_weights(source.record_count());
    plan_move_with_bias(source, persona, preset, ctx, from, to, &bias, seed)
}

/// cursory's paths are sampled at this rate: dense enough to follow at any speed.
pub const PATH_HZ: f64 = 1000.0;
/// A move this short is one plain bell: no creep, no miss, no correction (under a few sensor
/// counts a hand does not correct).
const PLAIN_MOVE_PX: f64 = 5.0;
/// A correction this short goes straight, without a cursory path.
const STRAIGHT_PX: f64 = 3.0;
/// A trackpad reports on its own clock, ms (the browser's kTrackpadInterval).
pub const TRACKPAD_REPORT_MS: f64 = 8.05;
/// A lifted finger is down this long at least, ms: one movement splits there (the stand's 200 ms).
const CLUTCH_FLOOR_MS: f64 = 200.001;
/// The longest halt of a slow stretch, reports: a longer one would split the movement.
const MAX_HALT_REPORTS: usize = 23;
/// A stroke never lasts more reports than this (8 s): a persona overridden with an absurd law plans
/// a bounded movement, which the duration window then cuts.
const MAX_PULSE_REPORTS: f64 = 1000.0;
/// How far past its end a stroke goes at most, a share of its chord.
const MAX_OVERSHOOT: f64 = 1.0;

/// One stretch of the hand's way: along `path`, from `start_ms` for `ms`, at `progress`.
struct Piece {
    path: Path,
    start_ms: f64,
    ms: f64,
    progress: Progress,
}

enum Progress {
    /// A minimum-jerk bell blended with the recording's own progress (knots of time share, way
    /// share) by `bell`.
    Bell { bell: f64, own: Vec<(f64, f64)> },
    /// A slice [from, to] of the way of a minimum-jerk bell over the whole correction.
    MinJerk,
}

fn minimum_jerk(tau: f64) -> f64 {
    tau * tau * tau * (10.0 - 15.0 * tau + 6.0 * tau * tau)
}

/// A polyline by arc length.
struct Path {
    points: Vec<Point>,
    cumulative: Vec<f64>,
}

impl Path {
    fn new(points: Vec<Point>) -> Path {
        let mut cumulative = Vec::with_capacity(points.len());
        let mut length = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                length += (p.x - points[i - 1].x).hypot(p.y - points[i - 1].y);
            }
            cumulative.push(length);
        }
        Path { points, cumulative }
    }

    fn length(&self) -> f64 {
        *self.cumulative.last().unwrap_or(&0.0)
    }

    fn at(&self, s: f64) -> Point {
        let i = self.cumulative.partition_point(|&c| c < s).clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = self.cumulative[i] - self.cumulative[i - 1];
        let f = if span > 0.0 { ((s - self.cumulative[i - 1]) / span).clamp(0.0, 1.0) } else { 1.0 };
        Point { x: a.x + (b.x - a.x) * f, y: a.y + (b.y - a.y) * f }
    }

    /// The part of the way between arc lengths `s0` and `s1`.
    fn slice(&self, s0: f64, s1: f64) -> Path {
        let mut points = vec![self.at(s0)];
        points.extend(self.points.iter().zip(&self.cumulative).filter(|(_, &c)| c > s0 && c < s1).map(|(p, _)| *p));
        points.push(self.at(s1));
        Path::new(points)
    }
}

/// Interpolates the knots (time share, way share) at `tau`.
fn own_progress(knots: &[(f64, f64)], tau: f64) -> f64 {
    let i = knots.partition_point(|k| k.0 <= tau).clamp(1, knots.len() - 1);
    let ((t0, s0), (t1, s1)) = (knots[i - 1], knots[i]);
    if t1 > t0 { s0 + (s1 - s0) * ((tau - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { s1 }
}

impl Piece {
    /// The hand's position `t` ms into the piece.
    fn at(&self, t: f64) -> Point {
        let tau = if self.ms > 0.0 { (t / self.ms).clamp(0.0, 1.0) } else { 1.0 };
        let share = match &self.progress {
            Progress::Bell { bell, own } if own.len() >= 2 => bell * minimum_jerk(tau) + (1.0 - bell) * own_progress(own, tau),
            _ => minimum_jerk(tau),
        };
        self.path.at(share * self.path.length())
    }
}

/// `plan_move` with the persona's recording weights drawn once by the caller as
/// `persona.affinity_weights(source.record_count())`: the browser keeps them for the process
/// instead of drawing 2356 of them per movement.
#[allow(clippy::too_many_arguments)]
pub fn plan_move_with_bias(
    source: &dyn TrajectorySource,
    persona: &Persona,
    preset: Preset,
    ctx: &MouseContext,
    from: Point,
    to: Point,
    bias: &[f64],
    seed: u64,
) -> Result<MousePlan, PlanError> {
    match plan_pieces(source, persona, preset, ctx, from, to, bias, seed) {
        // A source that answers with no trajectory (a replacement library, D2): no curve, the
        // destination alone.
        Err(PlanError::OutOfRange) => Ok(MousePlan { steps: Vec::new(), end_at_ms: 0.0, seed }),
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_pieces(
    source: &dyn TrajectorySource,
    persona: &Persona,
    preset: Preset,
    ctx: &MouseContext,
    from: Point,
    to: Point,
    bias: &[f64],
    seed: u64,
) -> Result<MousePlan, PlanError> {
    let finite = [from.x, from.y, to.x, to.y, ctx.frame_hz, ctx.device_pixel_ratio, ctx.viewport.width, ctx.viewport.height, ctx.min_ms, ctx.max_ms, ctx.duration_mult];
    let sensor_ok = ctx.sensor.map_or(true, |s| s.poll_ms.is_finite() && s.poll_ms > 0.0 && s.count_px.is_finite() && s.count_px > 0.0);
    if !finite.iter().all(|v| v.is_finite()) || ctx.frame_hz <= 0.0 || ctx.device_pixel_ratio <= 0.0 || ctx.duration_mult <= 0.0 || !sensor_ok {
        return Err(PlanError::NonFinite);
    }
    let distance = (to.x - from.x).hypot(to.y - from.y);
    if distance < 1e-9 {
        return Ok(MousePlan { steps: Vec::new(), end_at_ms: 0.0, seed });
    }
    // Finite whatever the overrides say: an infinite phase would make the clock NaN (and the sampling loop endless).
    let pace = (persona.mouse.speed_factor * preset.time_mult() * ctx.duration_mult).min(MAX_PHASE_MS);
    let mv = match &persona.mouse.movement {
        Movement::Mouse(mv) => mv,
        Movement::Trackpad(tp) => return Ok(plan_trackpad(tp, pace, ctx, from, to, seed)),
    };
    let mut rng = Rng::new(seed);
    let options = |seed: u64| SourceOptions {
        frequency_hz: PATH_HZ,
        frequency_randomizer_ms: persona.mouse.frequency_randomizer_ms,
        seed,
        directness: persona.mouse.directness,
        // No recordings, no weights: an empty list would be a length the source rejects.
        selection_bias: (!bias.is_empty()).then_some(bias),
    };
    // cursory's way from a to b and its own progress along it; the still tail at b is no part of
    // the movement.
    let cursory_path = |a: Point, b: Point, rng: &mut Rng| -> Result<(Path, Vec<(f64, f64)>), PlanError> {
        let t = source.generate(a, b, &options(rng.next_u64())).map_err(PlanError::Source)?;
        // One point is no way to follow (a replacement library may answer so): the destination alone.
        if t.points.len() < 2 {
            return Err(PlanError::OutOfRange);
        }
        let mut points = vec![a];
        points.extend(t.points.iter().skip(1).take(t.points.len().saturating_sub(2)).copied());
        points.push(b);
        let path = Path::new(points);
        let moving = path.cumulative.iter().position(|&c| c >= path.length() - 1e-9).unwrap_or(0);
        let active = t.timings_ms.get(moving.min(t.timings_ms.len().saturating_sub(1))).copied().unwrap_or(0) as f64;
        let own = if active > 0.0 && path.length() > 0.0 && t.timings_ms.len() == t.points.len() {
            (0..=moving).map(|i| ((t.timings_ms[i] as f64 / active).min(1.0), path.cumulative[i] / path.length())).collect()
        } else {
            Vec::new()
        };
        Ok((path, own))
    };
    let unit = ((to.x - from.x) / distance, (to.y - from.y) / distance);
    let toward = |p: Point, d: f64| Point { x: p.x + unit.0 * d, y: p.y + unit.1 * d };
    let dist = |a: Point, b: Point| (b.x - a.x).hypot(b.y - a.y);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut t = 0.0;
    let mut cur = from;
    let left = dist(cur, to);
    let plain = left < PLAIN_MOVE_PX;

    // The slow start: a few percent of the way, halting now and then.
    if !plain && rng.chance(mv.lead_p) {
        let ms = phase(mv.lead.sample(&mut rng) * pace);
        let next = toward(cur, (left * mv.lead_share.sample(&mut rng)).min(0.2 * left));
        let stops = rng.poisson(mv.lead_stops);
        t = halting(&mut pieces, Path::new(vec![cur, next]), ms, t, stops, mv, pace, &mut rng);
        cur = next;
    }

    // The primary: a bell that ends short of the target or past it, a little to the side.
    let left = dist(cur, to);
    let reach = if ctx.overshoot { 1.5 } else { 1.0 };
    let (along, side) = if plain { (1.0, 0.0) } else { (rng.truncated(0.4, reach, |g| mv.reach.mean + mv.reach.sd * g.normal()), mv.lateral * rng.normal()) };
    let end = Point { x: cur.x + (unit.0 * along - unit.1 * side) * left, y: cur.y + (unit.1 * along + unit.0 * side) * left };
    let end = if plain { to } else { end };
    let ms = phase(mv.primary.sample(left, &mut rng) * pace);
    let (path, own) = cursory_path(cur, end, &mut rng)?;
    pieces.push(Piece { path, start_ms: t, ms, progress: Progress::Bell { bell: mv.bell, own } });
    t += ms;
    cur = end;

    // The correction onto the target, halting now and then.
    let left = dist(cur, to);
    if left > 1e-9 {
        let ms = phase(mv.correction.sample(left, &mut rng) * pace);
        let path = if left > STRAIGHT_PX { cursory_path(cur, to, &mut rng)?.0 } else { Path::new(vec![cur, to]) };
        let stops = rng.poisson(mv.correction_stops);
        t = halting(&mut pieces, path, ms, t, stops, mv, pace, &mut rng);
    }

    // The duration window bounds the whole movement (spec 5.4 p.2): every time scales together.
    let max_ms = ctx.max_ms.max(0.0);
    let end_at_ms = t.clamp(ctx.min_ms.clamp(0.0, max_ms), max_ms);
    let scale = if t > 0.0 { end_at_ms / t } else { 1.0 };
    for piece in &mut pieces {
        piece.start_ms *= scale;
        piece.ms *= scale;
    }
    Ok(MousePlan { steps: sample(&pieces, ctx, from, to, end_at_ms), end_at_ms, seed })
}

/// No phase of a movement lasts longer than this, ms: the window clamps the whole anyway.
const MAX_PHASE_MS: f64 = 1e9;

fn phase(ms: f64) -> f64 {
    ms.min(MAX_PHASE_MS)
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

fn exponential(rng: &mut Rng) -> f64 {
    -(1.0 - rng.unit()).ln()
}

/// One stroke of a trackpad over a chord of `d` px (decision D61): its reports as (along, sideways)
/// shares of the chord, the first at the start and the last on (1, 0), and its duration, ms. The
/// speed is a pulse in log time peaking at P over a slow floor β, jittered report to report; the way
/// follows a direction field — forward, then back once past the end when it overshoots — bent by a
/// sideways arch and correlated jitter, then turned and scaled as one so that it ends on the chord's
/// end.
fn pulse(tp: &TrackpadMovement, d: f64, last: bool, overshoot: bool, rng: &mut Rng) -> (Vec<(f64, f64)>, f64) {
    let dt = TRACKPAD_REPORT_MS;
    let z = [rng.normal(), rng.normal(), rng.normal()];
    let c = &tp.joint_cholesky;
    let (e_t, e_p, e_w) = (c[0] * z[0], c[1] * z[1], c[2] * z[0] + c[3] * z[2]);
    let x = (d.max(1.0) / 100.0).ln();
    let last = if last { 1.0 } else { 0.0 };
    let duration = (tp.duration[0] + tp.duration[1] * x + tp.duration[2] * last + e_t).exp();
    let peak = (tp.peak_time[0] + tp.peak_time[1] * x + e_p).exp();
    let width = (tp.effective_width[0] + tp.effective_width[1] * last + e_w).exp();
    let reports = (duration / dt).round().clamp(4.0, MAX_PULSE_REPORTS);
    let n = if reports.is_nan() { 4 } else { reports as usize };
    let duration = n as f64 * dt;
    let peak = peak.clamp(dt / 2.0, duration - dt / 2.0);
    let ratio = (duration / width).clamp(1.001, n as f64);
    let beta = sigmoid(tp.tail_fraction[0] + tp.tail_fraction[1] * ratio.ln() + tp.tail_fraction[2] * rng.normal());
    let target = (ratio - beta) / (1.0 - beta);
    let jitter: Vec<f64> = (0..n).map(|_| tp.speed_noise_scale * rng.normal()).collect();
    let weights = |sigma: f64| -> Vec<f64> {
        let logs: Vec<f64> = (0..n).map(|i| -(((i as f64 + 0.5) * dt / peak).ln().powi(2)) / (2.0 * sigma * sigma) + jitter[i]).collect();
        let top = logs.iter().cloned().fold(f64::MIN, f64::max);
        logs.iter().map(|v| (v - top).exp()).collect()
    };
    // The pulse's width in log time that gives the drawn peak over mean speed (60 halvings).
    let (mut lo, mut hi) = (0.001, 100.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let w = weights(mid);
        let peak_over_mean = w.iter().cloned().fold(0.0, f64::max) * n as f64 / w.iter().sum::<f64>();
        if peak_over_mean > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let w = weights((lo + hi) / 2.0);
    let total: f64 = w.iter().sum();
    let shares: Vec<f64> = w.iter().map(|v| beta / n as f64 + (1.0 - beta) * v / total).collect();

    let over = if rng.chance(sigmoid(tp.overshoot[0] + tp.overshoot[1] * x)) && overshoot {
        (tp.overshoot[2] + tp.overshoot[3] * x + tp.overshoot[4] * rng.normal()).exp().min(MAX_OVERSHOOT)
    } else {
        0.0
    };
    let u = rng.unit() - 0.5;
    let arch = tp.arch_scale * (-(1.0 - 2.0 * u.abs()).ln()).copysign(u);
    let [a_noise, b_noise, rho] = tp.lateral_noise;
    let mut v = rng.normal();
    let switch = (1.0 + over) / (1.0 + 2.0 * over);
    let mut progress = 0.0;
    let mut vectors = Vec::with_capacity(n);
    for &q in &shares {
        v = rho * v + (1.0 - rho * rho).sqrt() * rng.normal();
        let noise = (v * (a_noise * a_noise + b_noise * b_noise / (d * d * q * q).max(1e-20)).sqrt() / (1.0 + 2.0 * over)).atan();
        let mut cuts = vec![progress, progress + q];
        if progress < switch && switch < progress + q {
            cuts.insert(1, switch);
        }
        let (mut dx, mut dy) = (0.0, 0.0);
        for pair in cuts.windows(2) {
            let mid = (pair[0] + pair[1]) / 2.0;
            let along = (1.0 + 2.0 * over) * if mid <= switch { 1.0 } else { -1.0 };
            let side = std::f64::consts::PI * arch * (std::f64::consts::PI * mid).cos();
            let angle = side.atan2(along) + noise;
            dx += (pair[1] - pair[0]) * angle.cos();
            dy += (pair[1] - pair[0]) * angle.sin();
        }
        vectors.push((dx, dy));
        progress += q;
    }
    // One turn and scale for all, so the stroke ends on its chord's end.
    let (sx, sy) = vectors.iter().fold((0.0, 0.0), |(a, b), v| (a + v.0, b + v.1));
    let norm = (sx * sx + sy * sy).max(1e-20);
    let mut points = vec![(0.0, 0.0)];
    let (mut px, mut py) = (0.0, 0.0);
    for (dx, dy) in vectors {
        px += (dx * sx + dy * sy) / norm;
        py += (-dx * sy + dy * sx) / norm;
        points.push((px, py));
    }
    *points.last_mut().unwrap() = (1.0, 0.0);
    (points, duration)
}

/// A trackpad's movement (decision D61): the way in strokes with the finger lifted between them, each
/// stroke a pulse (above) reported every TRACKPAD_REPORT_MS from its start, halting now and then in
/// its slow stretches. A halt and the first report after a lift carry `rest`: the browser fills no
/// reports across them. Times scale with `pace` and the window, as a mouse's do.
fn plan_trackpad(tp: &TrackpadMovement, pace: f64, ctx: &MouseContext, from: Point, to: Point, seed: u64) -> MousePlan {
    let mut rng = Rng::new(seed);
    let distance = (to.x - from.x).hypot(to.y - from.y);
    let unit = ((to.x - from.x) / distance, (to.y - from.y) / distance);
    let strokes = 1 + rng.poisson(distance / tp.stroke_scale) as usize;
    let mut weights: Vec<f64> = (0..strokes).map(|_| exponential(&mut rng)).collect();
    weights.sort_by(|a, b| b.total_cmp(a));
    let total: f64 = weights.iter().sum();
    // The hand at each report: (ms, point, rest).
    let mut reports: Vec<(f64, Point, bool)> = Vec::new();
    let (mut t, mut cur) = (0.0, from);
    for (j, w) in weights.iter().enumerate() {
        let last = j == strokes - 1;
        let d = distance * w / total;
        let end = if last { to } else { Point { x: cur.x + unit.0 * d, y: cur.y + unit.1 * d } };
        // The first report is a sensor count along the way, the pulse starts there: a count and a 1/128 px
        // more, so the 1/256 px grid keeps it a count and the browser never takes it for a still hand.
        let onset = ((0.25 + 1.0 / 128.0) / unit.0.abs().max(unit.1.abs())).min(d);
        let start = Point { x: cur.x + unit.0 * onset, y: cur.y + unit.1 * onset };
        let chord = (end.x - start.x).hypot(end.y - start.y);
        let (points, duration) = pulse(tp, chord.max(1.0), last, ctx.overshoot, &mut rng);
        let steps: Vec<f64> = points.windows(2).map(|p| (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1)).collect();
        let fastest = steps.iter().cloned().fold(0.0, f64::max);
        let halt_p = sigmoid(tp.microgap[0] + tp.microgap[1] * (chord.max(1.0) / 100.0).ln());
        let (mut halt, mut held) = (0usize, start);
        for (i, &(a, b)) in points.iter().enumerate() {
            let holding = halt > 0 && i < points.len() - 1;
            let here = if holding {
                halt -= 1;
                held
            } else {
                held = Point { x: start.x + chord * (unit.0 * a - unit.1 * b), y: start.y + chord * (unit.1 * a + unit.0 * b) };
                if i < points.len() - 1 && steps[i] <= 0.15 * fastest && rng.chance(halt_p) {
                    halt = halt_reports(tp, &mut rng);
                }
                held
            };
            let here = if ctx.overshoot { here } else { short_of(here, from, unit, distance) };
            reports.push((t + TRACKPAD_REPORT_MS * (i + 1) as f64, here, holding || (j > 0 && i == 0)));
        }
        t += duration + TRACKPAD_REPORT_MS;
        cur = end;
        if !last {
            t += CLUTCH_FLOOR_MS + tp.clutch_scale * exponential(&mut rng);
        }
    }
    let max_ms = ctx.max_ms.max(0.0);
    let end_at_ms = (t * pace).clamp(ctx.min_ms.clamp(0.0, max_ms), max_ms);
    let scale = if t > 0.0 { end_at_ms / t } else { 1.0 };
    let grid = |v: f64| (v * ctx.device_pixel_ratio).round() / ctx.device_pixel_ratio;
    let destination = (grid(to.x), grid(to.y));
    let mut previous = (grid(from.x), grid(from.y));
    let (mut steps, mut rest) = (Vec::new(), false);
    for (at, p, lifted) in reports {
        rest |= lifted;
        let pixel = (grid(p.x), grid(p.y));
        // Overshoot off: the bound holds on the emitted pixel too (see `sample`).
        let pixel = if ctx.overshoot { pixel } else { nudged_back(pixel, from, unit, distance, ctx.device_pixel_ratio) };
        let inside = pixel.0 >= 0.0 && pixel.1 >= 0.0 && pixel.0 <= ctx.viewport.width - 1.0 && pixel.1 <= ctx.viewport.height - 1.0;
        if !inside || pixel == destination || pixel == previous || at * scale >= end_at_ms {
            continue;
        }
        steps.push(Step { x: pixel.0, y: pixel.1, at_ms: at * scale, rest });
        rest = false;
        previous = pixel;
    }
    MousePlan { steps: one_per_frame(steps, previous_of(from, ctx), ctx.frame_hz, end_at_ms), end_at_ms, seed }
}

/// How many reports a halt lasts: P(m) ∝ (m + c)^-α, m = 1…MAX_HALT_REPORTS.
fn halt_reports(tp: &TrackpadMovement, rng: &mut Rng) -> usize {
    let weight = |m: usize| (m as f64 + tp.microgap[3]).powf(-tp.microgap[2]);
    let total: f64 = (1..=MAX_HALT_REPORTS).map(weight).sum();
    let mut u = rng.unit() * total;
    for m in 1..=MAX_HALT_REPORTS {
        u -= weight(m);
        if u <= 0.0 {
            return m;
        }
    }
    MAX_HALT_REPORTS
}

/// `path` over `ms` as `stops + 1` pieces of random shares of the way, each a minimum-jerk bell, the
/// stops (the persona's dwell) taken out of `ms`: the hand moves a third of it at least. Returns
/// when it ends.
#[allow(clippy::too_many_arguments)]
fn halting(pieces: &mut Vec<Piece>, path: Path, ms: f64, start: f64, stops: u32, mv: &crate::persona::MovementTraits, pace: f64, rng: &mut Rng) -> f64 {
    let mut pauses: Vec<f64> = (0..stops).map(|_| phase(mv.dwell.sample(rng) * pace)).collect();
    let paused: f64 = pauses.iter().sum();
    let moving = (ms - paused).max(ms / 3.0);
    if paused > 0.0 {
        let fit = (ms - moving) / paused;
        pauses.iter_mut().for_each(|p| *p *= fit);
    }
    let weights: Vec<f64> = (0..=stops).map(|_| rng.unit() + 0.3).collect();
    let total: f64 = weights.iter().sum();
    let (mut t, mut s) = (start, 0.0);
    for (i, w) in weights.iter().enumerate() {
        if i > 0 {
            t += pauses[i - 1];
        }
        let share = w / total;
        let next = if i == weights.len() - 1 { path.length() } else { s + share * path.length() };
        pieces.push(Piece { path: path.slice(s, next), start_ms: t, ms: moving * share, progress: Progress::MinJerk });
        t += moving * share;
        s = next;
    }
    t
}

/// The steps the browser sends: the hand's position at each poll of the sensor (whole counts per
/// axis), or at each frame; on the physical pixel grid, inside the viewport, never the destination's
/// pixel nor the pixel the cursor already shows.
fn sample(pieces: &[Piece], ctx: &MouseContext, from: Point, to: Point, end_ms: f64) -> Vec<Step> {
    let grid = |v: f64| (v * ctx.device_pixel_ratio).round() / ctx.device_pixel_ratio;
    let distance = (to.x - from.x).hypot(to.y - from.y);
    let unit = ((to.x - from.x) / distance, (to.y - from.y) / distance);
    let destination = (grid(to.x), grid(to.y));
    let dt = ctx.sensor.map_or(1000.0 / ctx.frame_hz, |s| s.poll_ms);
    let hand = |t: f64| -> Point {
        // The last piece that started by `t` holds the hand (at its end during a pause after it).
        let i = pieces.partition_point(|p| p.start_ms <= t);
        if i == 0 {
            return from;
        }
        let p = &pieces[i - 1];
        let at = p.at(t - p.start_ms);
        if ctx.overshoot { at } else { short_of(at, from, unit, distance) }
    };
    let mut steps: Vec<Step> = Vec::new();
    let mut reported = from;
    let mut previous = (grid(from.x), grid(from.y));
    let mut k = 1;
    loop {
        let t = k as f64 * dt;
        if end_ms.is_nan() || t >= end_ms {
            break;
        }
        k += 1;
        let mut here = hand(t);
        if let Some(sensor) = ctx.sensor {
            let counts = |d: f64| sensor.count_px * (d / sensor.count_px).trunc();
            let (dx, dy) = (counts(here.x - reported.x), counts(here.y - reported.y));
            if dx == 0.0 && dy == 0.0 {
                continue;
            }
            here = Point { x: reported.x + dx, y: reported.y + dy };
            reported = here;
        }
        let pixel = (grid(here.x), grid(here.y));
        // Overshoot off: the bound holds on the emitted coordinates, after the sensor's counts and
        // the pixel grid, which can carry a point on the destination's plane past it.
        let pixel = if ctx.overshoot { pixel } else { nudged_back(pixel, from, unit, distance, ctx.device_pixel_ratio) };
        // Inside the last whole pixel: hit testing rounds to device pixels, and 1279.996 of a
        // 1280 px viewport lands outside it (a mouseleave of the document).
        let inside = pixel.0 >= 0.0 && pixel.1 >= 0.0 && pixel.0 <= ctx.viewport.width - 1.0 && pixel.1 <= ctx.viewport.height - 1.0;
        if !inside || pixel == destination || pixel == previous {
            continue;
        }
        steps.push(Step { x: pixel.0, y: pixel.1, at_ms: t, rest: false });
        previous = pixel;
    }
    if ctx.sensor.is_some() {
        steps
    } else {
        one_per_frame(steps, previous_of(from, ctx), ctx.frame_hz, end_ms)
    }
}

fn previous_of(from: Point, ctx: &MouseContext) -> (f64, f64) {
    let grid = |v: f64| (v * ctx.device_pixel_ratio).round() / ctx.device_pixel_ratio;
    (grid(from.x), grid(from.y))
}
/// Keeps the last step of each frame, as Chromium's rAF-aligned input would deliver them, and none
/// in the destination's frame; then drops a step on the pixel the cursor already shows (`start`
/// before the first), which the collapse can bring together. A dropped step's `rest` goes on to the
/// next one kept.
fn one_per_frame(steps: Vec<Step>, start: (f64, f64), frame_hz: f64, end_ms: f64) -> Vec<Step> {
    let frame = |t: f64| (t * frame_hz / 1000.0).floor();
    let mut kept: Vec<Step> = Vec::with_capacity(steps.len());
    for step in steps {
        match kept.last_mut() {
            Some(last) if frame(last.at_ms) == frame(step.at_ms) => *last = Step { rest: last.rest || step.rest, ..step },
            _ => kept.push(step),
        }
    }
    let mut rest = false;
    while kept.last().is_some_and(|s| frame(s.at_ms) == frame(end_ms)) {
        kept.pop();
    }
    let mut previous = start;
    let mut out = Vec::with_capacity(kept.len());
    for step in kept {
        rest |= step.rest;
        if (step.x, step.y) == previous {
            continue;
        }
        previous = (step.x, step.y);
        out.push(Step { rest, ..step });
        rest = false;
    }
    out
}
