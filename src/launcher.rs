//! `launch()` — ensure binary, build args, spawn + connect CDP. See docs/SPEC-core.md.
//!
//! STUB — to be implemented.

use crate::error::Result;
use crate::LaunchOptions;
use chromiumoxide::{Browser, Handler};

/// Launch stealth Chromium and connect over CDP.
///
/// Returns the `Browser` and its `Handler` (the caller must drive the handler,
/// typically via `tokio::spawn`).
pub async fn launch(_opts: LaunchOptions) -> Result<(Browser, Handler)> {
    unimplemented!("launcher::launch — see docs/SPEC-core.md")
}
