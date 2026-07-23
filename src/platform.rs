//! Platform detection, version map, cache/binary path resolution.
//!
//! Mirrors the load-bearing constants and helpers from `cloakbrowser/config.py`.
//! Ported strings must stay byte-exact: cache dir `~/.cloakbrowser`, binary
//! subdir `chromium-{version}` (NO `v`), URL segment `chromium-v{version}`
//! (WITH `v`), archive `cloakbrowser-{tag}{.tar.gz|.zip}`.

use crate::error::{CloakError, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::PathBuf;

/// Latest version across all platforms (display/fallback).
pub const CHROMIUM_VERSION: &str = "146.0.7680.177.5";

/// Base download host (overridable via `CLOAKBROWSER_DOWNLOAD_URL`).
pub const DEFAULT_DOWNLOAD_BASE_URL: &str = "https://cloakbrowser.dev";
pub const GITHUB_API_URL: &str = "https://api.github.com/repos/CloakHQ/cloakbrowser/releases";
pub const GITHUB_DOWNLOAD_BASE_URL: &str =
    "https://github.com/CloakHQ/cloakbrowser/releases/download";

/// Base64 of 32-byte raw Ed25519 public key(s) that sign SHA256SUMS.
pub const BINARY_SIGNING_PUBKEYS: &[&str] = &["MKFKwIhUcKWq5xTuNA0Ovg99njcDEcEJvmWYYhApvaU="];

/// Per-platform pinned Chromium versions.
pub fn platform_chromium_version(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "linux-x64" => "146.0.7680.177.5",
        "linux-arm64" => "146.0.7680.177.3",
        "darwin-arm64" => "145.0.7632.109.2",
        "darwin-x64" => "145.0.7632.109.2",
        "windows-x64" => "146.0.7680.177.5",
        _ => return None,
    })
}

pub const AVAILABLE_PLATFORMS: &[&str] = &[
    "linux-x64",
    "linux-arm64",
    "darwin-arm64",
    "darwin-x64",
    "windows-x64",
];

static VERSION_PIN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9]+(?:\.[0-9]+){3,4}$").unwrap());

/// Return the platform tag for the current build target, e.g. `linux-x64`.
pub fn platform_tag() -> Result<&'static str> {
    let tag = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("windows", "x86_64") => "windows-x64",
        (os, arch) => {
            return Err(CloakError::UnsupportedPlatform {
                system: os.to_string(),
                arch: arch.to_string(),
            })
        }
    };
    Ok(tag)
}

/// `.zip` on Windows, `.tar.gz` elsewhere.
pub fn archive_ext() -> &'static str {
    if cfg!(windows) {
        ".zip"
    } else {
        ".tar.gz"
    }
}

/// `cloakbrowser-{tag}{ext}`.
pub fn archive_name(tag: &str) -> String {
    format!("cloakbrowser-{tag}{}", archive_ext())
}

/// The Chromium version for the current platform.
pub fn chromium_version() -> String {
    let tag = platform_tag().unwrap_or("");
    platform_chromium_version(tag)
        .unwrap_or(CHROMIUM_VERSION)
        .to_string()
}

/// Validate/normalize a requested version pin. `None`/empty → `Ok(None)`.
pub fn normalize_requested_version(arg: Option<&str>) -> Result<Option<String>> {
    let raw = arg
        .map(|s| s.to_string())
        .or_else(|| std::env::var("CLOAKBROWSER_VERSION").ok());
    let raw = match raw {
        Some(s) => s.trim().to_string(),
        None => return Ok(None),
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if !VERSION_PIN_RE.is_match(&raw) {
        return Err(CloakError::InvalidVersion(raw));
    }
    Ok(Some(raw))
}

/// `~/.cloakbrowser` (override: `CLOAKBROWSER_CACHE_DIR`).
pub fn cache_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CLOAKBROWSER_CACHE_DIR") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cloakbrowser")
}

/// `cache_dir/chromium-{version}` — note: NO `v` prefix.
pub fn binary_dir(version: Option<&str>) -> PathBuf {
    let v = version.map(|s| s.to_string()).unwrap_or_else(chromium_version);
    cache_dir().join(format!("chromium-{v}"))
}

/// Path to the chrome executable inside the binary dir.
pub fn binary_path(version: Option<&str>) -> PathBuf {
    let dir = binary_dir(version);
    if cfg!(target_os = "macos") {
        dir.join("Chromium.app")
            .join("Contents")
            .join("MacOS")
            .join("Chromium")
    } else if cfg!(windows) {
        dir.join("chrome.exe")
    } else {
        dir.join("chrome")
    }
}

/// GeoIP database directory: `cache_dir/geoip`.
pub fn geoip_dir() -> PathBuf {
    cache_dir().join("geoip")
}

/// Version-tuple comparison: `a > b` component-wise (missing → error → false).
pub fn version_newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> Option<Vec<u64>> {
        v.split('.').map(|p| p.parse::<u64>().ok()).collect()
    }
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Whether the current platform has a downloadable binary.
pub fn check_platform_available() -> Result<()> {
    let tag = platform_tag()?;
    if AVAILABLE_PLATFORMS.contains(&tag) {
        Ok(())
    } else {
        Err(CloakError::UnsupportedPlatform {
            system: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        })
    }
}

/// Resolved download base URL (env override or default).
pub fn download_base_url() -> String {
    std::env::var("CLOAKBROWSER_DOWNLOAD_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_DOWNLOAD_BASE_URL.to_string())
}

/// Whether a custom (self-hosted) download URL is configured.
pub fn has_custom_download_url() -> bool {
    std::env::var("CLOAKBROWSER_DOWNLOAD_URL")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}
