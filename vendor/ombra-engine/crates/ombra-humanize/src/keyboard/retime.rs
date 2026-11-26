//! Retiming of client-typed keys (spec 6.4): `Input.dispatchKeyEvent` keeps its fields and order,
//! only its dispatch waits for the persona's rhythm.

use crate::persona::{LogNormal, Persona, Preset};
use crate::rng::Rng;

/// Shortest median flight (keyup to the next keydown), ms.
const MIN_FLIGHT_MS: f64 = 30.0; // prior

/// One per CDP session; times are the caller's monotonic clock in ms.
pub struct Retimer {
    rng: Rng,
    dwell: LogNormal,
    flight: LogNormal,
    time_mult: f64,
    last_down: Option<f64>,
    last_up: Option<f64>,
    /// The latest dispatch time returned: nothing goes out before it, so the order holds.
    last: f64,
}

impl Retimer {
    pub fn new(persona: &Persona, preset: Preset, seed: u64) -> Retimer {
        let k = &persona.keyboard;
        // Keys cannot overlap here, so keydown to keydown is hold + flight; keep its median at the
        // persona's speed.
        let flight = LogNormal { median: (1000.0 / k.keys_per_sec - k.dwell.median).max(MIN_FLIGHT_MS), sigma: k.iki_sigma };
        Retimer { rng: Rng::new(seed), dwell: k.dwell, flight, time_mult: preset.time_mult(), last_down: None, last_up: None, last: f64::MIN }
    }

    /// When to dispatch a keyDown (or rawKeyDown) that arrived at `now_ms`: not before the previous
    /// keyUp plus a flight. A client that waited longer goes through at once.
    pub fn key_down(&mut self, now_ms: f64) -> f64 {
        let earliest = self.last_up.map_or(now_ms, |up| up + self.flight.sample(&mut self.rng) * self.time_mult);
        let at = self.dispatch(now_ms, earliest);
        self.last_down = Some(at);
        at
    }

    /// When to dispatch a keyUp that arrived at `now_ms`: not before the keyDown plus a hold.
    pub fn key_up(&mut self, now_ms: f64) -> f64 {
        let earliest = self.last_down.map_or(now_ms, |down| down + self.dwell.sample(&mut self.rng) * self.time_mult);
        let at = self.dispatch(now_ms, earliest);
        self.last_up = Some(at);
        at
    }

    /// A key the tab sent another way (a typing plan): the next retimed key keeps the rhythm after it.
    pub fn follow(&mut self, down_ms: f64, up_ms: f64) {
        self.last_down = Some(down_ms);
        self.last_up = Some(up_ms);
        self.last = self.last.max(up_ms);
    }

    fn dispatch(&mut self, now_ms: f64, earliest: f64) -> f64 {
        self.last = now_ms.max(earliest).max(self.last);
        self.last
    }
}
