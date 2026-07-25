# cloak-cdp

**Unofficial Rust port of [CloakBrowser](https://github.com/CloakHQ/cloakbrowser) by [CloakHQ](https://github.com/CloakHQ).**

Stealth Chromium automation from Rust over the Chrome DevTools Protocol
(via [`chromiumoxide`](https://crates.io/crates/chromiumoxide)). It drives
CloakHQ's custom, source-patched Chromium binary — the "cloak" lives in that
binary's 71 compile-level fingerprint patches; this crate is the Rust
orchestration (download, verify, stealth args, spawn + connect) that activates
them, replacing the upstream Python/JS wrappers.

> ### ⚠️ Attribution & status
>
> This is an **independent, community port** and is **not affiliated with,
> endorsed by, or maintained by CloakHQ**. All credit for the stealth technology,
> the patched Chromium binary, and the original design belongs to CloakHQ.
>
> - Upstream project: **https://github.com/CloakHQ/cloakbrowser**
> - Learn more / Pro tier: **https://cloakbrowser.dev**
>
> This crate downloads CloakHQ's Chromium binary at runtime. **That binary is
> covered by CloakHQ's own separate license** (see their
> [`BINARY-LICENSE.md`](https://github.com/CloakHQ/cloakbrowser/blob/main/BINARY-LICENSE.md)),
> **not** by this crate's MIT license. By using this crate you agree to comply
> with CloakHQ's binary license and terms. The MIT license here covers only the
> Rust port code.

## Features

- `launch()` → a [`chromiumoxide`] `Browser` connected to stealth Chromium
- Source-level fingerprint patches active (`navigator.webdriver === false`, GPU/canvas/WebGL/audio spoofing)
- Automatic binary download + **Ed25519 signature verification** + extraction
- GeoIP-driven timezone/locale + WebRTC exit-IP spoofing (MaxMind GeoLite2)
- SOCKS5 / HTTP(S) proxy support
- Humanize layer: bézier mouse movement, human typing cadence, scroll — over CDP
- **Anti-redirect / URL blocking** — cancel navigations to matching URL globs so a page can't auto-redirect you away (an addition; upstream has no such feature)
- **Resource blocking** — block images/CSS/fonts/media/… by category to cut proxy bandwidth (often 90%+ on heavy pages)
- `serve` feature: a CDP multiplexer (one Chrome per fingerprint seed on one port)
- Widevine CDM hint-seeding + fetcher (Linux x86-64)

## Install

```toml
[dependencies]
cloak-cdp = "0.1"
tokio = { version = "1", features = ["full"] }
futures = "0.3"
```

## Usage

```rust
use cloak_cdp::{launch, LaunchOptions};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Custom stealth Chromium is auto-downloaded + verified on first launch.
    let (mut browser, mut handler) = launch(LaunchOptions::default()).await?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("about:blank").await?;
    page.goto("https://abrahamjuliot.github.io/creepjs/").await?;

    let webdriver: serde_json::Value =
        page.evaluate("navigator.webdriver").await?.into_value()?;
    assert_eq!(webdriver, serde_json::Value::Bool(false));

    browser.close().await?;
    Ok(())
}
```

For heavy anti-bot pages (Cloudflare), open `about:blank` then `page.goto(url)`,
and drain the handler without breaking on a single errored event.

### Anti-redirect (URL blocking)

Stop a page from auto-redirecting you to another site:

```rust
use cloak_cdp::{launch, LaunchOptions};
use cloak_cdp::intercept::block_navigations;

let (mut browser, mut handler) = launch(LaunchOptions {
    // non-empty => request interception is enabled at launch
    block_urls: vec!["*://ads.example.com/*".into(), "*doubleclick*".into()],
    ..Default::default()
}).await?;
tokio::spawn(async move { while handler.next().await.is_some() {} });

let page = browser.new_page("about:blank").await?;
let _guard = block_navigations(&page, ["*://ads.example.com/*", "*doubleclick*"]).await?;
page.goto("https://some-site.com").await?; // redirects to a blocked URL are cancelled
```

Glob syntax: `*` matches any run of characters; matching is case-insensitive.
A blocked top-level navigation lands on Chrome's error page (the target is never
reached); blocked sub-resources are just dropped.

### Resource blocking (save proxy bandwidth)

Block whole resource categories so images/CSS/fonts/media never download —
typically the bulk of a page's transfer:

```rust
use cloak_cdp::{launch, LaunchOptions};
use cloak_cdp::intercept::{block_requests, BlockConfig, ResourceType};

let (mut browser, mut handler) = launch(LaunchOptions {
    block_resources: vec!["image".into(), "css".into(), "font".into(), "media".into()],
    ..Default::default()
}).await?;
tokio::spawn(async move { while handler.next().await.is_some() {} });

let page = browser.new_page("about:blank").await?;
let _guard = block_requests(&page, BlockConfig::new()
    .resources([ResourceType::Image, ResourceType::Stylesheet, ResourceType::Font, ResourceType::Media])
    .urls(["*doubleclick*"])   // combine with URL blocking if you like
).await?;
page.goto("https://example.com").await?;
```

Categories (case-insensitive names): `stylesheet`/`css`, `image`/`img`, `font`,
`media`, `script`/`js`, `xhr`, `fetch`, `websocket`, `manifest`, `ping`, `document`, …

### CDP multiplexer (`serve` feature)

```bash
cargo run --example serve --features serve
# then: curl 'http://127.0.0.1:9222/json/version?fingerprint=12345'
```

## Examples

| Example | What it shows |
|---|---|
| `basic` | Launch + read stealth signals |
| `turnstile` | Headed run passing Cloudflare Turnstile |
| `human` | Bézier mouse move + human typing over CDP |
| `block_redirect` | Anti-redirect: block auto-redirect to a URL |
| `block_resources` | Block images/CSS/fonts to save bandwidth |
| `verify_download` | Download + Ed25519 verify + extract |
| `verify_geoip` | GeoIP timezone/locale/exit-IP resolution |
| `serve` / `serve_client` | CDP multiplexer server + client |
| `verify_widevine` | Widevine hint-seeding / fetch |

Run download/geoip examples with `CLOAKBROWSER_CACHE_DIR=/tmp/...` to avoid
touching your real cache.

## Environment variables

Compatible with the upstream project's variables, including:
`CLOAKBROWSER_CACHE_DIR`, `CLOAKBROWSER_BINARY_PATH`, `CLOAKBROWSER_DOWNLOAD_URL`,
`CLOAKBROWSER_VERSION`, `CLOAKBROWSER_GEOIP_TIMEOUT_SECONDS`, `CLOAKBROWSER_WIDEVINE`.

## License

MIT — see [`LICENSE`](LICENSE). Covers the Rust port code only. The downloaded
Chromium binary is licensed separately by CloakHQ.
