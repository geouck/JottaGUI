//! jotta-core: the engine layer of Jotta Desktop.
//!
//! Zero external dependencies on purpose:
//! - compiles/link anywhere (also inside a Flatpak builder sandbox),
//! - fully unit-testable without a running jottad,
//! - the GUI (gtk4-rs) and tray (ksni) crates are thin layers on top.
//!
//! Modules:
//! - `json`: minimal JSON parser/serializer (jotta-cli emits `--json`).
//! - `cli`: typed wrapper around the `jotta-cli` binary (Model B: we ship it).
//! - `daemon`: supervision of the bundled `jottad` daemon (start/stop/wait).
//! - `status`: tolerant extraction of status information from JSON.

pub mod cli;
pub mod daemon;
pub mod error;
pub mod json;
pub mod status;

pub use error::{Error, Result};
