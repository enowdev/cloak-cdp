//! # cloakbrowser
//!
//! Rust port of [CloakBrowser](https://github.com/CloakHQ/cloakbrowser): a stealth
//! Chromium launcher that drives a custom-patched Chromium binary over CDP via
//! [`chromiumoxide`]. The "cloak" lives in the downloaded binary's source-level
//! fingerprint patches; this crate replicates the orchestration (download, verify,
//! stealth args, spawn + connect) that activates them.
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use cloakbrowser::{launch, LaunchOptions};
//!
//! let (browser, mut handler) = launch(LaunchOptions::default()).await?;
//! tokio::spawn(async move { while handler.next().await.is_some() {} });
//! let page = browser.new_page("https://abrahamjuliot.github.io/creepjs/").await?;
//! page.goto("https://example.com").await?;
//! # Ok(()) }
//! ```

pub mod error;
pub mod platform;

pub mod args;
pub mod download;
pub mod geoip;
pub mod intercept;
pub mod proxy;
pub mod launcher;

pub mod human;
pub mod widevine;

#[cfg(feature = "serve")]
pub mod serve;

pub use error::{CloakError, Result};

/// Playwright-compatible proxy configuration.
///
/// Accepts either a bare URL string or structured settings, mirroring the Python
/// wrapper's `str | ProxySettings | None`.
#[derive(Debug, Clone)]
pub enum Proxy {
    /// A bare proxy URL, e.g. `http://user:pass@host:8080` or `socks5://host:1080`.
    Url(String),
    /// Structured proxy settings.
    Settings(ProxySettings),
}

/// Structured proxy settings (mirrors Playwright `ProxySettings`).
#[derive(Debug, Clone, Default)]
pub struct ProxySettings {
    /// Required proxy server URL (e.g. `http://host:8080`).
    pub server: String,
    /// Comma-separated bypass list.
    pub bypass: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Proxy {
    /// Return the underlying server URL string (for either variant).
    pub fn server_url(&self) -> &str {
        match self {
            Proxy::Url(u) => u,
            Proxy::Settings(s) => &s.server,
        }
    }
}

/// Options for [`launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Run headless (default `true`).
    pub headless: bool,
    pub proxy: Option<Proxy>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    /// Resolve timezone/locale/exit-IP from the proxy (or machine) via GeoIP.
    pub geoip: bool,
    /// Extra Chrome flags appended after stealth defaults.
    pub extra_args: Vec<String>,
    /// Whether to inject the default stealth args (default `true`).
    pub stealth_args: bool,
    pub extension_paths: Vec<String>,
    /// Pin a specific binary version (`None` → platform default).
    pub browser_version: Option<String>,
    /// Open the window maximized where the binary supports it coherently.
    pub start_maximized: bool,
    /// URL glob patterns to block (anti-redirect). When non-empty, the browser
    /// is launched with request interception enabled; attach the blocker to a
    /// page with [`intercept::block_navigations`]. Requests/navigations matching
    /// any pattern are cancelled, so an auto-redirect to a matching URL is
    /// prevented and the current page stays. See [`intercept`].
    pub block_urls: Vec<String>,
    /// Resource categories to block (bandwidth saving for proxies), by
    /// case-insensitive name: `"stylesheet"`/`"css"`, `"image"`, `"font"`,
    /// `"media"`, `"script"`, etc. When non-empty, request interception is
    /// enabled at launch; attach with [`intercept::block_requests`]. Blocking
    /// images/CSS/fonts/media typically cuts most of a page's transfer.
    pub block_resources: Vec<String>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        LaunchOptions {
            headless: true,
            proxy: None,
            timezone: None,
            locale: None,
            geoip: false,
            extra_args: Vec::new(),
            stealth_args: true,
            extension_paths: Vec::new(),
            browser_version: None,
            start_maximized: false,
            block_urls: Vec::new(),
            block_resources: Vec::new(),
        }
    }
}

pub use launcher::launch;
