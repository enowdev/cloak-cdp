//! `launch()` — ensure binary, build args, spawn + connect CDP. See docs/SPEC-core.md.
//!
//! ## Removing `--enable-automation` (mirror of Python `ignore_default_args`)
//!
//! In the Python port, Playwright is launched with
//! `ignore_default_args = IGNORE_DEFAULT_ARGS = ["--enable-automation",
//! "--enable-unsafe-swiftshader"]`, which subtracts *only those two* flags from
//! Playwright's default set while keeping the rest.
//!
//! chromiumoxide 0.7 injects its own `DEFAULT_ARGS` (a 25-element list ported
//! from puppeteer) that includes `--enable-automation`. Crucially the crate only
//! exposes an all-or-nothing toggle — `BrowserConfigBuilder::disable_default_args`
//! — there is no per-flag ignore list (see `chromiumoxide-0.7.0/src/browser.rs`,
//! `BrowserConfig::launch`: `if self.disable_default_args { cmd.args(&self.args) }
//! else { cmd.args(DEFAULT_ARGS).args(&self.args) }`).
//!
//! So to mirror `ignore_default_args` faithfully we:
//!   1. call `.disable_default_args()` so chromiumoxide contributes *no* defaults
//!      (and therefore never adds `--enable-automation`), then
//!   2. re-inject the crate's useful defaults ourselves via
//!      [`CHROMIUMOXIDE_DEFAULT_ARGS_KEPT`], which is chromiumoxide's
//!      `DEFAULT_ARGS` with the [`args::IGNORE_DEFAULT_ARGS`] flags filtered out.
//!
//! Note `--enable-unsafe-swiftshader` is not present in chromiumoxide's
//! `DEFAULT_ARGS` at all, so filtering it is a no-op today but keeps us robust to
//! upstream additions.
//!
//! Result: `navigator.webdriver === false`, no automation banner.

use crate::args;
use crate::download;
use crate::error::{CloakError, Result};
use crate::proxy;
use crate::LaunchOptions;
use chromiumoxide::browser::HeadlessMode;
use chromiumoxide::{Browser, BrowserConfig, Handler};
use tracing::{debug, info};

/// chromiumoxide 0.7's built-in `DEFAULT_ARGS`, with [`args::IGNORE_DEFAULT_ARGS`]
/// removed. We inject these manually after `.disable_default_args()` so the useful
/// defaults survive while `--enable-automation` (and `--enable-unsafe-swiftshader`,
/// were it ever added upstream) are dropped — the exact effect of Playwright's
/// `ignore_default_args`.
///
/// Kept in sync with `chromiumoxide-0.7.0/src/browser.rs::DEFAULT_ARGS`.
pub const CHROMIUMOXIDE_DEFAULT_ARGS_KEPT: &[&str] = &[
    "--disable-background-networking",
    "--enable-features=NetworkService,NetworkServiceInProcess",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-breakpad",
    "--disable-client-side-phishing-detection",
    "--disable-component-extensions-with-background-pages",
    "--disable-default-apps",
    "--disable-dev-shm-usage",
    "--disable-extensions",
    "--disable-features=TranslateUI",
    "--disable-hang-monitor",
    "--disable-ipc-flooding-protection",
    "--disable-popup-blocking",
    "--disable-prompt-on-repost",
    "--disable-renderer-backgrounding",
    "--disable-sync",
    "--force-color-profile=srgb",
    "--metrics-recording-only",
    "--no-first-run",
    // "--enable-automation"  <- dropped (IGNORE_DEFAULT_ARGS)
    "--password-store=basic",
    "--use-mock-keychain",
    "--enable-blink-features=IdleDetection",
    "--lang=en_US",
];

/// Launch stealth Chromium and connect over CDP.
///
/// Returns the `Browser` and its `Handler` (the caller must drive the handler,
/// typically via `tokio::spawn`).
///
/// Flow (mirrors `browser.py:launch`):
///   1. [`download::ensure_binary`] → executable path.
///   2. [`proxy::maybe_resolve_geoip`] → timezone / locale / exit-IP.
///   3. [`proxy::resolve_proxy_config`] → launcher proxy settings + `--proxy-*` args.
///   4. [`proxy::resolve_webrtc_args`] → substitute `--fingerprint-webrtc-ip=auto`.
///   5. [`proxy::append_webrtc_exit_ip`] → append resolved exit IP.
///   6. [`args::build_args`] → final Chromium arg vector.
///   7. Build [`BrowserConfig`] and spawn + connect CDP.
pub async fn launch(opts: LaunchOptions) -> Result<(Browser, Handler)> {
    let LaunchOptions {
        headless,
        proxy,
        timezone,
        locale,
        geoip,
        extra_args,
        stealth_args,
        extension_paths,
        browser_version,
        start_maximized,
    } = opts;

    // 1. Ensure the stealth binary is present.
    let binary_path = download::ensure_binary(browser_version.as_deref()).await?;
    debug!(path = %binary_path.display(), "resolved stealth chromium binary");

    // 2. Resolve timezone / locale / exit-IP from geoip + proxy.
    let (timezone, locale, exit_ip) =
        proxy::maybe_resolve_geoip(geoip, proxy.as_ref(), timezone, locale, Some(&extra_args)).await;

    // 3. Split proxy into launcher-level settings vs chrome flags.
    let resolved_proxy = proxy::resolve_proxy_config(proxy.as_ref());

    // 4. WebRTC: replace `--fingerprint-webrtc-ip=auto` using the proxy egress.
    let args_after_webrtc = proxy::resolve_webrtc_args(Some(extra_args), proxy.as_ref()).await;

    // 5. Append the resolved exit IP for WebRTC spoofing (if any).
    let args_with_exit_ip = proxy::append_webrtc_exit_ip(args_after_webrtc, exit_ip.as_deref());

    // Merge user/webrtc args with proxy-derived `--proxy-*` args (user < proxy),
    // matching Python's `(args or []) + proxy_extra_args`.
    let mut combined_args = args_with_exit_ip.unwrap_or_default();
    combined_args.extend(resolved_proxy.extra_args.iter().cloned());

    // 6. Build the final stealth arg set.
    let extension_paths = if extension_paths.is_empty() {
        None
    } else {
        Some(extension_paths)
    };
    let chrome_args = args::build_args(
        stealth_args,
        Some(combined_args),
        timezone.as_deref(),
        locale.as_deref(),
        headless,
        extension_paths,
        start_maximized,
    );

    if let Some(settings) = resolved_proxy.proxy_settings.as_ref() {
        debug!(server = %settings.server, "launching with proxy");
    }
    debug!(?chrome_args, headless, "final chromium launch args");

    // 7. Assemble the BrowserConfig.
    //
    // `.disable_default_args()` + re-injecting `CHROMIUMOXIDE_DEFAULT_ARGS_KEPT`
    // is how we strip `--enable-automation` (mirror of Python's
    // `ignore_default_args`). See the module docs for the rationale.
    let headless_mode = if headless {
        HeadlessMode::New
    } else {
        HeadlessMode::False
    };

    let builder = BrowserConfig::builder()
        .chrome_executable(&binary_path)
        .headless_mode(headless_mode)
        .disable_default_args()
        // Re-inject chromiumoxide's useful defaults, minus IGNORE_DEFAULT_ARGS.
        .args(CHROMIUMOXIDE_DEFAULT_ARGS_KEPT.iter().map(|s| s.to_string()))
        // Our stealth / proxy / webrtc / locale args.
        .args(chrome_args);

    let config = builder
        .build()
        .map_err(|e| CloakError::Launch(format!("invalid browser config: {e}")))?;

    // 8. Spawn the process and connect over CDP.
    let (browser, handler) = Browser::launch(config)
        .await
        .map_err(|e| CloakError::Launch(e.to_string()))?;

    info!("stealth chromium launched and connected over CDP");
    Ok((browser, handler))
}
