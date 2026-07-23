//! Binary download + Ed25519 verification + extraction. See docs/SPEC-download.md.
//!
//! STUB — to be implemented.

use crate::error::Result;
use std::path::PathBuf;

/// Ensure the stealth Chromium binary is present, downloading + verifying if needed.
/// Returns the path to the chrome executable.
pub async fn ensure_binary(_browser_version: Option<&str>) -> Result<PathBuf> {
    unimplemented!("download::ensure_binary — see docs/SPEC-download.md")
}
