//! SuperEditr's platform-independent engine: model providers, prompts, diffing, the offline
//! checker, settings, and account detection. The desktop shell supplies text capture,
//! replacement, and UI for each operating system.

pub mod config;
pub mod detect;
pub mod diff;
pub mod edits;
pub mod engine;
pub mod guard;
pub mod install;
pub mod lang;
pub mod local;
pub mod prompts;
pub mod provider;
pub mod sensitive;
pub mod sse;
pub mod words;

pub use config::Config;
pub use engine::{Engine, Event, Job, Outcome};
