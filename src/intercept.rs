//! Anti-redirect / URL blocking via CDP request interception.
//!
//! CloakBrowser upstream has no such feature (it only does stealth); this is an
//! addition. It uses the CDP `Fetch` domain: every request the browser is about
//! to make (including navigations triggered by HTTP 30x, `<meta refresh>`, or
//! `location = ...`) is paused and matched against a glob blocklist. Matches are
//! failed with `BlockedByClient`, so the request is cancelled and the target is
//! never loaded; everything else is continued untouched.
//!
//! Note on behaviour: blocking a top-level *navigation* means the destination is
//! never fetched — the tab lands on Chrome's error page (`chrome-error://…`)
//! rather than the blocked site. The point is that the target site is never
//! reached. Blocked sub-resources (images, scripts, XHR) are simply dropped with
//! the page otherwise intact.
//!
//! Interception requires the browser to be launched with request interception
//! enabled — pass a non-empty [`crate::LaunchOptions::block_urls`], or build the
//! [`chromiumoxide::BrowserConfig`] with `.enable_request_intercept()` yourself.
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use cloak_cdp::{launch, LaunchOptions};
//! use cloak_cdp::intercept::block_navigations;
//!
//! let (mut browser, mut handler) = launch(LaunchOptions {
//!     block_urls: vec!["*://ads.example.com/*".into(), "*doubleclick*".into()],
//!     ..Default::default()
//! }).await?;
//! tokio::spawn(async move { while handler.next().await.is_some() {} });
//!
//! let page = browser.new_page("about:blank").await?;
//! // Start blocking; returns a guard task handle.
//! let _guard = block_navigations(&page, ["*://ads.example.com/*", "*doubleclick*"]).await?;
//! page.goto("https://example.com").await?;
//! # Ok(()) }
//! ```

use crate::error::{CloakError, Result};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, ResourceType as CdpResourceType};
use chromiumoxide::Page;
use futures::StreamExt;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// A network resource category, used to block whole classes of requests to save
/// proxy bandwidth (e.g. images, stylesheets, fonts, media).
///
/// Mirrors CDP's `Network.ResourceType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Document,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    TextTrack,
    Xhr,
    Fetch,
    Prefetch,
    EventSource,
    WebSocket,
    Manifest,
    Ping,
    Other,
}

impl ResourceType {
    fn from_cdp(rt: &CdpResourceType) -> Option<ResourceType> {
        Some(match rt {
            CdpResourceType::Document => ResourceType::Document,
            CdpResourceType::Stylesheet => ResourceType::Stylesheet,
            CdpResourceType::Image => ResourceType::Image,
            CdpResourceType::Media => ResourceType::Media,
            CdpResourceType::Font => ResourceType::Font,
            CdpResourceType::Script => ResourceType::Script,
            CdpResourceType::TextTrack => ResourceType::TextTrack,
            CdpResourceType::Xhr => ResourceType::Xhr,
            CdpResourceType::Fetch => ResourceType::Fetch,
            CdpResourceType::Prefetch => ResourceType::Prefetch,
            CdpResourceType::EventSource => ResourceType::EventSource,
            CdpResourceType::WebSocket => ResourceType::WebSocket,
            CdpResourceType::Manifest => ResourceType::Manifest,
            CdpResourceType::Ping => ResourceType::Ping,
            CdpResourceType::Other => ResourceType::Other,
            _ => return None,
        })
    }

    /// Parse a case-insensitive name (e.g. "image", "stylesheet", "css", "font").
    /// Returns `None` for an unknown name.
    pub fn parse(s: &str) -> Option<ResourceType> {
        Some(match s.trim().to_lowercase().as_str() {
            "document" => ResourceType::Document,
            "stylesheet" | "css" => ResourceType::Stylesheet,
            "image" | "img" => ResourceType::Image,
            "media" => ResourceType::Media,
            "font" => ResourceType::Font,
            "script" | "js" => ResourceType::Script,
            "texttrack" => ResourceType::TextTrack,
            "xhr" => ResourceType::Xhr,
            "fetch" => ResourceType::Fetch,
            "prefetch" => ResourceType::Prefetch,
            "eventsource" => ResourceType::EventSource,
            "websocket" | "ws" => ResourceType::WebSocket,
            "manifest" => ResourceType::Manifest,
            "ping" => ResourceType::Ping,
            "other" => ResourceType::Other,
            _ => return None,
        })
    }
}

/// A compiled URL glob pattern.
///
/// Supported syntax (case-insensitive):
/// - `*` matches any run of characters (including `/`)
/// - everything else is a literal
///
/// Examples: `*://ads.example.com/*`, `*doubleclick*`, `https://evil.site/*`.
#[derive(Debug, Clone)]
pub struct UrlPattern {
    /// Literal segments that must appear in order; `None` gaps are `*`.
    segments: Vec<String>,
    anchored_start: bool,
    anchored_end: bool,
}

impl UrlPattern {
    /// Compile a glob pattern.
    pub fn new(pattern: &str) -> Self {
        let lower = pattern.to_lowercase();
        let anchored_start = !lower.starts_with('*');
        let anchored_end = !lower.ends_with('*');
        let segments: Vec<String> = lower
            .split('*')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        UrlPattern {
            segments,
            anchored_start,
            anchored_end,
        }
    }

    /// Whether `url` matches this pattern.
    pub fn matches(&self, url: &str) -> bool {
        let hay = url.to_lowercase();
        if self.segments.is_empty() {
            // Pattern was "*" (or empty) → matches everything.
            return true;
        }

        let mut pos = 0usize;
        for (i, seg) in self.segments.iter().enumerate() {
            let found = match hay[pos..].find(seg.as_str()) {
                Some(idx) => pos + idx,
                None => return false,
            };
            if i == 0 && self.anchored_start && found != 0 {
                return false;
            }
            pos = found + seg.len();
        }
        if self.anchored_end && pos != hay.len() {
            return false;
        }
        true
    }
}

/// A set of URL patterns; a URL is blocked if it matches any of them.
#[derive(Debug, Clone, Default)]
pub struct BlockList {
    patterns: Vec<UrlPattern>,
}

impl BlockList {
    /// Build from an iterator of glob strings.
    pub fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        BlockList {
            patterns: patterns
                .into_iter()
                .map(|p| UrlPattern::new(p.as_ref()))
                .collect(),
        }
    }

    /// Whether `url` should be blocked.
    pub fn is_blocked(&self, url: &str) -> bool {
        self.patterns.iter().any(|p| p.matches(url))
    }

    /// Whether the list has no patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Guard returned by [`block_navigations`]. Dropping it stops the interceptor
/// task (the browser keeps running; new requests are simply no longer paused
/// by this guard).
#[must_use = "dropping the guard stops URL blocking"]
pub struct BlockGuard {
    handle: JoinHandle<()>,
}

impl BlockGuard {
    /// Stop blocking immediately.
    pub fn stop(self) {
        self.handle.abort();
    }
}

impl Drop for BlockGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// What to block: URL glob patterns and/or whole resource categories.
///
/// Blocking resource categories (images, stylesheets, fonts, media, …) is the
/// usual way to cut proxy bandwidth for scraping — those resources are often the
/// bulk of a page's transfer.
///
/// ```no_run
/// use cloak_cdp::intercept::{BlockConfig, ResourceType};
/// let cfg = BlockConfig::new()
///     .resources([ResourceType::Image, ResourceType::Stylesheet, ResourceType::Font, ResourceType::Media])
///     .urls(["*doubleclick*"]);
/// ```
#[derive(Debug, Clone, Default)]
pub struct BlockConfig {
    urls: BlockList,
    resources: std::collections::HashSet<ResourceType>,
}

impl BlockConfig {
    /// An empty config (blocks nothing).
    pub fn new() -> Self {
        BlockConfig::default()
    }

    /// Block requests whose URL matches any of these globs (anti-redirect).
    pub fn urls<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.urls = BlockList::new(patterns);
        self
    }

    /// Block whole resource categories (bandwidth saving).
    pub fn resources<I>(mut self, types: I) -> Self
    where
        I: IntoIterator<Item = ResourceType>,
    {
        self.resources = types.into_iter().collect();
        self
    }

    /// Parse resource categories from case-insensitive names (unknown names are
    /// ignored). Useful for wiring from config/CLI strings.
    pub fn resources_from_names<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.resources = names
            .into_iter()
            .filter_map(|n| ResourceType::parse(n.as_ref()))
            .collect();
        self
    }

    /// Whether nothing is configured to block.
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty() && self.resources.is_empty()
    }
}

/// Start intercepting requests on `page` and blocking those matching `config`
/// (by URL glob and/or resource category).
///
/// The page's browser MUST have been launched with request interception enabled
/// (non-empty [`crate::LaunchOptions::block_urls`] /
/// [`crate::LaunchOptions::block_resources`], or
/// `BrowserConfig::builder().enable_request_intercept()`), otherwise no
/// `Fetch.requestPaused` events arrive and this is a no-op.
///
/// Returns a [`BlockGuard`]; keep it alive for as long as you want blocking
/// active. Matching requests are failed with `BlockedByClient`; everything else
/// is continued untouched.
pub async fn block_requests(page: &Page, config: BlockConfig) -> Result<BlockGuard> {
    let mut paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|e| CloakError::other(format!("failed to listen for paused requests: {e}")))?;
    let page = Arc::new(page.clone());

    let handle = tokio::spawn(async move {
        while let Some(event) = paused.next().await {
            let request_id = event.request_id.clone();

            let url_blocked = config.urls.is_blocked(&event.request.url);
            let type_blocked = !config.resources.is_empty()
                && ResourceType::from_cdp(&event.resource_type)
                    .map(|rt| config.resources.contains(&rt))
                    .unwrap_or(false);

            if url_blocked || type_blocked {
                tracing::debug!(
                    url = %event.request.url,
                    resource_type = ?event.resource_type,
                    "blocking request"
                );
                let _ = page
                    .execute(FailRequestParams::new(
                        request_id,
                        ErrorReason::BlockedByClient,
                    ))
                    .await;
            } else {
                let _ = page
                    .execute(ContinueRequestParams::new(request_id))
                    .await;
            }
        }
    });

    Ok(BlockGuard { handle })
}

/// Convenience: block only URL glob patterns (anti-redirect). Equivalent to
/// `block_requests(page, BlockConfig::new().urls(patterns))`.
pub async fn block_navigations<I, S>(page: &Page, patterns: I) -> Result<BlockGuard>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    block_requests(page, BlockConfig::new().urls(patterns)).await
}

/// Convenience: block only whole resource categories (bandwidth saving).
/// Equivalent to `block_requests(page, BlockConfig::new().resources(types))`.
pub async fn block_resources<I>(page: &Page, types: I) -> Result<BlockGuard>
where
    I: IntoIterator<Item = ResourceType>,
{
    block_requests(page, BlockConfig::new().resources(types)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_prefix_suffix_and_contains() {
        assert!(UrlPattern::new("*doubleclick*").matches("https://a.doubleclick.net/x"));
        assert!(UrlPattern::new("https://evil.site/*").matches("https://evil.site/path"));
        assert!(!UrlPattern::new("https://evil.site/*").matches("https://ok.site/path"));
        assert!(UrlPattern::new("*://ads.example.com/*").matches("http://ads.example.com/a"));
        assert!(UrlPattern::new("*://ads.example.com/*").matches("https://ads.example.com/b"));
        assert!(!UrlPattern::new("*://ads.example.com/*").matches("https://cdn.example.com/b"));
    }

    #[test]
    fn anchored_exact() {
        let p = UrlPattern::new("https://exact.com/page");
        assert!(p.matches("https://exact.com/page"));
        assert!(!p.matches("https://exact.com/page2"));
        assert!(!p.matches("x-https://exact.com/page"));
    }

    #[test]
    fn case_insensitive() {
        assert!(UrlPattern::new("*ADS*").matches("http://x/ads/y"));
    }

    #[test]
    fn star_matches_all() {
        assert!(UrlPattern::new("*").matches("anything"));
    }

    #[test]
    fn blocklist_any() {
        let bl = BlockList::new(["*ads*", "*tracker*"]);
        assert!(bl.is_blocked("http://x/ads"));
        assert!(bl.is_blocked("http://tracker.io/"));
        assert!(!bl.is_blocked("http://safe.com/"));
    }

    #[test]
    fn resource_type_parse_aliases() {
        assert_eq!(ResourceType::parse("css"), Some(ResourceType::Stylesheet));
        assert_eq!(ResourceType::parse("CSS"), Some(ResourceType::Stylesheet));
        assert_eq!(ResourceType::parse("image"), Some(ResourceType::Image));
        assert_eq!(ResourceType::parse("img"), Some(ResourceType::Image));
        assert_eq!(ResourceType::parse("font"), Some(ResourceType::Font));
        assert_eq!(ResourceType::parse("js"), Some(ResourceType::Script));
        assert_eq!(ResourceType::parse("nonsense"), None);
    }

    #[test]
    fn block_config_from_names_ignores_unknown() {
        let cfg = BlockConfig::new().resources_from_names(["image", "css", "bogus"]);
        assert!(cfg.resources.contains(&ResourceType::Image));
        assert!(cfg.resources.contains(&ResourceType::Stylesheet));
        assert_eq!(cfg.resources.len(), 2);
        assert!(!cfg.is_empty());
    }
}
