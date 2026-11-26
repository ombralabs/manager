//! Persona and input planners behind Ombra's humanized CDP input
//! (spec: docs/superpowers/specs/2026-10-06-humanize-input-design.md, decisions D1–D17, D43–D47).
//!
//! No browser dependencies: the browser glue executes the plans this crate returns. Every planner
//! takes an explicit `seed`; the caller draws it from a CSPRNG per action (spec 5.4 p.13), so a
//! logged seed replays the action exactly.

pub mod click;
pub mod idle;
pub mod keyboard;
pub mod mouse;
pub mod persona;
pub mod rng;
pub mod scroll;

pub use persona::{Os, Persona, Preset};

/// A point in CSS pixels of the top-level viewport.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Why a planner gave up; the glue then sends the client's event as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlanError {
    /// A coordinate or a context value is NaN or infinite, or the frame rate, the DPR or the
    /// duration multiplier is not positive.
    NonFinite,
    /// The trajectory source failed; the caller sends the destination without a curve.
    Source(mouse::SourceError),
    /// A request or a calibration value the planner does not shape (a scroll beyond
    /// `scroll::MAX_SCROLL_PX`, a scroll table that would not end): the caller sends the event as
    /// it is.
    OutOfRange,
}
