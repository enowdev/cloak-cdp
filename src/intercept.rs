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
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::Page;
use futures::StreamExt;
use std::sync::Arc;
use tokio::task::JoinHandle;

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

/// Start blocking navigations/requests to any URL matching `patterns` on `page`.
///
/// The page's browser MUST have been launched with request interception enabled
/// (non-empty [`crate::LaunchOptions::block_urls`], or
/// `BrowserConfig::builder().enable_request_intercept()`), otherwise no
/// `Fetch.requestPaused` events arrive and this is a no-op.
///
/// Returns a [`BlockGuard`]; keep it alive for as long as you want blocking
/// active. Matching requests are failed with `BlockedByClient` (the navigation
/// is cancelled — the current page stays), everything else is continued.
pub async fn block_navigations<I, S>(page: &Page, patterns: I) -> Result<BlockGuard>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let blocklist = BlockList::new(patterns);
    let mut paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|e| CloakError::other(format!("failed to listen for paused requests: {e}")))?;
    let page = Arc::new(page.clone());

    let handle = tokio::spawn(async move {
        while let Some(event) = paused.next().await {
            let request_id = event.request_id.clone();
            if blocklist.is_blocked(&event.request.url) {
                tracing::debug!(url = %event.request.url, "blocking navigation/request");
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
}
