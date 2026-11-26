//! Click, double click and drag (spec 5.5).

use crate::persona::{LogNormal, Persona, Preset};
use crate::rng::Rng;
use crate::Point;

/// Press to press of a double click stays under this, below the 500 ms OS default.
pub const DOUBLE_CLICK_MAX_MS: f64 = 450.0; // prior
/// The shortest release-to-press gap of a double click.
const MIN_GAP_MS: f64 = 30.0; // prior
/// Press to the start of a drag's movement. Stage Э6б: the owner's 12 drags moved 3.9–129 ms after the press (median
/// 9 ms), not the spec's 100–300.
pub const DRAG_PAUSE: LogNormal = LogNormal { median: 20.0, sigma: 1.0 }; // prior
/// How far past the aim radius a first click still counts as near the target: the first aim as it
/// went out, rounded to the device's 1/256 px grid, can land a hair outside (D27).
const NEAR_SLACK_PX: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClickOptions {
    /// Offset the aim from the client's point (off for canvas apps that need the exact pixel).
    pub aim: bool,
    /// Largest aim offset, px.
    pub aim_radius_px: f64,
}

impl Default for ClickOptions {
    fn default() -> Self {
        ClickOptions { aim: true, aim_radius_px: 3.0 }
    }
}

/// A 1 px slip while the button is down.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Drift {
    /// After the press.
    pub at_ms: f64,
    pub to: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct ClickPlan {
    /// Where the press lands, and the release too unless `drift` moves the cursor first (then at
    /// `drift.to`).
    pub aim: Point,
    /// Cursor arrival to `mousePressed`.
    pub pre_press_ms: f64,
    /// `mousePressed` to `mouseReleased`; a client release that comes sooner waits for it.
    pub hold_ms: f64,
    pub drift: Option<Drift>,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DoubleClickPlan {
    pub first: ClickPlan,
    /// Release of the first click to the press of the second.
    pub gap_ms: f64,
    /// Within 2 px of the first aim and inside the aim radius around the target; the first aim
    /// itself when aiming is off.
    pub second_aim: Point,
    pub second_hold_ms: f64,
}

/// The second click of a double click, after a first that landed and was held as given.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct SecondClickPlan {
    /// Release of the first click to the press of this one.
    pub gap_ms: f64,
    pub aim: Point,
    pub hold_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DragPlan {
    /// Press to the start of the movement.
    pub after_press_ms: f64,
    /// Multiplier on the drag movement's duration: pass it as `MouseContext::duration_mult`.
    pub speed_mult: f64,
}

/// A point around `center` from an isotropic Gaussian with `sigma`, truncated to `radius`: the
/// distance drawn by inverting the truncated Rayleigh CDF, the angle uniform. Exact, no redraw
/// loop, never a fallback onto the centre.
fn offset(center: Point, sigma: f64, radius: f64, rng: &mut Rng) -> Point {
    if !(radius > 0.0 && sigma > 0.0) {
        return center;
    }
    let cap = 1.0 - (-radius * radius / (2.0 * sigma * sigma)).exp();
    let r = sigma * (-2.0 * (1.0 - rng.unit() * cap).ln()).sqrt();
    let angle = std::f64::consts::TAU * rng.unit();
    Point { x: center.x + r * angle.cos(), y: center.y + r * angle.sin() }
}

pub fn plan_click(persona: &Persona, preset: Preset, target: Point, options: &ClickOptions, seed: u64) -> ClickPlan {
    let mut rng = Rng::new(seed);
    let c = &persona.click;
    let aim = if options.aim { offset(target, c.aim_sigma_px, options.aim_radius_px, &mut rng) } else { target };
    let pre_press_ms = c.pre_click.sample(&mut rng) * preset.time_mult();
    let hold_ms = c.hold.sample(&mut rng) * preset.time_mult();
    let drift = if rng.chance(c.drift_p) {
        let (dx, dy) = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)][rng.below(4)];
        Some(Drift { at_ms: rng.uniform(0.2, 0.8) * hold_ms, to: Point { x: aim.x + dx, y: aim.y + dy } }) // prior: drift window 0.2-0.8
    } else {
        None
    };
    ClickPlan { aim, pre_press_ms, hold_ms, drift }
}

pub fn plan_double_click(persona: &Persona, preset: Preset, target: Point, options: &ClickOptions, seed: u64) -> DoubleClickPlan {
    let first = plan_click(persona, preset, target, options, seed);
    // Press to press stays under DOUBLE_CLICK_MAX_MS. A first hold that leaves no room for the
    // shortest gap is drawn again from the same distribution (a truncation, not a clamp).
    let hold_cap = DOUBLE_CLICK_MAX_MS - MIN_GAP_MS;
    let hold_ms = if first.hold_ms <= hold_cap {
        first.hold_ms
    } else {
        Rng::for_label(seed, "click.first_hold").truncated(0.0, hold_cap, |g| persona.click.hold.sample(g) * preset.time_mult())
    };
    let first = ClickPlan { drift: None, hold_ms, ..first };
    let second = plan_second_click(persona, preset, target, first.aim, hold_ms, options, seed);
    DoubleClickPlan { first, gap_ms: second.gap_ms, second_aim: second.aim, second_hold_ms: second.hold_ms }
}

/// The second click of a double click whose first click landed on `first_aim` and was held
/// `first_hold_ms` — what the browser plans when the client's second press comes after a first
/// click it already ran. The gap takes what the first hold left of DOUBLE_CLICK_MAX_MS, the
/// shortest gap at least. The press lands near the first one when that was within the aim radius
/// of `target` (or a grid step past it); otherwise (a first click far from the target, or aiming
/// off) on `target` itself.
pub fn plan_second_click(
    persona: &Persona,
    preset: Preset,
    target: Point,
    first_aim: Point,
    first_hold_ms: f64,
    options: &ClickOptions,
    seed: u64,
) -> SecondClickPlan {
    let mut rng = Rng::for_label(seed, "click.second");
    let t = preset.time_mult();
    let gap_ms = rng.truncated(MIN_GAP_MS, (DOUBLE_CLICK_MAX_MS - first_hold_ms).max(MIN_GAP_MS), |g| persona.click.double_click.sample(g) * t);
    // Within 2 px of the first click and inside the aim radius around the target, so a target the
    // first click hits the second hits too; a first click a hair past the radius gets no room and
    // the second lands on it. A first click outside the aim radius (or aiming off) is no footing
    // for that: the second lands on the target itself.
    let near = options.aim && (first_aim.x - target.x).hypot(first_aim.y - target.y) <= options.aim_radius_px + NEAR_SLACK_PX;
    let room = if options.aim {
        (options.aim_radius_px - (first_aim.x - target.x).hypot(first_aim.y - target.y)).clamp(0.0, 2.0)
    } else {
        0.0
    };
    let aim = if near { offset(first_aim, 0.7, room, &mut rng) } else { target }; // prior: sigma 0.7 px
    let hold_ms = persona.click.hold.sample(&mut rng) * t;
    SecondClickPlan { gap_ms, aim, hold_ms }
}

pub fn plan_drag(persona: &Persona, preset: Preset, seed: u64) -> DragPlan {
    let mut rng = Rng::new(seed);
    DragPlan { after_press_ms: DRAG_PAUSE.sample(&mut rng) * preset.time_mult(), speed_mult: persona.click.drag_speed_mult }
}
