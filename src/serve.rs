//! cloakserve — CDP multiplexer (one Chrome per fingerprint seed, one public port).
//! See docs/SPEC-cloakserve-widevine.md (PART 1). Feature-gated behind `serve`.
//!
//! STUB — to be implemented.

use crate::error::Result;

/// Configuration for the multiplexer server.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub port: u16,
    pub headless: bool,
    pub data_dir: Option<String>,
    pub idle_timeout: f64,
    pub global_args: Vec<String>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        ServeConfig {
            port: 9222,
            headless: true,
            data_dir: None,
            idle_timeout: 0.0,
            global_args: Vec::new(),
        }
    }
}

/// Run the CDP multiplexer server (blocks until shutdown).
pub async fn run(_config: ServeConfig) -> Result<()> {
    unimplemented!("serve::run — see docs/SPEC-cloakserve-widevine.md")
}
