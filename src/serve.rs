//! cloakserve — CDP multiplexer (one Chrome per fingerprint seed, one public port).
//! See docs/SPEC-cloakserve-widevine.md (PART 1). Feature-gated behind `serve`.
//!
//! Single HTTP+WS server fronting many Chrome processes. Each fingerprint seed maps
//! to its own Chrome process (own `--user-data-dir`, own `--remote-debugging-port`);
//! clients talk to one public port and the server proxies CDP HTTP + WebSockets to
//! the correct backend by seed.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{ws::Message as AxumMessage, ws::WebSocket, RawQuery, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use tokio::process::Child;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::protocol::Message as TungMessage;

use crate::error::{CloakError, Result};
use crate::Proxy;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Args for running Chrome directly (outside Playwright).
pub const BASE_CHROME_ARGS: &[&str] = &[
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-dev-shm-usage",
    "--disable-extensions",
    "--disable-popup-blocking",
    "--disable-background-networking",
    "--metrics-recording-only",
    "--ignore-gpu-blocklist",
];

/// First CDP port tried when spawning Chrome. Monotonically increasing thereafter.
pub const BASE_CDP_PORT: u16 = 5100;

/// Default seed key used for the no-seed (shared) Chrome process.
pub const DEFAULT_SEED_KEY: &str = "__default__";

/// Query params that need special handling (not generic `--fingerprint-{name}=` mapping).
const SPECIAL_PARAMS: &[&str] = &["fingerprint", "proxy", "geoip", "locale", "timezone"];

/// Allowed fingerprint seed shape.
pub static SAFE_SEED_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}$").unwrap());

/// Seeds reserved for internal use; a client may never request one of these.
pub static RESERVED_SEEDS: Lazy<std::collections::HashSet<&'static str>> =
    Lazy::new(|| ["__default__"].into_iter().collect());

/// WebSocket origins that are always trusted (DevTools front-ends).
pub static TRUSTED_WS_ORIGINS: Lazy<std::collections::HashSet<&'static str>> =
    Lazy::new(|| ["devtools://devtools", "chrome-devtools://devtools"].into_iter().collect());

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the multiplexer server.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub port: u16,
    pub headless: bool,
    pub data_dir: Option<String>,
    pub idle_timeout: f64,
    pub global_args: Vec<String>,
    /// CLI default fingerprint seed (applied when a connection omits `fingerprint`).
    pub default_seed: Option<String>,
    /// CLI default locale.
    pub default_locale: Option<String>,
    /// CLI default timezone.
    pub default_timezone: Option<String>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        ServeConfig {
            port: 9222,
            headless: true,
            data_dir: None,
            idle_timeout: 0.0,
            global_args: Vec::new(),
            default_seed: None,
            default_locale: None,
            default_timezone: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Security helpers (ported from cloakserve)
// ---------------------------------------------------------------------------

/// Return a normalized `(host, port)` pair for an Origin/Host netloc, or `None`.
pub fn host_port_from_netloc(netloc: &str, default_port: u16) -> Option<(String, u16)> {
    if netloc.contains(',') {
        return None;
    }
    let trimmed = netloc.trim();
    // A path/query/fragment in the raw netloc is illegal here.
    let authority = trimmed.rsplit('@').next().unwrap_or(trimmed);
    if authority.ends_with(':') {
        return None;
    }
    if trimmed_has_path(trimmed) {
        return None;
    }
    if trimmed.contains('?') || trimmed.contains('#') {
        return None;
    }
    // Parse via a synthetic URL to extract host/port and reject credentials.
    let parsed = url::Url::parse(&format!("http://{trimmed}")).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?;
    if host.is_empty() {
        return None;
    }
    let port = parsed.port().unwrap_or(default_port);
    Some((host.to_lowercase(), port))
}

/// True when the raw netloc contains a path segment (a `/` after the authority).
fn trimmed_has_path(netloc: &str) -> bool {
    let after_at = netloc.rsplit('@').next().unwrap_or(netloc);
    if let Some(close) = after_at.find(']') {
        after_at[close + 1..].contains('/')
    } else {
        after_at.contains('/')
    }
}

/// Return `true` for `localhost` and loopback IP literals.
pub fn is_loopback_host(hostname: &str) -> bool {
    let h = hostname.trim_matches(|c| c == '[' || c == ']');
    let h = h.trim_end_matches('.').to_lowercase();
    if h == "localhost" {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// Return `true` when a WebSocket Origin is safe to proxy to local CDP.
pub fn origin_is_allowed(origin: Option<&str>, host: Option<&str>, request_scheme: &str) -> bool {
    // Non-browser CDP clients (Playwright/Puppeteer) commonly omit Origin.
    let origin = match origin {
        None => return true,
        Some(o) => o.trim(),
    };
    if origin.is_empty() || origin.eq_ignore_ascii_case("null") {
        return false;
    }
    if TRUSTED_WS_ORIGINS.contains(origin) {
        return true;
    }

    // Scheme must be http/https; there must be no path/query/fragment.
    let (scheme, rest) = match origin.split_once("://") {
        Some((s, r)) => (s.to_lowercase(), r),
        None => return false,
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    if rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return false;
    }

    let origin_default_port = if scheme == "https" { 443 } else { 80 };
    let request_scheme = request_scheme
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let request_default_port = if request_scheme == "https" || request_scheme == "wss" {
        443
    } else {
        80
    };

    let origin_host = host_port_from_netloc(rest, origin_default_port);
    let request_host = host_port_from_netloc(host.unwrap_or(""), request_default_port);

    let (origin_host, request_host) = match (origin_host, request_host) {
        (Some(a), Some(b)) => (a, b),
        _ => return false,
    };
    if !is_loopback_host(&request_host.0) {
        return false;
    }
    origin_host == request_host
}

// ---------------------------------------------------------------------------
// Proxy normalization (local minimal port of _normalize_socks_string_url)
// ---------------------------------------------------------------------------

/// Re-encode credentials in a proxy URL so Chromium's parser doesn't truncate them
/// at special chars. Idempotent; passes malformed input through unchanged.
fn normalize_socks_string_url(raw: &str) -> String {
    use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
    let parsed = match url::Url::parse(raw) {
        Ok(p) => p,
        Err(_) => return raw.to_string(),
    };
    let user = parsed.username();
    let pass = parsed.password();
    if user.is_empty() && pass.is_none() {
        return raw.to_string();
    }
    let dec_user = percent_decode_str(user).decode_utf8_lossy();
    let enc_user = utf8_percent_encode(&dec_user, NON_ALPHANUMERIC).to_string();

    let host = parsed.host_str().unwrap_or("");
    let scheme = parsed.scheme();
    let port = parsed.port();

    let mut out = format!("{scheme}://");
    out.push_str(&enc_user);
    if let Some(p) = pass {
        let dec_pass = percent_decode_str(p).decode_utf8_lossy();
        let enc_pass = utf8_percent_encode(&dec_pass, NON_ALPHANUMERIC).to_string();
        out.push(':');
        out.push_str(&enc_pass);
    }
    out.push('@');
    out.push_str(host);
    if let Some(p) = port {
        out.push_str(&format!(":{p}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Connection param parsing
// ---------------------------------------------------------------------------

/// Parsed connection parameters from a CDP query string.
#[derive(Debug, Default, Clone)]
pub struct ConnectionParams {
    pub seed: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub proxy: Option<String>,
    pub geoip: bool,
    pub extra_args: Vec<String>,
}

/// Parse a raw query string into connection config (mirrors `parse_qs`, `values[0]`).
pub fn parse_connection_params(query_string: &str) -> ConnectionParams {
    let mut result = ConnectionParams::default();
    let mut seen: HashMap<String, ()> = HashMap::new();

    for (key, val) in url::form_urlencoded::parse(query_string.as_bytes()) {
        // parse_qs keep_blank_values=False → drop empty values.
        if val.is_empty() {
            continue;
        }
        let key = key.into_owned();
        let val = val.into_owned();
        // values[0] semantics: only the first occurrence per key wins.
        if seen.insert(key.clone(), ()).is_some() {
            continue;
        }
        match key.as_str() {
            "fingerprint" => result.seed = Some(val),
            "timezone" => result.timezone = Some(val),
            "locale" => result.locale = Some(val),
            "proxy" => result.proxy = Some(val),
            "geoip" => {
                let v = val.to_lowercase();
                result.geoip = v == "true" || v == "1" || v == "yes";
            }
            other if !SPECIAL_PARAMS.contains(&other) => {
                result.extra_args.push(format!("--fingerprint-{other}={val}"));
            }
            _ => {}
        }
    }
    result
}

// ---------------------------------------------------------------------------
// ChromeProcess + ChromePool
// ---------------------------------------------------------------------------

/// One running Chrome instance.
struct ChromeProcess {
    seed: String,
    child: Child,
    cdp_port: u16,
    user_data_dir: PathBuf,
    timezone: Option<String>,
    locale: Option<String>,
    proxy: Option<String>,
}

/// Per-seed shared state. The launch mutex serializes launch/cleanup; the process
/// mutex guards the live handle.
struct SeedSlot {
    launch_lock: Mutex<()>,
    process: Mutex<Option<ChromeProcess>>,
}

impl SeedSlot {
    fn new() -> Self {
        SeedSlot {
            launch_lock: Mutex::new(()),
            process: Mutex::new(None),
        }
    }
}

/// Manages multiple Chrome processes keyed by seed.
pub struct ChromePool {
    binary: PathBuf,
    global_args: Vec<String>,
    headless: bool,
    data_dir: PathBuf,
    default_seed: Option<String>,
    default_locale: Option<String>,
    default_timezone: Option<String>,
    idle_timeout: f64,
    slots: DashMap<String, Arc<SeedSlot>>,
    /// Connection refcount per seed_key.
    connections: DashMap<String, i64>,
    /// Monotonic port allocator (never decreases).
    next_port: AtomicU16,
    /// Marks seeds with a pending idle-cleanup task (status + cancellation).
    idle_pending: DashMap<String, ()>,
}

/// Routing info for a spawned/looked-up Chrome process.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub seed_key: String,
    pub cdp_port: u16,
}

impl ChromePool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binary: PathBuf,
        global_args: Vec<String>,
        headless: bool,
        data_dir: PathBuf,
        default_seed: Option<String>,
        default_locale: Option<String>,
        default_timezone: Option<String>,
        idle_timeout: f64,
    ) -> Self {
        ChromePool {
            binary,
            global_args,
            headless,
            data_dir,
            default_seed,
            default_locale,
            default_timezone,
            idle_timeout,
            slots: DashMap::new(),
            connections: DashMap::new(),
            next_port: AtomicU16::new(BASE_CDP_PORT),
            idle_pending: DashMap::new(),
        }
    }

    fn slot(&self, seed_key: &str) -> Arc<SeedSlot> {
        if let Some(s) = self.slots.get(seed_key) {
            return s.clone();
        }
        self.slots
            .entry(seed_key.to_string())
            .or_insert_with(|| Arc::new(SeedSlot::new()))
            .clone()
    }

    /// Refuse to delete anything not strictly inside `data_dir` (traversal guard).
    fn safe_rmtree(&self, path: &Path) {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let data_resolved = self
            .data_dir
            .canonicalize()
            .unwrap_or_else(|_| self.data_dir.clone());
        if resolved == data_resolved || !resolved.starts_with(&data_resolved) {
            tracing::error!("Refusing to delete path outside data_dir: {}", resolved.display());
            return;
        }
        let _ = std::fs::remove_dir_all(path);
    }

    /// Find a free port starting from `next_port` (monotonic, never decreases).
    fn allocate_port(&self) -> Result<u16> {
        for _ in 0..100 {
            let port = self.next_port.fetch_add(1, Ordering::SeqCst);
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return Ok(port);
            }
        }
        Err(CloakError::Serve("No free ports available for Chrome CDP".into()))
    }

    /// Increment the connection refcount for a seed (cancels a pending idle marker).
    pub fn connect(&self, seed_key: &str) {
        self.idle_pending.remove(seed_key);
        *self.connections.entry(seed_key.to_string()).or_insert(0) += 1;
    }

    /// Decrement the connection refcount; schedule idle cleanup at zero.
    pub fn disconnect(self: &Arc<Self>, seed_key: &str) {
        let mut remove = false;
        if let Some(mut c) = self.connections.get_mut(seed_key) {
            *c -= 1;
            if *c <= 0 {
                remove = true;
            }
        }
        if remove {
            self.connections.remove(seed_key);
            self.schedule_idle_cleanup(seed_key);
        }
    }

    fn schedule_idle_cleanup(self: &Arc<Self>, seed_key: &str) {
        if self.idle_timeout <= 0.0 || !self.slots.contains_key(seed_key) {
            return;
        }
        self.idle_pending.insert(seed_key.to_string(), ());
        let pool = self.clone();
        let key = seed_key.to_string();
        let timeout = self.idle_timeout;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(timeout)).await;
            // Cancelled (removed by connect / reschedule) → do nothing.
            if pool.idle_pending.remove(&key).is_none() {
                return;
            }
            if pool.connections.get(&key).map(|c| *c > 0).unwrap_or(false) {
                return;
            }
            tracing::info!("Cleaning up idle Chrome process (seed={key})");
            pool.cleanup_process(&key).await;
        });
    }

    /// Get an existing live process for `seed`, or launch a new one.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_or_launch(
        self: &Arc<Self>,
        seed: Option<String>,
        extra_args: Option<Vec<String>>,
        timezone: Option<String>,
        locale: Option<String>,
        proxy: Option<String>,
        geoip: bool,
    ) -> Result<Resolved> {
        // Apply CLI defaults when query params don't provide values.
        let mut seed = seed;
        if seed.is_none() {
            if let Some(d) = &self.default_seed {
                seed = Some(d.clone());
            }
        }
        let mut locale = locale;
        if locale.is_none() {
            locale = self.default_locale.clone();
        }
        let mut timezone = timezone;
        if timezone.is_none() {
            timezone = self.default_timezone.clone();
        }

        // Determine seed_key + actual_seed.
        let (seed_key, actual_seed) = match &seed {
            None => {
                let n: u32 = rand::Rng::gen_range(&mut rand::thread_rng(), 10000..=99999);
                (DEFAULT_SEED_KEY.to_string(), n.to_string())
            }
            Some(s) => {
                if !SAFE_SEED_RE.is_match(s) || RESERVED_SEEDS.contains(s.as_str()) {
                    return Err(CloakError::InvalidSeed);
                }
                (s.clone(), s.clone())
            }
        };

        let slot = self.slot(&seed_key);
        let _launch_guard = slot.launch_lock.lock().await;

        // Fast path: an existing, live process (or clean up a dead one).
        {
            let mut proc_guard = slot.process.lock().await;
            let live_port = proc_guard.as_mut().and_then(|cp| {
                if is_alive(&mut cp.child) {
                    Some((cp.cdp_port, cp.timezone.clone(), cp.locale.clone(), cp.proxy.clone()))
                } else {
                    None
                }
            });
            if let Some((port, tz, loc, px)) = live_port {
                if self.idle_pending.contains_key(&seed_key) {
                    self.schedule_idle_cleanup(&seed_key);
                }
                if extra_args.as_ref().map(|v| !v.is_empty()).unwrap_or(false)
                    || timezone.is_some()
                    || locale.is_some()
                    || proxy.is_some()
                    || geoip
                {
                    tracing::warn!(
                        "Seed {seed_key} already running (port {port}, tz={tz:?}, locale={loc:?}, \
                         proxy={px:?}) — ignoring new params (first-launch wins)"
                    );
                }
                drop(proc_guard);
                return Ok(Resolved { seed_key, cdp_port: port });
            }
            // Dead entry (or none): clean up and relaunch below.
            if let Some(mut dead) = proc_guard.take() {
                let _ = dead.child.start_kill();
                let _ = dead.child.wait().await;
                self.safe_rmtree(&dead.user_data_dir);
            }
        }

        // Resolve geoip if requested (proxy present).
        let mut exit_ip: Option<String> = None;
        if geoip {
            if let Some(px) = &proxy {
                let p = Proxy::Url(px.clone());
                let (tz, loc, ip) = crate::proxy::maybe_resolve_geoip(
                    true,
                    Some(&p),
                    timezone.clone(),
                    locale.clone(),
                    None,
                )
                .await;
                timezone = tz;
                locale = loc;
                exit_ip = ip;
            }
        }

        // Build the fingerprint extra-args set.
        let mut fp_extra: Vec<String> = vec![format!("--fingerprint={actual_seed}")];
        if let Some(ea) = &extra_args {
            fp_extra.extend(ea.iter().cloned());
        }
        if let Some(px) = &proxy {
            fp_extra.push(format!("--proxy-server={}", normalize_socks_string_url(px)));
        }

        // WebRTC IP spoofing: resolve `auto`, inject geoip exit IP if none present.
        let proxy_obj = proxy.as_ref().map(|p| Proxy::Url(p.clone()));
        let mut fp_extra = crate::proxy::resolve_webrtc_args(Some(fp_extra), proxy_obj.as_ref())
            .await
            .unwrap_or_default();
        if let Some(ip) = &exit_ip {
            if !fp_extra.iter().any(|a| a.starts_with("--fingerprint-webrtc-ip")) {
                fp_extra.push(format!("--fingerprint-webrtc-ip={ip}"));
            }
        }

        let chrome_args = crate::args::build_args(
            true,
            Some(fp_extra),
            timezone.as_deref(),
            locale.as_deref(),
            self.headless,
            None,
            false,
        );

        // Allocate port and user data dir.
        let port = self.allocate_port()?;
        let user_data_dir = self.data_dir.join(&seed_key);
        std::fs::create_dir_all(&user_data_dir)?;

        let mut full_args: Vec<String> = Vec::new();
        full_args.extend(BASE_CHROME_ARGS.iter().map(|s| s.to_string()));
        full_args.extend(chrome_args);
        full_args.extend(self.global_args.iter().cloned());
        full_args.push(format!("--remote-debugging-port={port}"));
        full_args.push("--remote-debugging-address=127.0.0.1".to_string());
        full_args.push(format!("--user-data-dir={}", user_data_dir.display()));

        tracing::info!("Launching Chrome (seed={actual_seed}, port={port})");
        let child = tokio::process::Command::new(&self.binary)
            .args(&full_args)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| CloakError::Serve(format!("failed to spawn Chrome: {e}")))?;

        let mut cp = ChromeProcess {
            seed: actual_seed.clone(),
            child,
            cdp_port: port,
            user_data_dir: user_data_dir.clone(),
            timezone,
            locale,
            proxy: proxy.clone(),
        };

        // Wait for CDP to be ready.
        if !wait_for_cdp(port, 10.0).await {
            let _ = cp.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), cp.child.wait()).await;
            self.safe_rmtree(&user_data_dir);
            return Err(CloakError::Serve("Chrome failed to start".into()));
        }

        let pid = cp.child.id();
        tracing::info!("Chrome ready (seed={actual_seed}, port={port}, pid={pid:?})");

        {
            let mut proc_guard = slot.process.lock().await;
            *proc_guard = Some(cp);
        }

        Ok(Resolved { seed_key, cdp_port: port })
    }

    /// Terminate a Chrome process and clean up its resources.
    async fn cleanup_process(self: &Arc<Self>, seed_key: &str) {
        self.idle_pending.remove(seed_key);
        let slot = match self.slots.get(seed_key) {
            Some(s) => s.clone(),
            None => return,
        };
        let taken = {
            let mut g = slot.process.lock().await;
            g.take()
        };
        if let Some(mut cp) = taken {
            if is_alive(&mut cp.child) {
                let _ = cp.child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), cp.child.wait()).await;
            }
            self.safe_rmtree(&cp.user_data_dir);
        }
        self.connections.remove(seed_key);
    }

    /// Terminate all Chrome processes (invoked on shutdown).
    pub async fn shutdown(self: &Arc<Self>) {
        self.idle_pending.clear();
        let keys: Vec<String> = self.slots.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            self.cleanup_process(&key).await;
        }
        tracing::info!("All Chrome processes terminated");
    }

    /// Snapshot of live processes for the status endpoint.
    async fn status_processes(&self) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        let keys: Vec<String> = self.slots.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            let slot = match self.slots.get(&key) {
                Some(s) => s.clone(),
                None => continue,
            };
            let mut g = slot.process.lock().await;
            if let Some(cp) = g.as_mut() {
                if is_alive(&mut cp.child) {
                    out.push((
                        key.clone(),
                        serde_json::json!({
                            "pid": cp.child.id(),
                            "port": cp.cdp_port,
                            "seed": cp.seed,
                            "connections": self.connections.get(&key).map(|c| *c).unwrap_or(0),
                            "idle_cleanup_pending": self.idle_pending.contains_key(&key),
                            "timezone": cp.timezone,
                            "locale": cp.locale,
                            "proxy": cp.proxy,
                        }),
                    ));
                }
            }
        }
        out
    }
}

/// True if the child process is still running (non-blocking check).
fn is_alive(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

/// Poll Chrome's `/json/version` until it returns 200, up to `timeout` seconds.
async fn wait_for_cdp(port: u16, timeout: f64) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    let mut delay = 0.1_f64;
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let url = format!("http://127.0.0.1:{port}/json/version");
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs_f64(delay)).await;
        delay = (delay * 2.0).min(1.0);
    }
    false
}

// ---------------------------------------------------------------------------
// Request-derived helpers
// ---------------------------------------------------------------------------

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// `"wss"` if the client connected via HTTPS (TLS-terminating proxy), else `"ws"`.
fn ws_scheme(headers: &HeaderMap) -> &'static str {
    let proto = header_str(headers, "x-forwarded-proto").unwrap_or("http");
    let proto = proto.split(',').next().unwrap_or("").trim().to_lowercase();
    if proto == "https" {
        "wss"
    } else {
        "ws"
    }
}

/// The public host to embed in rewritten CDP WebSocket URLs.
fn external_host(headers: &HeaderMap, port: u16) -> String {
    if let Some(fwd) = header_str(headers, "x-forwarded-host") {
        let public = fwd.split(',').next().unwrap_or("").trim();
        if !public.is_empty() {
            return public.to_string();
        }
    }
    if let Some(h) = header_str(headers, "host") {
        if !h.is_empty() {
            return h.to_string();
        }
    }
    format!("localhost:{port}")
}

/// Scheme used for origin comparison (X-Forwarded-Proto, fallback `http`).
fn request_scheme(headers: &HeaderMap) -> String {
    header_str(headers, "x-forwarded-proto")
        .unwrap_or("http")
        .to_string()
}

// ---------------------------------------------------------------------------
// axum app state + handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    pool: Arc<ChromePool>,
    port: u16,
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn handle_root(State(state): State<AppState>) -> Response {
    let processes = state.pool.status_processes().await;
    let mut map = serde_json::Map::new();
    for (k, v) in &processes {
        map.insert(k.clone(), v.clone());
    }
    let body = serde_json::json!({
        "status": "ok",
        "active": processes.len(),
        "idle_timeout": state.pool.idle_timeout,
        "processes": serde_json::Value::Object(map),
    });
    json_response(StatusCode::OK, &body)
}

async fn handle_json_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    let params = parse_connection_params(q.as_deref().unwrap_or(""));
    let resolved = match launch_from_params(&state, &params).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let upstream = format!("http://127.0.0.1:{}/json/version", resolved.cdp_port);
    let mut data = match fetch_json(&upstream).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to reach Chrome CDP (port {}): {e}", resolved.cdp_port);
            return json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error": "CDP endpoint unreachable"}),
            );
        }
    };

    let host = external_host(&headers, state.port);
    let scheme = ws_scheme(&headers);
    let ws_path = match &params.seed {
        Some(seed_key) => format!("fingerprint/{seed_key}/devtools/browser"),
        None => "devtools/browser".to_string(),
    };
    let orig_ws = data
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let guid: String = if orig_ws.contains("/devtools/") {
        orig_ws.rsplit('/').next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "webSocketDebuggerUrl".to_string(),
            serde_json::Value::String(format!("{scheme}://{host}/{ws_path}/{guid}")),
        );
    }
    json_response(StatusCode::OK, &data)
}

async fn handle_json_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    let params = parse_connection_params(q.as_deref().unwrap_or(""));
    let resolved = match launch_from_params(&state, &params).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let upstream = format!("http://127.0.0.1:{}/json/list", resolved.cdp_port);
    let mut data = match fetch_json(&upstream).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to reach Chrome CDP (port {}): {e}", resolved.cdp_port);
            return json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error": "CDP endpoint unreachable"}),
            );
        }
    };

    let host = external_host(&headers, state.port);
    let scheme = ws_scheme(&headers);
    let seed_key = params.seed.clone();

    if let Some(arr) = data.as_array_mut() {
        for entry in arr.iter_mut() {
            let ws_url = entry
                .get("webSocketDebuggerUrl")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(ws_url) = ws_url {
                let ws_tail = ws_url.split("/devtools/").last().unwrap_or("");
                let rewritten = match &seed_key {
                    Some(sk) => format!("{scheme}://{host}/fingerprint/{sk}/devtools/{ws_tail}"),
                    None => format!("{scheme}://{host}/devtools/{ws_tail}"),
                };
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert(
                        "webSocketDebuggerUrl".to_string(),
                        serde_json::Value::String(rewritten),
                    );
                }
            }
        }
    }
    json_response(StatusCode::OK, &data)
}

/// Shared launch path for the two JSON HTTP handlers; maps errors to responses.
async fn launch_from_params(
    state: &AppState,
    params: &ConnectionParams,
) -> std::result::Result<Resolved, Response> {
    match state
        .pool
        .get_or_launch(
            params.seed.clone(),
            if params.extra_args.is_empty() {
                None
            } else {
                Some(params.extra_args.clone())
            },
            params.timezone.clone(),
            params.locale.clone(),
            params.proxy.clone(),
            params.geoip,
        )
        .await
    {
        Ok(r) => Ok(r),
        Err(CloakError::InvalidSeed) => Err(json_response(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({"error": "Invalid fingerprint seed"}),
        )),
        Err(e) => {
            tracing::error!("get_or_launch failed: {e}");
            Err(json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error": "Chrome failed to start"}),
            ))
        }
    }
}

async fn fetch_json(url: &str) -> Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let resp = client.get(url).send().await?;
    let text = resp.text().await?;
    let val = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| CloakError::Serve(format!("invalid JSON from CDP: {e}")))?;
    Ok(val)
}

// ---------------------------------------------------------------------------
// WebSocket handlers
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct SeedPath {
    seed: String,
    path: String,
}

async fn handle_ws_seed(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(sp): axum::extract::Path<SeedPath>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(resp) = reject_untrusted_origin(&headers) {
        return resp;
    }
    let seed = sp.seed;
    let path = sp.path;

    let resolved = match state
        .pool
        .get_or_launch(Some(seed.clone()), None, None, None, None, false)
        .await
    {
        Ok(r) => r,
        Err(CloakError::InvalidSeed) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error": "Invalid fingerprint seed"}),
            );
        }
        Err(e) => {
            tracing::error!("get_or_launch failed: {e}");
            return json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error": "Chrome failed to start"}),
            );
        }
    };

    let pool = state.pool.clone();
    let seed_key = resolved.seed_key.clone();
    let target = format!("ws://127.0.0.1:{}/devtools/{path}", resolved.cdp_port);
    let label = format!("CDP seed={seed} [{path}]");

    pool.connect(&seed_key);
    ws.on_upgrade(move |socket| async move {
        proxy_cdp_websocket(socket, target, label).await;
        pool.disconnect(&seed_key);
    })
}

async fn handle_ws_default(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(path): axum::extract::Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(resp) = reject_untrusted_origin(&headers) {
        return resp;
    }

    let resolved = match state
        .pool
        .get_or_launch(None, None, None, None, None, false)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("get_or_launch failed: {e}");
            return json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error": "Chrome failed to start"}),
            );
        }
    };

    let pool = state.pool.clone();
    let seed_key = DEFAULT_SEED_KEY.to_string();
    let target = format!("ws://127.0.0.1:{}/devtools/{path}", resolved.cdp_port);
    let label = format!("CDP default [{path}]");

    pool.connect(&seed_key);
    ws.on_upgrade(move |socket| async move {
        proxy_cdp_websocket(socket, target, label).await;
        pool.disconnect(&seed_key);
    })
}

/// Reject browser-origin WebSocket upgrades that would expose local CDP.
fn reject_untrusted_origin(headers: &HeaderMap) -> Option<Response> {
    let origin = header_str(headers, "origin");
    let host = header_str(headers, "host");
    let scheme = request_scheme(headers);
    if origin_is_allowed(origin, host, &scheme) {
        None
    } else {
        tracing::warn!(
            "Rejected CDP WebSocket from untrusted Origin {:?} for Host {:?}",
            origin,
            host
        );
        Some(
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Body::from("Forbidden: untrusted WebSocket origin\n"))
                .unwrap(),
        )
    }
}

/// Bidirectional WebSocket proxy between the client and Chrome CDP.
async fn proxy_cdp_websocket(client_ws: WebSocket, target_url: String, label: String) {
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

    let config = WebSocketConfig {
        max_message_size: None,
        max_frame_size: None,
        ..Default::default()
    };
    let cdp_ws =
        match tokio_tungstenite::connect_async_with_config(&target_url, Some(config), false).await {
            Ok((ws, _resp)) => ws,
            Err(e) => {
                tracing::error!("{label} error: {e}");
                return;
            }
        };
    tracing::info!("{label}: connected to {target_url}");

    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut cdp_tx, mut cdp_rx) = cdp_ws.split();

    let l1 = label.clone();
    let client_to_cdp = async move {
        while let Some(msg) = client_rx.next().await {
            match msg {
                Ok(AxumMessage::Text(t)) => {
                    if cdp_tx.send(TungMessage::Text(t)).await.is_err() {
                        break;
                    }
                }
                Ok(AxumMessage::Binary(b)) => {
                    if cdp_tx.send(TungMessage::Binary(b)).await.is_err() {
                        break;
                    }
                }
                Ok(AxumMessage::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("{l1} [c->cdp]: {e}");
                    break;
                }
            }
        }
        let _ = cdp_tx.close().await;
    };

    let l2 = label.clone();
    let cdp_to_client = async move {
        while let Some(msg) = cdp_rx.next().await {
            match msg {
                Ok(TungMessage::Text(t)) => {
                    if client_tx.send(AxumMessage::Text(t)).await.is_err() {
                        break;
                    }
                }
                Ok(TungMessage::Binary(b)) => {
                    if client_tx.send(AxumMessage::Binary(b)).await.is_err() {
                        break;
                    }
                }
                Ok(TungMessage::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("{l2} [cdp->c]: {e}");
                    break;
                }
            }
        }
        let _ = client_tx.close().await;
    };

    tokio::select! {
        _ = client_to_cdp => {},
        _ = cdp_to_client => {},
    }
    tracing::info!("{label}: disconnected");
}

// ---------------------------------------------------------------------------
// Container detection + defaults
// ---------------------------------------------------------------------------

fn in_container() -> bool {
    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

fn default_data_dir() -> PathBuf {
    if in_container() {
        PathBuf::from("/tmp/cloakserve")
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cloakbrowser")
            .join("cloakserve")
    }
}

// ---------------------------------------------------------------------------
// run()
// ---------------------------------------------------------------------------

/// Run the CDP multiplexer server (blocks until shutdown / Ctrl-C).
pub async fn run(config: ServeConfig) -> Result<()> {
    // Validate CLI default seed (mirrors main()'s sys.exit(1) guard).
    if let Some(seed) = &config.default_seed {
        if !SAFE_SEED_RE.is_match(seed) || RESERVED_SEEDS.contains(seed.as_str()) {
            return Err(CloakError::Serve(format!("Invalid --fingerprint seed: {seed}")));
        }
    }

    let binary = crate::download::ensure_binary(None).await?;

    let data_dir = config
        .data_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);

    let pool = Arc::new(ChromePool::new(
        binary,
        config.global_args.clone(),
        config.headless,
        data_dir,
        config.default_seed.clone(),
        config.default_locale.clone(),
        config.default_timezone.clone(),
        config.idle_timeout,
    ));

    let state = AppState {
        pool: pool.clone(),
        port: config.port,
    };

    // Routes — seed WS must be registered before default WS to match first.
    let app = Router::new()
        .route("/", get(handle_root))
        .route("/json/version", get(handle_json_version))
        .route("/json/version/", get(handle_json_version))
        .route("/json/list", get(handle_json_list))
        .route("/json/list/", get(handle_json_list))
        .route("/json", get(handle_json_list))
        .route("/json/", get(handle_json_list))
        .route("/fingerprint/:seed/devtools/*path", get(handle_ws_seed))
        .route("/devtools/*path", get(handle_ws_default))
        .with_state(state);

    let host = if in_container() { "0.0.0.0" } else { "127.0.0.1" };
    let addr: std::net::SocketAddr = format!("{host}:{}", config.port)
        .parse()
        .map_err(|e| CloakError::Serve(format!("invalid bind address: {e}")))?;

    tracing::info!("CloakBrowser CDP multiplexer starting on port {}", config.port);
    tracing::info!(
        "Connect: playwright.chromium.connect_over_cdp(\"http://localhost:{}?fingerprint=<seed>\")",
        config.port
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CloakError::Serve(format!("failed to bind {addr}: {e}")))?;

    let shutdown_pool = pool.clone();
    let serve = axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        });

    let result = serve.await;
    shutdown_pool.shutdown().await;
    result.map_err(|e| CloakError::Serve(format!("server error: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_seed_accepts_valid() {
        assert!(SAFE_SEED_RE.is_match("12345"));
        assert!(SAFE_SEED_RE.is_match("abc_DEF-123"));
        assert!(SAFE_SEED_RE.is_match("a"));
        assert!(SAFE_SEED_RE.is_match(&"x".repeat(128)));
    }

    #[test]
    fn safe_seed_rejects_invalid() {
        assert!(!SAFE_SEED_RE.is_match(""));
        assert!(!SAFE_SEED_RE.is_match("has space"));
        assert!(!SAFE_SEED_RE.is_match("has/slash"));
        assert!(!SAFE_SEED_RE.is_match("dot.dot"));
        assert!(!SAFE_SEED_RE.is_match(&"x".repeat(129)));
        assert!(!SAFE_SEED_RE.is_match("../etc"));
    }

    #[test]
    fn reserved_seed_present() {
        assert!(RESERVED_SEEDS.contains("__default__"));
    }

    #[test]
    fn origin_none_is_allowed() {
        // Non-browser CDP clients omit Origin.
        assert!(origin_is_allowed(None, Some("127.0.0.1:9222"), "http"));
    }

    #[test]
    fn origin_empty_or_null_rejected() {
        assert!(!origin_is_allowed(Some(""), Some("127.0.0.1:9222"), "http"));
        assert!(!origin_is_allowed(Some("null"), Some("127.0.0.1:9222"), "http"));
        assert!(!origin_is_allowed(Some("NULL"), Some("127.0.0.1:9222"), "http"));
    }

    #[test]
    fn origin_trusted_devtools_allowed() {
        assert!(origin_is_allowed(
            Some("devtools://devtools"),
            Some("evil.example.com"),
            "http"
        ));
        assert!(origin_is_allowed(
            Some("chrome-devtools://devtools"),
            Some("127.0.0.1:9222"),
            "http"
        ));
    }

    #[test]
    fn origin_matching_loopback_allowed() {
        assert!(origin_is_allowed(
            Some("http://127.0.0.1:9222"),
            Some("127.0.0.1:9222"),
            "http"
        ));
        assert!(origin_is_allowed(
            Some("http://localhost:9222"),
            Some("localhost:9222"),
            "http"
        ));
    }

    #[test]
    fn origin_mismatched_port_rejected() {
        assert!(!origin_is_allowed(
            Some("http://127.0.0.1:8080"),
            Some("127.0.0.1:9222"),
            "http"
        ));
    }

    #[test]
    fn origin_non_loopback_request_host_rejected() {
        // Even if origin == request host, a non-loopback request host is rejected.
        assert!(!origin_is_allowed(
            Some("http://evil.example.com"),
            Some("evil.example.com"),
            "http"
        ));
    }

    #[test]
    fn origin_bad_scheme_rejected() {
        assert!(!origin_is_allowed(
            Some("ftp://127.0.0.1:9222"),
            Some("127.0.0.1:9222"),
            "http"
        ));
    }

    #[test]
    fn origin_with_path_rejected() {
        assert!(!origin_is_allowed(
            Some("http://127.0.0.1:9222/evil"),
            Some("127.0.0.1:9222"),
            "http"
        ));
    }

    #[test]
    fn origin_default_ports_match() {
        // http origin (default 80) vs http request (default 80) on loopback.
        assert!(origin_is_allowed(
            Some("http://127.0.0.1"),
            Some("127.0.0.1"),
            "http"
        ));
        // https origin default 443 must not equal http request default 80.
        assert!(!origin_is_allowed(
            Some("https://127.0.0.1"),
            Some("127.0.0.1"),
            "http"
        ));
    }

    #[test]
    fn loopback_host_detection() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.0.0.1."));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("8.8.8.8"));
        assert!(!is_loopback_host("example.com"));
    }

    #[test]
    fn parse_params_basic() {
        let p = parse_connection_params("fingerprint=12345&timezone=America/New_York&locale=en-US");
        assert_eq!(p.seed.as_deref(), Some("12345"));
        assert_eq!(p.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(p.locale.as_deref(), Some("en-US"));
        assert!(!p.geoip);
        assert!(p.extra_args.is_empty());
    }

    #[test]
    fn parse_params_geoip_and_generic() {
        let p = parse_connection_params("geoip=yes&platform=Win32");
        assert!(p.geoip);
        assert_eq!(p.extra_args, vec!["--fingerprint-platform=Win32".to_string()]);
    }
}
