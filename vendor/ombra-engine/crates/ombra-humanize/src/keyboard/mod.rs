//! Keyboard input (spec 6): layouts, the typing planner and retiming.

pub mod digraphs;
pub mod layout;
pub(crate) mod priors;
pub mod retime;
pub mod timing;
pub mod typing;
pub mod typos;

pub use retime::Retimer;
pub use typing::{plan_typing, Action, Field, Key, Previous, Role, TimedAction, TypingContext, TypingPlan};
