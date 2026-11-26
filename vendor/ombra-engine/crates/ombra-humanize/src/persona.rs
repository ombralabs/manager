//! The profile's persona (spec 7): distribution parameters derived from one seed.
//!
//! Decision D4: the seed comes from `--fingerprint-humanize-seed` (a random one when absent) and
//! the parameters are derived here, deterministically. Each parameter draws from its own labelled
//! stream (`Rng::for_label`), so adding a parameter never moves the existing ones. Keyboard priors
//! are the RAW Aalto calibration in `keyboard::priors` (D56). Other starting priors are marked
//! below. GENERATOR_VERSION 2 marks this calibration (D57, D66), GENERATOR_VERSION 3 marks Э6в's correction of the clutch (D71); golden personas are rebuilt
//! with every persona change.

use crate::keyboard::priors;
use crate::rng::{fnv1a, normal_stream, Rng};
use serde::{Deserialize, Serialize};

pub const GENERATOR_VERSION: u32 = 3;
/// Spread of the per-recording weights `exp(N(0, sigma))` (spec 7.3, `recording_affinity`).
pub const AFFINITY_SIGMA: f64 = 0.7;
/// Spread of the per-digraph multipliers `LogNormal(0, sigma)` (spec 6.3, the persona's signature).
pub const SIGNATURE_SIGMA: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

/// Multipliers on top of the persona, never a replacement (spec 7.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Default,
    Careful,
    Fast,
}

impl Preset {
    /// Multiplier on every duration and pause.
    pub fn time_mult(self) -> f64 {
        match self {
            Preset::Default => 1.0,
            Preset::Careful => 1.25,
            Preset::Fast => 0.7,
        }
    }

    /// `fast` types without typos.
    pub fn typos(self) -> bool {
        self != Preset::Fast
    }

    /// Only `careful` lets the cursor drift when nothing happens for a while (spec 5.7, 7.4).
    pub fn idle(self) -> bool {
        self == Preset::Careful
    }
}

/// A log-normal distribution by its median and log-scale sigma.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogNormal {
    pub median: f64,
    pub sigma: f64,
}

impl LogNormal {
    pub fn sample(&self, rng: &mut Rng) -> f64 {
        rng.lognormal(self.median, self.sigma)
    }

    fn is_sound(&self) -> bool {
        self.median.is_finite() && self.median > 0.0 && self.sigma.is_finite() && self.sigma >= 0.0
    }
}

/// A duration that follows a distance: `exp(a + b·ln d)` ms at the median (d in px, 1 px at least),
/// log-normal with `sigma` around it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistanceLaw {
    pub a: f64,
    pub b: f64,
    pub sigma: f64,
}

impl DistanceLaw {
    pub fn median(&self, distance: f64) -> f64 {
        (self.a + self.b * distance.max(1.0).ln()).exp()
    }

    pub fn sample(&self, distance: f64, rng: &mut Rng) -> f64 {
        rng.lognormal(self.median(distance), self.sigma)
    }

    /// A positive, finite median from 1 px to 10 000 px.
    fn is_sound(&self) -> bool {
        let fine = |d: f64| self.median(d).is_finite() && self.median(d) > 0.0;
        self.a.is_finite() && self.b.is_finite() && self.sigma.is_finite() && self.sigma >= 0.0 && fine(1.0) && fine(10_000.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Normal {
    pub mean: f64,
    pub sd: f64,
}

/// How a mouse's aimed movement unfolds (decision D61): cursory gives the path, these the time. An
/// optional slow start, the primary submovement (a bell that falls short of the target or passes
/// it), then the correction onto the target; the slow phases halt now and then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementTraits {
    /// Share of movements that start with a slow creep.
    pub lead_p: f64,
    /// The creep's duration, ms.
    pub lead: LogNormal,
    /// The share of the distance the creep covers.
    pub lead_share: LogNormal,
    /// Mean number of stops in the creep (Poisson).
    pub lead_stops: f64,
    /// The primary submovement's duration by its distance, ms.
    pub primary: DistanceLaw,
    /// Where the primary ends along the way, as a share of the distance: under 1 it falls short.
    pub reach: Normal,
    /// The primary's sideways miss: sd as a share of the distance.
    pub lateral: f64,
    /// The primary's speed: 1 a minimum-jerk bell, 0 the recording's own progress.
    pub bell: f64,
    /// The correction's duration by the distance left after the primary, ms.
    pub correction: DistanceLaw,
    /// Mean number of stops in the correction (Poisson).
    pub correction_stops: f64,
    /// One stop, ms.
    pub dwell: LogNormal,
}

/// How a trackpad's aimed movement unfolds (decision D61): a compact analytic model of the owner's
/// trackpad (Codex, stage Э6б), no recorded shape inside. The way goes in finger strokes; a stroke
/// is a speed pulse in log time over a slow floor, along a direction field with a sideways arch,
/// correlated jitter and a return past its end; slow stretches halt for a few reports. `x` below is
/// ln(d / 100 px) of a stroke's chord `d`, `last` 1 for the stroke that ends on the target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackpadMovement {
    /// Strokes of a way `D`: 1 + Poisson(D / stroke_scale); shares ~ Exp(1), longest first.
    pub stroke_scale: f64,
    /// The finger lifted between strokes: 200 ms + Exp(mean clutch_scale), ms.
    pub clutch_scale: f64,
    /// ln T = [0] + [1]·x + [2]·last + e_T: the stroke's duration, ms.
    pub duration: [f64; 3],
    /// ln P = [0] + [1]·x + e_P: when its speed peaks, ms.
    pub peak_time: [f64; 2],
    /// ln W = [0] + [1]·last + e_W: its path over its peak speed, ms.
    pub effective_width: [f64; 2],
    /// The residuals' Cholesky factor: e_T = [0]·z0, e_P = [1]·z1, e_W = [2]·z0 + [3]·z2.
    pub joint_cholesky: [f64; 4],
    /// logit β = [0] + [1]·ln(T / W) + [2]·z: the slow floor's share of the way.
    pub tail_fraction: [f64; 3],
    /// Log-speed jitter of a report.
    pub speed_noise_scale: f64,
    /// P(past the end) = sigmoid([0] + [1]·x); how far: ln o = [2] + [3]·x + [4]·z, a share of d.
    pub overshoot: [f64; 5],
    /// The sideways arch: Laplace(0, arch_scale), a share of d.
    pub arch_scale: f64,
    /// Direction jitter: [0] per share of the stroke, [1] px, AR(1) [2] report to report.
    pub lateral_noise: [f64; 3],
    /// A halt in a slow stretch: P = sigmoid([0] + [1]·x) per report; m reports long with
    /// P(m) ∝ (m + [3])^-[2], m = 1…23.
    pub microgap: [f64; 4],
}

impl TrackpadMovement {
    fn is_sound(&self) -> bool {
        let fine = |v: &[f64]| v.iter().all(|x| x.is_finite());
        let positive = |v: f64| v.is_finite() && v > 0.0;
        fine(&self.duration)
            && fine(&self.peak_time)
            && fine(&self.effective_width)
            && fine(&self.tail_fraction)
            && fine(&self.overshoot)
            && fine(&self.microgap)
            && positive(self.stroke_scale)
            && positive(self.clutch_scale)
            // A lift is seconds at most: an override of 1e308 ms would overflow the plan's clock.
            && self.clutch_scale <= 1e4
            // A Cholesky factor: its diagonal ([0], [1], [3]) is a spread, [2] a covariance of any sign.
            && fine(&self.joint_cholesky)
            && [0, 1, 3].iter().all(|&i| self.joint_cholesky[i] >= 0.0)
            && self.tail_fraction[2] >= 0.0
            && self.speed_noise_scale.is_finite()
            && (0.0..=1e3).contains(&self.speed_noise_scale)
            && self.overshoot[4] >= 0.0
            && self.arch_scale.is_finite()
            && self.arch_scale >= 0.0
            && fine(&self.lateral_noise)
            && self.lateral_noise[0] >= 0.0
            && self.lateral_noise[1] >= 0.0
            && (0.0..1.0).contains(&self.lateral_noise[2])
            && positive(self.microgap[2])
            && self.microgap[3] > -1.0
    }
}

/// A persona's movement: a mouse's or a trackpad's model, by its pointing device (`scroll.device`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Movement {
    Mouse(MovementTraits),
    Trackpad(TrackpadMovement),
}

impl MovementTraits {
    fn is_sound(&self) -> bool {
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        let rate = |v: f64| v.is_finite() && (0.0..=20.0).contains(&v);
        unit(self.lead_p)
            && self.lead.is_sound()
            && self.lead_share.is_sound()
            && rate(self.lead_stops)
            && self.primary.is_sound()
            && self.reach.mean.is_finite()
            && self.reach.sd.is_finite()
            && self.reach.sd >= 0.0
            // A share of the distance: 1e308 would send the primary's end 1e308 px aside.
            && unit(self.lateral)
            && unit(self.bell)
            && self.correction.is_sound()
            && rate(self.correction_stops)
            && self.dwell.is_sound()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDevice {
    Wheel,
    Trackpad,
}

/// Where the cursor rests in a new tab, as fractions of the viewport (spec 5.4 p.11).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestZone {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MouseTraits {
    /// Multiplier on movement duration.
    pub speed_factor: f64,
    /// Cursory's `directness`.
    pub directness: f64,
    /// Cursory's `frequency_randomizer`, ms.
    pub frequency_randomizer_ms: f64,
    /// Seed of the recording weights (`Persona::affinity_weights`).
    pub affinity_seed: u64,
    pub rest_zone: RestZone,
    pub movement: Movement,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickTraits {
    pub aim_sigma_px: f64,
    /// Cursor arrival to `mousePressed`, ms.
    pub pre_click: LogNormal,
    /// `mousePressed` to `mouseReleased`, ms.
    pub hold: LogNormal,
    /// Release of the first click to press of the second, ms.
    pub double_click: LogNormal,
    /// Probability of a 1 px drift while the button is held.
    pub drift_p: f64,
    /// Duration multiplier while dragging.
    pub drag_speed_mult: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollTraits {
    pub device: ScrollDevice,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardTraits {
    pub keys_per_sec: f64,
    /// Log-scale sigma of the inter-key interval.
    pub iki_sigma: f64,
    /// Key hold time, ms.
    pub dwell: LogNormal,
    /// Share of eligible key pairs typed with overlap.
    pub rollover: f64,
    /// Typos per character.
    pub typo_rate: f64,
    /// Typo to the first correcting Backspace, ms.
    pub correction: LogNormal,
    /// Interval multiplier after a word.
    pub word_pause_mult: f64,
    /// Probability of a 0.5–2 s pause before a word.
    pub thinking_pause_p: f64,
    /// Lean towards the left Shift (0) or the right one (1).
    pub shift_side_bias: f64,
    /// Seed of the digraph multipliers (`Persona::digraph_multipliers`).
    pub signature_seed: u64,
    /// Focus to the first key of a field, or to the paste chord in it, ms.
    pub first_key: LogNormal,
    pub paste: PasteTraits,
}

/// The paste chord, Command+V on macOS (decision D50).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasteTraits {
    /// The modifier's keydown to V's, ms.
    pub command_to_v: LogNormal,
    /// The modifier's keyup less V's, ms, normal per chord: below zero the modifier goes up first.
    pub release_mean_ms: f64,
    pub release_sd_ms: f64,
}

/// The cursor's drift when nothing happens for a while (spec 5.7): on in the `careful` preset only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdleTraits {
    /// From the last input of the tab to a drift, ms.
    pub after: LogNormal,
    /// The farthest a drift goes, px.
    pub drift_px: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Persona {
    pub generator_version: u32,
    pub seed: u64,
    pub os: Os,
    pub mouse: MouseTraits,
    pub click: ClickTraits,
    pub scroll: ScrollTraits,
    pub keyboard: KeyboardTraits,
    pub idle: IdleTraits,
}

/// The movement priors of a pointing device (D61). The mouse's centres come from cursory's own
/// recordings split the same way as the stand splits people's (5.34 + 0.04·ln d ms primary, 4.23 +
/// 0.37·ln r correction, a creep in 59 % of them). The trackpad's come from the owner's recordings,
/// the only ones there are: around the stand's fit of them, scales ±20 %, log intercepts and logits
/// ±0.25, slopes ±0.1. Every range holds the owner's own values (the owner is one of the people).
fn movement_priors(device: ScrollDevice, r: &dyn Fn(&str) -> Rng) -> Movement {
    let uniform = |label: &str, low: f64, high: f64| r(label).uniform(low, high);
    // Uniform below the centre half the time and above it the other half: the median stays on the
    // centre while the range reaches as far as the owner's values need.
    let around = |label: &str, low: f64, centre: f64, high: f64| {
        let mut g = r(label);
        if g.chance(0.5) { g.uniform(low, centre) } else { g.uniform(centre, high) }
    };
    let law = |label: &str, a: (f64, f64, f64), b: (f64, f64, f64), sigma: (f64, f64, f64)| DistanceLaw {
        a: around(&format!("{label}.a"), a.0, a.1, a.2),
        b: around(&format!("{label}.b"), b.0, b.1, b.2),
        sigma: around(&format!("{label}.sigma"), sigma.0, sigma.1, sigma.2),
    };
    let ln = |label: &str, median: (f64, f64, f64), sigma: (f64, f64)| LogNormal {
        median: around(&format!("{label}.median"), median.0, median.1, median.2),
        sigma: uniform(&format!("{label}.sigma"), sigma.0, sigma.1),
    };
    let m = "mouse.movement";
    match device {
        ScrollDevice::Wheel => Movement::Mouse(MovementTraits {
            // (low, cursory's, high); the owner's values (the stand's fit of mouse-75) are inside.
            lead_p: around(&format!("{m}.lead_p"), 0.45, 0.59, 0.9),
            lead: ln(&format!("{m}.lead"), (70.0, 101.0, 180.0), (0.5, 0.85)),
            lead_share: ln(&format!("{m}.lead_share"), (0.01, 0.017, 0.03), (0.6, 1.0)),
            lead_stops: uniform(&format!("{m}.lead_stops"), 0.2, 0.9), // the owner's alone: cursory's are not split by stops
            primary: law(&format!("{m}.primary"), (5.1, 5.34, 5.9), (-0.1, 0.038, 0.1), (0.3, 0.399, 0.5)),
            reach: Normal {
                mean: around(&format!("{m}.reach.mean"), 0.88, 0.909, 0.96),
                sd: around(&format!("{m}.reach.sd"), 0.08, 0.113, 0.14),
            },
            lateral: around(&format!("{m}.lateral"), 0.03, 0.063, 0.07),
            bell: uniform(&format!("{m}.bell"), 0.6, 1.0),
            correction: law(&format!("{m}.correction"), (4.0, 4.229, 4.5), (0.3, 0.372, 0.5), (0.4, 0.455, 0.7)),
            correction_stops: uniform(&format!("{m}.correction_stops"), 0.3, 1.0),
            dwell: ln(&format!("{m}.dwell"), (50.0, 75.0, 110.0), (0.35, 0.55)),
        }),
        ScrollDevice::Trackpad => {
            let t = "mouse.movement.trackpad";
            // The owner's trackpad (qa/humanize/reference/macos-humans.json, "fit" of trackpad-120).
            let scale = |label: &str, v: f64| uniform(&format!("{t}.{label}"), 0.8 * v, 1.2 * v);
            let shift = |label: &str, v: f64, by: f64| uniform(&format!("{t}.{label}"), v - by, v + by);
            Movement::Trackpad(TrackpadMovement {
                stroke_scale: scale("stroke_scale", 909.43),
                clutch_scale: scale("clutch_scale", 31.45),
                duration: [shift("duration.0", 4.7186, 0.25), shift("duration.1", 0.1674, 0.1), shift("duration.2", 1.3366, 0.1)],
                peak_time: [shift("peak_time.0", 3.8869, 0.25), shift("peak_time.1", 0.2445, 0.1)],
                effective_width: [shift("effective_width.0", 4.1361, 0.25), shift("effective_width.1", 0.3726, 0.1)],
                joint_cholesky: [
                    scale("joint_cholesky.0", 0.4520),
                    scale("joint_cholesky.1", 0.4372),
                    scale("joint_cholesky.2", 0.1683),
                    scale("joint_cholesky.3", 0.2511),
                ],
                tail_fraction: [shift("tail_fraction.0", -1.5383, 0.25), shift("tail_fraction.1", 0.1972, 0.1), scale("tail_fraction.2", 0.5926)],
                speed_noise_scale: scale("speed_noise_scale", 0.3032),
                overshoot: [
                    shift("overshoot.0", -1.0085, 0.25),
                    shift("overshoot.1", -0.2021, 0.1),
                    shift("overshoot.2", -2.1140, 0.25),
                    shift("overshoot.3", -0.4308, 0.1),
                    scale("overshoot.4", 1.2087),
                ],
                arch_scale: scale("arch_scale", 0.0944),
                lateral_noise: [scale("lateral_noise.0", 0.1733), scale("lateral_noise.1", 0.3144), uniform(&format!("{t}.lateral_noise.2"), 0.8, 0.95)],
                microgap: [
                    shift("microgap.0", -1.8376, 0.25),
                    shift("microgap.1", -0.5348, 0.1),
                    scale("microgap.2", 1.2534),
                    shift("microgap.3", -0.6379, 0.2),
                ],
            })
        }
    }
}

impl Persona {
    pub fn from_seed(seed: u64, os: Os) -> Persona {
        let r = |label: &str| Rng::for_label(seed, label);
        let uniform = |label: &str, low: f64, high: f64| r(label).uniform(low, high);
        let keys_per_sec = priors::KEYS_PER_SEC.sample(seed, "keyboard.keys_per_sec", 0.0);
        // D16's eligible-pair probability, not the stand's all-pair rollover share.
        let rollover = priors::ROLLOVER.sample(seed, "keyboard.rollover", keys_per_sec);
        let trackpad_p = match os {
            Os::Macos => 0.7,
            Os::Windows => 0.1,
            Os::Linux => 0.2, // prior
        };
        let device = if r("scroll.device").chance(trackpad_p) { ScrollDevice::Trackpad } else { ScrollDevice::Wheel };
        let movement = movement_priors(device, &r);
        Persona {
            generator_version: GENERATOR_VERSION,
            seed,
            os,
            mouse: MouseTraits {
                speed_factor: r("mouse.speed_factor").truncated(0.75, 1.35, |g| (0.15 * g.normal()).exp()),
                directness: r("mouse.directness").truncated(0.4, 0.9, |g| g.beta(6.5, 3.5)),
                frequency_randomizer_ms: uniform("mouse.frequency_randomizer_ms", 0.0, 2.0),
                affinity_seed: r("mouse.affinity").next_u64(),
                rest_zone: RestZone {
                    x: uniform("mouse.rest_zone.x", 0.35, 0.75), // prior
                    y: uniform("mouse.rest_zone.y", 0.35, 0.75), // prior
                    radius: uniform("mouse.rest_zone.radius", 0.05, 0.15), // prior
                },
                movement,
            },
            click: ClickTraits {
                aim_sigma_px: uniform("click.aim_sigma_px", 1.5, 4.0),
                // Stage Э6б: the owner's sessions, 37.8–48.9 ms with log-sd 0.74–1.23, fall inside.
                pre_click: LogNormal {
                    median: uniform("click.pre_click.median", 30.0, 220.0),
                    sigma: uniform("click.pre_click.sigma", 0.3, 1.25),
                },
                hold: LogNormal {
                    median: uniform("click.hold.median", 60.0, 130.0),
                    sigma: uniform("click.hold.sigma", 0.15, 0.3), // prior
                },
                double_click: LogNormal {
                    median: uniform("click.double_click.median", 90.0, 180.0),
                    sigma: uniform("click.double_click.sigma", 0.1, 0.45), // the owner's 0.12–0.42
                },
                drift_p: uniform("click.drift_p", 0.02, 0.08), // prior
                drag_speed_mult: uniform("click.drag_speed_mult", 1.2, 1.7),
            },
            scroll: ScrollTraits { device },
            keyboard: KeyboardTraits {
                keys_per_sec,
                iki_sigma: priors::IKI_SIGMA.sample(seed, "keyboard.iki_sigma", keys_per_sec),
                dwell: LogNormal {
                    median: priors::DWELL_MEDIAN.sample(seed, "keyboard.dwell.median", keys_per_sec),
                    sigma: priors::DWELL_SIGMA.sample(seed, "keyboard.dwell.sigma", keys_per_sec),
                },
                rollover,
                typo_rate: priors::TYPO_RATE.sample(seed, "keyboard.typo_rate", keys_per_sec),
                correction: LogNormal {
                    median: uniform("keyboard.correction.median", 200.0, 500.0),
                    sigma: uniform("keyboard.correction.sigma", 0.25, 0.4), // prior
                },
                word_pause_mult: priors::WORD_PAUSE_MULT.sample(seed, "keyboard.word_pause_mult", keys_per_sec),
                thinking_pause_p: priors::THINKING_PAUSE_P.sample(seed, "keyboard.thinking_pause_p", keys_per_sec),
                shift_side_bias: r("keyboard.shift_side_bias").beta(2.0, 2.0),
                signature_seed: r("keyboard.signature").next_u64(),
                // Stage Э6б, the owner's Command+V after a click into the field: 274 and 734 ms, log-sd 0.39–0.49.
                first_key: LogNormal {
                    median: uniform("keyboard.first_key.median", 250.0, 750.0),
                    sigma: uniform("keyboard.first_key.sigma", 0.2, 0.55),
                },
                // Stage Э6б, the owner's Command+V: V 246 and 578 ms after Command (log-sd 0.31–0.39), Command up
                // 38 ms before V and 41 ms after it (sd 17–18).
                paste: PasteTraits {
                    command_to_v: LogNormal {
                        median: uniform("keyboard.paste.command_to_v.median", 150.0, 650.0),
                        sigma: uniform("keyboard.paste.command_to_v.sigma", 0.2, 0.45),
                    },
                    release_mean_ms: uniform("keyboard.paste.release_mean_ms", -60.0, 80.0),
                    release_sd_ms: uniform("keyboard.paste.release_sd_ms", 8.0, 30.0),
                },
            },
            idle: IdleTraits {
                after: LogNormal {
                    median: uniform("idle.after.median", 8_000.0, 20_000.0), // prior
                    sigma: uniform("idle.after.sigma", 0.3, 0.5),            // prior
                },
                drift_px: uniform("idle.drift_px", 2.0, 6.0), // prior
            },
        }
    }

    /// Values every planner can work with: finite, positive where a size or a rate is meant,
    /// probabilities and fractions within [0, 1]. A persona built here is sound; one a client
    /// overrode (`Ombra.setHumanize`) is checked with this before it replaces the tab's.
    pub fn is_sound(&self) -> bool {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        let (m, c, k, i) = (&self.mouse, &self.click, &self.keyboard, &self.idle);
        positive(m.speed_factor)
            && unit(m.directness)
            && m.frequency_randomizer_ms.is_finite()
            && (0.0..=1000.0).contains(&m.frequency_randomizer_ms)
            && unit(m.rest_zone.x)
            && unit(m.rest_zone.y)
            // The model and the device go together: the glue reports by the device (D61).
            && match (&m.movement, self.scroll.device) {
                (Movement::Mouse(mv), ScrollDevice::Wheel) => mv.is_sound(),
                (Movement::Trackpad(tp), ScrollDevice::Trackpad) => tp.is_sound(),
                _ => false,
            }
            && m.rest_zone.radius.is_finite()
            && (0.0..=0.5).contains(&m.rest_zone.radius)
            && positive(c.aim_sigma_px)
            && c.pre_click.is_sound()
            && c.hold.is_sound()
            && c.double_click.is_sound()
            && unit(c.drift_p)
            && positive(c.drag_speed_mult)
            && positive(k.keys_per_sec)
            && positive(k.iki_sigma)
            && k.dwell.is_sound()
            && unit(k.rollover)
            && unit(k.typo_rate)
            && k.correction.is_sound()
            && positive(k.word_pause_mult)
            && unit(k.thinking_pause_p)
            && unit(k.shift_side_bias)
            && k.first_key.is_sound()
            && k.paste.command_to_v.is_sound()
            // Seconds at most: a release of 1e308 ms would hold Command and the tab's input queue for good. Far
            // outside the priors (medians up to 750 ms, log-sd 0.55, release −60…80 ms, sd 30 ms) and the owner's
            // values (1.78 s before the first key in the typing scene, log-sd 0.49, release ±41 ms, sd 18 ms).
            && [k.first_key, k.paste.command_to_v].iter().all(|d| d.median <= 5e3 && d.sigma <= 1.0)
            && k.paste.release_mean_ms.abs() <= 2e3
            && (0.0..=1e3).contains(&k.paste.release_sd_ms)
            && i.after.is_sound()
            && positive(i.drift_px)
    }

    /// Cursory `selection_bias` for a dataset of `records` recordings: `exp(N(0, AFFINITY_SIGMA))`
    /// per record, so one persona keeps leaning on the same recordings (spec 7.3).
    pub fn affinity_weights(&self, records: usize) -> Vec<f64> {
        normal_stream(self.mouse.affinity_seed).take(records).map(|z| (AFFINITY_SIGMA * z).exp()).collect()
    }

    /// Personal multipliers for the `count` most frequent digraphs of a layout (spec 6.3, D5).
    pub fn digraph_multipliers(&self, layout_id: &str, count: usize) -> Vec<f64> {
        normal_stream(self.keyboard.signature_seed ^ fnv1a(layout_id)).take(count).map(|z| (SIGNATURE_SIGMA * z).exp()).collect()
    }
}
