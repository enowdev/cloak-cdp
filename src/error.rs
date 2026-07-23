//! Shared error type for the crate.

use thiserror::Error;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CloakError>;

/// Top-level error type.
#[derive(Debug, Error)]
pub enum CloakError {
    #[error("unsupported platform: {system} {arch}")]
    UnsupportedPlatform { system: String, arch: String },

    #[error("invalid version pin: {0:?}")]
    InvalidVersion(String),

    #[error("binary not found at {0}")]
    BinaryNotFound(String),

    #[error("download failed: {0}")]
    Download(String),

    #[error("integrity verification failed: {0}")]
    Verification(String),

    #[error("archive extraction failed: {0}")]
    Extraction(String),

    #[error("path traversal detected in archive: {0}")]
    PathTraversal(String),

    #[error("geoip error: {0}")]
    GeoIp(String),

    #[error("proxy configuration error: {0}")]
    Proxy(String),

    #[error("browser launch failed: {0}")]
    Launch(String),

    #[error("widevine error: {0}")]
    Widevine(String),

    #[error("cloakserve error: {0}")]
    Serve(String),

    #[error("invalid fingerprint seed")]
    InvalidSeed,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Other(String),
}

impl CloakError {
    pub fn other(msg: impl Into<String>) -> Self {
        CloakError::Other(msg.into())
    }
}
