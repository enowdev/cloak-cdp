//! Widevine CDM hint-seeding + fetcher. See docs/SPEC-cloakserve-widevine.md (PART 2).
//! Linux-only.
//!
//! STUB — to be implemented.

use crate::error::Result;
use std::path::{Path, PathBuf};

/// Resolve the Widevine CDM directory (env override → binary dir → cache dir).
pub fn resolve_widevine_cdm_dir(_binary_path: &Path) -> Option<PathBuf> {
    unimplemented!("widevine::resolve_widevine_cdm_dir — see spec")
}

/// Seed the Widevine hint file into a persistent profile (no-op off Linux). Never fails hard.
pub fn seed_widevine_hint(_user_data_dir: &Path, _binary_path: &Path) -> Result<()> {
    unimplemented!("widevine::seed_widevine_hint — see spec")
}
