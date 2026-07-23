//! Stealth argument construction. See docs/SPEC-core.md.
//!
//! STUB — to be implemented.

/// `--enable-automation` and `--enable-unsafe-swiftshader` must be removed from
/// the final launch (mirrors Python `IGNORE_DEFAULT_ARGS`).
pub const IGNORE_DEFAULT_ARGS: &[&str] = &["--enable-automation", "--enable-unsafe-swiftshader"];

/// Default headless viewport (headed launches track the real window).
pub const DEFAULT_VIEWPORT: (u32, u32) = (1920, 947);

/// Build the default stealth args with a fresh random fingerprint seed.
pub fn get_default_stealth_args() -> Vec<String> {
    unimplemented!("args::get_default_stealth_args — see docs/SPEC-core.md")
}

/// Combine stealth args with user args + locale/timezone/extension flags.
#[allow(clippy::too_many_arguments)]
pub fn build_args(
    _stealth_args: bool,
    _extra_args: Option<Vec<String>>,
    _timezone: Option<&str>,
    _locale: Option<&str>,
    _headless: bool,
    _extension_paths: Option<Vec<String>>,
    _start_maximized: bool,
) -> Vec<String> {
    unimplemented!("args::build_args — see docs/SPEC-core.md")
}
