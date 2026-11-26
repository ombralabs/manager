//! Scroll (spec 5.6): a CDP wheel request becomes the series the persona's wheel or trackpad would
//! give. The numbers are not guessed: `ScrollTable` holds them per OS × device, measured from the
//! stand's recordings (stages Э3, Э6) and shipped with the crate as JSON (`data/`); this module only
//! shapes the series. The cursor stays put.

use crate::persona::{Os, Persona, Preset, ScrollDevice};
use crate::rng::Rng;
use crate::PlanError;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The largest scroll planned per axis, px; a larger request goes unplanned (no person turns the
/// wheel for minutes).
pub const MAX_SCROLL_PX: f64 = 10_000.0; // prior
/// A pause between wheel bursts never shorter than this, ms: the stand's analyzer splits bursts at
/// gaps over 90 ms per notch (D22), and a shorter pause would read as one long burst.
pub const MIN_BURST_PAUSE_MS: f64 = 90.0;
/// A finger phase or its momentum never runs longer than this many events.
const MAX_PHASE_EVENTS: usize = 500; // prior
/// A plan never holds more steps than this, whatever the table says.
const MAX_STEPS: usize = 5_000; // prior
/// A wheel's flick never leaves the next one fewer ticks than this: the owner's shortest burst is
/// two detents of a tick each, so a smaller rest goes with the flick's last detent (D43, Э6в).
const MIN_FLICK_TICKS: u32 = 2; // prior

/// The `WebMouseWheelEvent::Phase` values a plan uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Phase {
    None,
    Began,
    Changed,
    Ended,
}

/// One wheel event; deltas in CSS px, signed as in `Input.dispatchMouseEvent`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct WheelStep {
    pub at_ms: f64,
    pub delta_x: f64,
    pub delta_y: f64,
    /// A mouse's whole ticks (wheelDelta / 120), the delta's size in notches; 0 for a trackpad.
    pub ticks: u32,
    pub phase: Phase,
    pub momentum_phase: Phase,
}

/// A wheel, per detent (Э6в): macOS sends an event a detent and puts the spin's speed in its whole
/// ticks — the owner's flick is 1, 1, then 4-8 ticks a detent, 16-51 ms apart —, so a burst is one
/// flick of the finger. The lists are quantiles evenly spaced in probability, drawn linearly between
/// them (D65).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WheelTable {
    /// Distance of one tick, px.
    pub notch_px: f64,
    /// Detents (events) per flick.
    pub burst_events: Vec<f64>,
    /// Ticks of a detent by its place in the flick: the 1st, the 2nd, …; the last list serves
    /// every later place.
    pub event_ticks: Vec<Vec<f64>>,
    /// Between a flick's detents, ms.
    pub event_interval_ms: Vec<f64>,
    /// Between flicks, the finger going back over the wheel; quantiles drawn like the rest (D65).
    pub burst_pause: Vec<f64>,
}

/// A trackpad: finger strokes (phase began → changed → ended), each followed by momentum.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TrackpadTable {
    /// Between consecutive events, ms: the device's rate, not the person's. Quantiles evenly
    /// spaced in probability (the stand writes the 1st to the 99th percentile), drawn linearly
    /// between them: a person's trackpad reports at 4-33 ms, not at one rate (stage Э6а).
    pub event_interval_ms: Vec<f64>,
    /// Distance under the finger per stroke, inclusive, px.
    pub stroke_px: (f64, f64),
    /// From a stroke's first event to the finger's lift, one interval on, ms: quantiles as
    /// `event_interval_ms` (Э6в: the owner's strokes are no log-normal — 36 with momentum at 110-164 ms
    /// (q10-q90), 6 lifts without it at 170-740 ms; the fit's sigma of 0.42 put a fifth of the strokes
    /// under the owner's shortest, 104 ms).
    pub stroke_duration: Vec<f64>,
    /// Ratio of consecutive momentum deltas, in (0, 1).
    pub momentum_decay: f64,
    /// Momentum stops below this delta, px.
    pub momentum_min_px: f64,
    /// Share of strokes momentum follows; the others end on the finger's lift (6 of 42 of the owner's).
    pub momentum_share: f64,
    /// From the end of a stroke's momentum to the next stroke, ms: quantiles as `event_interval_ms`
    /// (Э6в: the owner's pauses are no log-normal — median 76.7 ms against the fit's 120.7, a tail of
    /// 0.5-0.8 s after a lift without momentum).
    pub stroke_pause: Vec<f64>,
    /// The device reports whole pixels (macOS trackpads do, D22): deltas are rounded with the
    /// remainder carried on, so the series still lands on the request.
    #[serde(default)]
    pub integer_deltas: bool,
}

/// Calibration for one OS: the caller picks it by the persona's OS, not the host's (spec 5.6).
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollTable {
    pub wheel: WheelTable,
    pub trackpad: TrackpadTable,
}

/// The stand's `macos-scroll.json`: a half is `null` when the recordings had too few events of
/// that device (D22); the rest of the file (counts, reassignments) is not read.
#[derive(Deserialize)]
struct TableFile {
    wheel: Option<WheelTable>,
    trackpad: Option<TrackpadTable>,
}

impl WheelTable {
    /// Values the planner can shape and that end: a finite positive tick, quantiles it can draw
    /// from for every place.
    pub fn is_sound(&self) -> bool {
        self.notch_px.is_finite()
            && self.notch_px > 0.0
            && quantiles(&self.burst_events)
            && !self.event_ticks.is_empty()
            && self.event_ticks.iter().all(|q| quantiles(q))
            && quantiles(&self.event_interval_ms)
            && quantiles(&self.burst_pause)
    }
}

impl TrackpadTable {
    /// Finite positive sizes and intervals, momentum that decays.
    pub fn is_sound(&self) -> bool {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        quantiles(&self.event_interval_ms)
            && positive(self.stroke_px.0)
            && self.stroke_px.0 <= self.stroke_px.1
            && self.stroke_px.1.is_finite()
            && quantiles(&self.stroke_duration)
            && self.momentum_decay > 0.0
            && self.momentum_decay < 1.0
            && positive(self.momentum_min_px)
            && (0.0..=1.0).contains(&self.momentum_share)
            && quantiles(&self.stroke_pause)
    }
}

/// Quantiles `quantile_draw` can take: two at least, finite, positive, in order.
fn quantiles(q: &[f64]) -> bool {
    q.len() >= 2 && q.iter().all(|&v| v.is_finite() && v > 0.0) && q.windows(2).all(|w| w[0] <= w[1])
}

impl ScrollTable {
    /// Both halves sound (`WheelTable::is_sound`, `TrackpadTable::is_sound`); `plan_scroll` checks
    /// only the half of the persona's device, so one bad half leaves the other device planned.
    pub fn is_sound(&self) -> bool {
        self.wheel.is_sound() && self.trackpad.is_sound()
    }

    /// The stand's table (`qa/humanize/reference/<os>-scroll.json`); the error names what is
    /// missing or unsound, for the browser's log.
    pub fn from_json(json: &str) -> Result<ScrollTable, String> {
        let file: TableFile = serde_json::from_str(json).map_err(|e| format!("scroll table: {e}"))?;
        let wheel = file.wheel.ok_or("scroll table: no wheel half (too few mouse-wheel recordings)")?;
        let trackpad = file.trackpad.ok_or("scroll table: no trackpad half (too few trackpad recordings)")?;
        let table = ScrollTable { wheel, trackpad };
        if !table.wheel.is_sound() {
            return Err("scroll table: the wheel half is unsound".into());
        }
        if !table.trackpad.is_sound() {
            return Err("scroll table: the trackpad half is unsound".into());
        }
        Ok(table)
    }
}

/// The table shipped for `os`, parsed once; `Err` names why there is none (no recordings of that
/// OS yet — stage Э7 — or a table that does not parse), and the glue then sends scrolls as they are.
pub fn table_for(os: Os) -> Result<&'static ScrollTable, String> {
    static MACOS: OnceLock<Result<ScrollTable, String>> = OnceLock::new();
    match os {
        Os::Macos => MACOS.get_or_init(|| ScrollTable::from_json(include_str!("../data/scroll-macos.json"))).as_ref().map_err(Clone::clone),
        Os::Windows | Os::Linux => Err(format!("scroll table: none for {os:?} yet (stage Э7)")),
    }
}

/// Plans a scroll by `(delta_x, delta_y)` px with the persona's device. A wheel turns whole
/// notches, so its total is the request rounded to notches (at least one per moving axis); a
/// trackpad's total is the request exactly (rounded to whole pixels when its table says so). A
/// request beyond `MAX_SCROLL_PX` on an axis, an unsound table for the device or a plan beyond
/// `MAX_STEPS` is `OutOfRange`.
pub fn plan_scroll(persona: &Persona, preset: Preset, table: &ScrollTable, delta_x: f64, delta_y: f64, seed: u64) -> Result<Vec<WheelStep>, PlanError> {
    if !(delta_x.is_finite() && delta_y.is_finite()) {
        return Err(PlanError::NonFinite);
    }
    if delta_x.abs().max(delta_y.abs()) > MAX_SCROLL_PX {
        return Err(PlanError::OutOfRange);
    }
    let mut rng = Rng::new(seed);
    let t = preset.time_mult();
    match persona.scroll.device {
        ScrollDevice::Wheel if table.wheel.is_sound() => wheel(&table.wheel, delta_x, delta_y, t, &mut rng),
        ScrollDevice::Trackpad if table.trackpad.is_sound() => trackpad(&table.trackpad, delta_x, delta_y, t, &mut rng),
        _ => None,
    }
    .ok_or(PlanError::OutOfRange)
}

fn wheel(table: &WheelTable, delta_x: f64, delta_y: f64, t: f64, rng: &mut Rng) -> Option<Vec<WheelStep>> {
    let mut steps = Vec::new();
    let mut at = 0.0;
    // Vertical first: turning the wheel sideways is a separate motion (Shift or a tilt wheel). Prior.
    for (vertical, delta) in [(true, delta_y), (false, delta_x)] {
        if delta == 0.0 {
            continue;
        }
        let notch = table.notch_px.copysign(delta);
        let mut left = ((delta.abs() / table.notch_px).round() as u32).max(1);
        while left > 0 {
            if !steps.is_empty() {
                at += (quantile_draw(&table.burst_pause, rng) * t).max(MIN_BURST_PAUSE_MS);
            }
            // One flick: its detents, each with the ticks of its place; the request ends on a detent
            // that carries what is left, and a flick leaves the next one MIN_FLICK_TICKS at least.
            let detents = (quantile_draw(&table.burst_events, rng).round() as u32).max(1);
            for j in 0..detents {
                if left == 0 {
                    break;
                }
                let place = &table.event_ticks[(j as usize).min(table.event_ticks.len() - 1)];
                let mut ticks = (quantile_draw(place, rng).round() as u32).clamp(1, left);
                if j > 0 {
                    at += quantile_draw(&table.event_interval_ms, rng) * t;
                }
                left -= ticks;
                if j + 1 == detents && left < MIN_FLICK_TICKS {
                    ticks += left;
                    left = 0;
                }
                let d = notch * f64::from(ticks);
                let (delta_x, delta_y) = if vertical { (0.0, d) } else { (d, 0.0) };
                steps.push(WheelStep { at_ms: at, delta_x, delta_y, ticks, phase: Phase::None, momentum_phase: Phase::None });
                if steps.len() > MAX_STEPS {
                    return None;
                }
            }
        }
    }
    Some(steps)
}

/// A draw from quantiles evenly spaced in probability, linear between them (at least two).
fn quantile_draw(knots: &[f64], rng: &mut Rng) -> f64 {
    let x = rng.unit() * (knots.len() - 1) as f64;
    let i = (x as usize).min(knots.len() - 2);
    knots[i] + (knots[i + 1] - knots[i]) * (x - i as f64)
}

fn trackpad(table: &TrackpadTable, delta_x: f64, delta_y: f64, t: f64, rng: &mut Rng) -> Option<Vec<WheelStep>> {
    let total = delta_x.hypot(delta_y);
    if total == 0.0 {
        return Some(Vec::new());
    }
    let interval = |rng: &mut Rng| quantile_draw(&table.event_interval_ms, rng);
    // (time, distance, phase, momentum phase), scaled at the end to land on the request.
    let mut raw: Vec<(f64, f64, Phase, Phase)> = Vec::new();
    let (mut covered, mut at) = (0.0, 0.0);
    while covered < total {
        if !raw.is_empty() {
            at += quantile_draw(&table.stroke_pause, rng) * t;
        }
        let distance = rng.uniform(table.stroke_px.0, table.stroke_px.1);
        let duration = quantile_draw(&table.stroke_duration, rng) * t;
        // The device reports while the finger is down: an event counts if it comes before the
        // stroke's duration less half its own interval (with one fixed rate, round(duration /
        // interval) events), two at least (prior). The draw that ends the loop is the Ended's.
        let start = at;
        let mut ticks = vec![start];
        loop {
            let gap = interval(rng);
            at += gap;
            if ticks.len() >= MAX_PHASE_EVENTS || (ticks.len() >= 2 && at - start > duration - gap / 2.0) {
                break;
            }
            ticks.push(at);
        }
        let n = ticks.len();
        // The finger speeds up and lifts at its fastest (a quarter sine: prior); momentum carries
        // on from there.
        let weights: Vec<f64> = (1..=n).map(|j| (std::f64::consts::FRAC_PI_2 * j as f64 / n as f64).sin()).collect();
        let sum: f64 = weights.iter().sum();
        for (j, (w, tick)) in weights.iter().zip(&ticks).enumerate() {
            raw.push((*tick, distance * w / sum, if j == 0 { Phase::Began } else { Phase::Changed }, Phase::None));
            covered += distance * w / sum;
        }
        raw.push((at, 0.0, Phase::Ended, Phase::None));
        let mut delta = distance * weights[n - 1] / sum * table.momentum_decay;
        let mut momentum = 0;
        let carries = rng.chance(table.momentum_share);
        while carries && delta >= table.momentum_min_px && momentum < MAX_PHASE_EVENTS {
            at += interval(rng);
            raw.push((at, delta, Phase::None, if momentum == 0 { Phase::Began } else { Phase::Changed }));
            covered += delta;
            delta *= table.momentum_decay;
            momentum += 1;
        }
        if momentum > 0 {
            at += interval(rng);
            raw.push((at, 0.0, Phase::None, Phase::Ended));
        }
        if raw.len() > MAX_STEPS {
            return None;
        }
    }
    let scale = total / covered;
    let mut steps: Vec<WheelStep> = raw
        .into_iter()
        .map(|(at_ms, d, phase, momentum_phase)| WheelStep {
            at_ms,
            delta_x: d * scale * delta_x / total,
            delta_y: d * scale * delta_y / total,
            ticks: 0,
            phase,
            momentum_phase,
        })
        .collect();
    if table.integer_deltas {
        // Whole pixels with the remainder carried to the next event, so the sum stays the request
        // (rounded). No event goes against the finger: one the carry would turn around reports
        // nothing, and a Changed event that reports nothing is dropped (a slow finger reports on
        // fewer of the device's ticks); the first event of a stroke moves at least a pixel.
        let mut carry = (0.0, 0.0);
        for step in &mut steps {
            let began = step.phase == Phase::Began;
            let round = |own: f64, carry: &mut f64| {
                let want = own + *carry;
                let mut got = want.round();
                if got != 0.0 && got.signum() != own.signum() {
                    got = 0.0;
                }
                if got == 0.0 && began && own != 0.0 {
                    got = own.signum();
                }
                *carry = want - got;
                got + 0.0 // no negative zero
            };
            step.delta_x = round(step.delta_x, &mut carry.0);
            step.delta_y = round(step.delta_y, &mut carry.1);
        }
        steps.retain(|s| s.delta_x != 0.0 || s.delta_y != 0.0 || !(s.phase == Phase::Changed || s.momentum_phase == Phase::Changed));
    }
    Some(steps)
}
