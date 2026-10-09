//! Coordination state for Bot teams.
//!
//! The gateway deliberately does not need to know how assignments are queued or
//! how a `send_msg` mention becomes a hand-off.  [`Orchestrator::rpc`] is a
//! small JSON adapter around the typed state machine and is also useful for the
//! mock gateway.

mod model;
mod state;

pub use model::*;
pub use state::{Orchestrator, OrchestratorError, OrchestratorSettings};
