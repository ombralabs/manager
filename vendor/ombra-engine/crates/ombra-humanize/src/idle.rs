//! Idle drift (spec 5.7): in the `careful` preset the cursor moves a little when nothing has
//! happened in the tab for a while, as a resting hand does. The glue runs a drift as a move command
//! of the tab's queue, so it never comes during typing or any other plan, and arms the next one when
//! the queue goes idle again.

use crate::persona::{Persona, Preset};
use crate::rng::Rng;

/// One drift: when, counted from the moment the tab went idle, and by how much.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct IdlePlan {
    pub after_ms: f64,
    pub dx: f64,
    pub dy: f64,
}

/// Plans the next drift of the cursor at rest; `None` when the preset has no idle (spec 7.4:
/// `default` and `fast`). The distance is uniform up to the persona's `drift_px`, the direction
/// uniform; at least one pixel, so that the device has something to report.
pub fn plan_idle(persona: &Persona, preset: Preset, seed: u64) -> Option<IdlePlan> {
    if !preset.idle() {
        return None;
    }
    let mut rng = Rng::new(seed);
    let after_ms = persona.idle.after.sample(&mut rng);
    let distance = rng.uniform(1.0, persona.idle.drift_px.max(1.0));
    let angle = rng.uniform(0.0, std::f64::consts::TAU);
    Some(IdlePlan { after_ms, dx: distance * angle.cos(), dy: distance * angle.sin() })
}
