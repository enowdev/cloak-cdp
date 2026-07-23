//! GeoIP resolution (timezone/locale/exit-IP) via MaxMind + IP echo services.
//! See docs/SPEC-geoip-proxy.md (PART A).
//!
//! Port of `cloakbrowser/geoip.py`. Resolves a proxy's (or the machine's) egress
//! IP and, via a MaxMind GeoLite2-City database, its timezone and BCP47 locale.

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt;
use once_cell::sync::Lazy;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::error::{CloakError, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// GeoLite2-City mmdb source (follow redirects, ~70MB).
const GEOIP_DB_URL: &str =
    "https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-City.mmdb";
/// On-disk filename for the MaxMind city database.
const GEOIP_DB_FILENAME: &str = "GeoLite2-City.mmdb";
/// Re-download the DB in the background once it is older than this (30 days).
const GEOIP_UPDATE_INTERVAL: Duration = Duration::from_secs(30 * 86_400);
/// Default resolution timeout when the env var is unset/empty.
const DEFAULT_GEOIP_TIMEOUT_SECONDS: f64 = 5.0;
/// Env var overriding the GeoIP resolution timeout (in seconds).
const GEOIP_TIMEOUT_ENV: &str = "CLOAKBROWSER_GEOIP_TIMEOUT_SECONDS";

/// IP echo endpoints tried in order; each returns a plaintext IP body.
const IP_ECHO_URLS: &[&str] = &[
    "https://api.ipify.org",
    "https://checkip.amazonaws.com",
    "https://ifconfig.me/ip",
];

/// Process-global mutex serializing DB downloads (first-use blocking; background try-lock).
static GEOIP_DOWNLOAD_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// ISO alpha-2 country code → BCP47 locale (verbatim from the Python port).
static COUNTRY_LOCALE_MAP: Lazy<std::collections::HashMap<&'static str, &'static str>> =
    Lazy::new(|| {
        [
            ("US", "en-US"), ("GB", "en-GB"), ("AU", "en-AU"), ("CA", "en-CA"),
            ("NZ", "en-NZ"), ("IE", "en-IE"), ("ZA", "en-ZA"), ("SG", "en-SG"),
            ("DE", "de-DE"), ("AT", "de-AT"), ("CH", "de-CH"), ("FR", "fr-FR"),
            ("BE", "fr-BE"), ("ES", "es-ES"), ("MX", "es-MX"), ("AR", "es-AR"),
            ("CO", "es-CO"), ("CL", "es-CL"), ("BR", "pt-BR"), ("PT", "pt-PT"),
            ("IT", "it-IT"), ("NL", "nl-NL"), ("JP", "ja-JP"), ("KR", "ko-KR"),
            ("CN", "zh-CN"), ("TW", "zh-TW"), ("HK", "zh-HK"), ("RU", "ru-RU"),
            ("UA", "uk-UA"), ("PL", "pl-PL"), ("CZ", "cs-CZ"), ("RO", "ro-RO"),
            ("IL", "he-IL"), ("TR", "tr-TR"), ("SA", "ar-SA"), ("AE", "ar-AE"),
            ("EG", "ar-EG"), ("IN", "hi-IN"), ("ID", "id-ID"), ("PH", "en-PH"),
            ("TH", "th-TH"), ("VN", "vi-VN"), ("MY", "ms-MY"), ("SE", "sv-SE"),
            ("NO", "nb-NO"), ("DK", "da-DK"), ("FI", "fi-FI"), ("GR", "el-GR"),
            ("HU", "hu-HU"), ("BG", "bg-BG"), ("SI", "sl-SI"), ("SK", "sk-SK"),
            ("HR", "hr-HR"), ("RS", "sr-RS"), ("LT", "lt-LT"), ("LV", "lv-LV"),
            ("EE", "et-EE"), ("IS", "is-IS"), ("LU", "fr-LU"), ("MT", "en-MT"),
            ("CY", "el-CY"), ("MD", "ro-MD"), ("BY", "ru-BY"), ("GE", "ka-GE"),
            ("AL", "sq-AL"), ("MK", "mk-MK"), ("BA", "bs-BA"), ("PE", "es-PE"),
            ("VE", "es-VE"), ("EC", "es-EC"), ("UY", "es-UY"), ("CR", "es-CR"),
            ("DO", "es-DO"), ("GT", "es-GT"), ("BO", "es-BO"), ("PY", "es-PY"),
            ("PK", "en-PK"), ("BD", "bn-BD"), ("LK", "si-LK"), ("KZ", "ru-KZ"),
            ("IR", "fa-IR"), ("IQ", "ar-IQ"), ("JO", "ar-JO"), ("LB", "ar-LB"),
            ("KW", "ar-KW"), ("QA", "ar-QA"), ("OM", "ar-OM"), ("BH", "ar-BH"),
            ("NG", "en-NG"), ("KE", "en-KE"), ("MA", "fr-MA"), ("DZ", "ar-DZ"),
            ("TN", "ar-TN"), ("GH", "en-GH"), ("AM", "hy-AM"), ("AZ", "az-AZ"),
            ("UZ", "uz-UZ"), ("KG", "ky-KG"), ("TJ", "tg-TJ"), ("TM", "tk-TM"),
            ("ME", "sr-ME"), ("XK", "sq-XK"), ("LI", "de-LI"), ("MC", "fr-MC"),
            ("AD", "ca-AD"), ("MM", "my-MM"), ("KH", "km-KH"), ("LA", "lo-LA"),
            ("MN", "mn-MN"), ("BN", "ms-BN"), ("MO", "zh-MO"), ("YE", "ar-YE"),
            ("SY", "ar-SY"), ("PS", "ar-PS"), ("LY", "ar-LY"), ("ET", "am-ET"),
            ("TZ", "sw-TZ"), ("UG", "en-UG"), ("SN", "fr-SN"), ("CI", "fr-CI"),
            ("CM", "fr-CM"), ("AO", "pt-AO"), ("MZ", "pt-MZ"), ("ZM", "en-ZM"),
            ("ZW", "en-ZW"), ("HN", "es-HN"), ("NI", "es-NI"), ("SV", "es-SV"),
            ("PA", "es-PA"), ("JM", "en-JM"), ("TT", "en-TT"), ("PR", "es-PR"),
        ]
        .into_iter()
        .collect()
    });

// ---------------------------------------------------------------------------
// Timeout / deadline helpers
// ---------------------------------------------------------------------------

/// Read `CLOAKBROWSER_GEOIP_TIMEOUT_SECONDS`; empty → default 5.0; unparseable/non-finite
/// → warn + 5.0; negative → clamped to 0.0.
fn get_geoip_timeout_seconds() -> f64 {
    let raw = std::env::var(GEOIP_TIMEOUT_ENV).unwrap_or_default();
    if raw.trim().is_empty() {
        return DEFAULT_GEOIP_TIMEOUT_SECONDS;
    }
    // float(raw) in Python; ValueError → NaN → not finite → warn + default.
    let timeout: f64 = raw.trim().parse().unwrap_or(f64::NAN);
    if !timeout.is_finite() {
        warn!("Invalid {GEOIP_TIMEOUT_ENV}={raw:?}; using {DEFAULT_GEOIP_TIMEOUT_SECONDS}s");
        return DEFAULT_GEOIP_TIMEOUT_SECONDS;
    }
    timeout.max(0.0)
}

/// A monotonic deadline. `t <= 0` → no deadline (`None`).
fn deadline_from_timeout(t: f64) -> Option<Instant> {
    if t <= 0.0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs_f64(t))
    }
}

/// Seconds remaining until `deadline`; `None` deadline → `None`; past → 0.0.
fn remaining_seconds(deadline: Option<Instant>) -> Option<f64> {
    deadline.map(|d| {
        let now = Instant::now();
        if d > now {
            (d - now).as_secs_f64()
        } else {
            0.0
        }
    })
}

/// Whether a (present) deadline has passed.
fn deadline_expired(deadline: Option<Instant>) -> bool {
    matches!(deadline, Some(d) if Instant::now() >= d)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve `(timezone, locale)` from a proxy (or the machine's egress).
pub async fn resolve_proxy_geo(proxy_url: Option<&str>) -> (Option<String>, Option<String>) {
    let (tz, locale, _ip) = resolve_proxy_geo_with_ip(proxy_url).await;
    (tz, locale)
}

/// Resolve `(timezone, locale, exit_ip)`. Never returns an error — failures degrade to `None`s.
pub async fn resolve_proxy_geo_with_ip(
    proxy_url: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    // (2) Ensure DB BEFORE starting the resolution deadline — the ~70MB download
    // is not bounded by the resolution timeout.
    let db_path = ensure_geoip_db().await;

    // (3) Start the deadline.
    let timeout = get_geoip_timeout_seconds();
    let deadline = deadline_from_timeout(timeout);

    // (4) Discover the exit IP through the proxy.
    let mut ip = resolve_exit_ip(proxy_url, remaining_seconds(deadline)).await;

    // (5) Fall back to resolving the proxy host itself if still within budget.
    if ip.is_none() {
        if let Some(url) = proxy_url {
            if !deadline_expired(deadline) {
                ip = resolve_proxy_ip(url);
            }
        }
    }

    // (6) Give up if no IP or the deadline has expired.
    if ip.is_none() || deadline_expired(deadline) {
        if deadline_expired(deadline) {
            warn!("GeoIP resolution timed out");
        }
        return (None, None, None);
    }
    let ip = ip.unwrap();

    // (7) No DB → still return exit IP for WebRTC.
    let db_path = match db_path {
        Some(p) => p,
        None => return (None, None, Some(ip)),
    };

    // (8) MaxMind city lookup.
    match lookup_city(&db_path, &ip) {
        Ok((tz, locale)) => (tz, locale, Some(ip)),
        Err(e) => {
            // (9)
            warn!("GeoIP city lookup failed for {ip}: {e}");
            (None, None, Some(ip))
        }
    }
}

/// Resolve just the egress IP (for WebRTC spoofing). Never returns an error.
pub async fn resolve_proxy_exit_ip(proxy_url: Option<&str>) -> Option<String> {
    let timeout = get_geoip_timeout_seconds();
    let deadline = deadline_from_timeout(timeout);
    let ip = resolve_exit_ip(proxy_url, Some(timeout)).await;
    if ip.is_none() && deadline_expired(deadline) {
        warn!("Exit IP resolution timed out");
    }
    ip
}

// ---------------------------------------------------------------------------
// MaxMind lookup
// ---------------------------------------------------------------------------

/// Open a MaxMind reader, look up `ip`, and extract `(timezone, locale)`.
fn lookup_city(db_path: &Path, ip: &str) -> Result<(Option<String>, Option<String>)> {
    let addr: IpAddr = ip
        .parse()
        .map_err(|_| CloakError::GeoIp(format!("invalid ip {ip:?}")))?;
    let reader = maxminddb::Reader::open_readfile(db_path)
        .map_err(|e| CloakError::GeoIp(format!("open mmdb: {e}")))?;
    let city: maxminddb::geoip2::City = reader
        .lookup(addr)
        .map_err(|e| CloakError::GeoIp(format!("lookup: {e}")))?;

    let timezone = city
        .location
        .as_ref()
        .and_then(|l| l.time_zone)
        .map(|s| s.to_string());
    let country = city.country.as_ref().and_then(|c| c.iso_code);
    let locale = country
        .and_then(|c| COUNTRY_LOCALE_MAP.get(c).copied())
        .map(|s| s.to_string());
    Ok((timezone, locale))
}

// ---------------------------------------------------------------------------
// Exit-IP discovery
// ---------------------------------------------------------------------------

/// Discover the exit IP through the proxy by hitting the IP-echo endpoints in order.
/// `timeout` (seconds) bounds the whole operation. Never returns an error.
async fn resolve_exit_ip(proxy_url: Option<&str>, timeout: Option<f64>) -> Option<String> {
    let deadline = deadline_from_timeout(timeout.unwrap_or(0.0));

    for url in IP_ECHO_URLS {
        // Respect the deadline.
        let remaining = remaining_seconds(deadline);
        if let Some(r) = remaining {
            if r <= 0.0 {
                return None;
            }
        }
        let request_timeout = match remaining {
            Some(r) => r.min(10.0),
            None => 10.0,
        };

        // Build a per-request client, wiring the proxy if given.
        let mut builder =
            reqwest::Client::builder().timeout(Duration::from_secs_f64(request_timeout));
        if let Some(p) = proxy_url.filter(|s| !s.is_empty()) {
            match reqwest::Proxy::all(p) {
                Ok(proxy) => builder = builder.proxy(proxy),
                Err(e) => {
                    // Reqwest cannot build the proxy — e.g. a SOCKS scheme without the
                    // socks feature. Mirror the Python "SOCKS unsupported" fast-fail.
                    warn!("SOCKS5 proxy requires socks support: {e}");
                    return None;
                }
            }
        }
        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to build HTTP client for exit-IP discovery: {e}");
                continue;
            }
        };

        match client.get(*url).send().await.and_then(|r| r.error_for_status()) {
            Ok(resp) => match resp.text().await {
                Ok(body) => {
                    let candidate = body.trim();
                    if candidate.parse::<IpAddr>().is_ok() {
                        return Some(candidate.to_string());
                    }
                    debug!("Echo {url} returned non-IP body");
                }
                Err(e) => debug!("Echo {url} body read failed: {e}"),
            },
            Err(e) => debug!("Echo {url} request failed: {e}"),
        }
    }

    warn!("Failed to discover exit IP through proxy");
    None
}

/// Resolve the proxy host to an IP (literal fast-path, else `getaddrinfo`). Never raises.
fn resolve_proxy_ip(proxy_url: &str) -> Option<String> {
    let parsed = url::Url::parse(proxy_url).ok()?;
    let raw_host = parsed.host_str().filter(|h| !h.is_empty())?;
    // `url` keeps IPv6 hosts bracketed (`[::1]`); Python's `.hostname` strips them.
    let hostname = raw_host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw_host);

    // IPv4/IPv6 literal → return as-is.
    if hostname.parse::<IpAddr>().is_ok() {
        return Some(hostname.to_string());
    }

    // getaddrinfo(hostname, None, AF_UNSPEC, SOCK_STREAM); first result.
    match (hostname, 0u16).to_socket_addrs() {
        Ok(mut addrs) => addrs.next().map(|sa| sa.ip().to_string()),
        Err(e) => {
            warn!("Failed to resolve proxy host {hostname:?}: {e}");
            None
        }
    }
}

/// `ipaddress.ip_address(ip).is_private`; invalid → `false`. Defined for parity (unused).
#[allow(dead_code)]
fn is_private_ip(ip: &str) -> bool {
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// GeoIP DB management
// ---------------------------------------------------------------------------

/// GeoIP database directory: `cache_dir/geoip`.
fn geoip_dir() -> PathBuf {
    crate::platform::geoip_dir()
}

/// Ensure the GeoLite2 city DB is present, downloading it (once, serialized) if absent.
/// Returns the path, or `None` if unavailable. Never returns an error.
async fn ensure_geoip_db() -> Option<PathBuf> {
    let db_path = geoip_dir().join(GEOIP_DB_FILENAME);

    // (2) Present → maybe trigger a background refresh and return.
    if db_path.exists() {
        maybe_trigger_update(db_path.clone());
        return Some(db_path);
    }

    // (3) Acquire the process-global lock (blocking), then download.
    let _guard = GEOIP_DOWNLOAD_LOCK.lock().await;
    if db_path.exists() {
        return Some(db_path);
    }
    match download_geoip_db(&db_path).await {
        Ok(()) => Some(db_path),
        Err(e) => {
            warn!("Failed to download GeoIP database: {e}");
            None
        }
    }
}

/// Download the DB to `dest` atomically: stream into a same-dir tempfile, then rename.
async fn download_geoip_db(dest: &Path) -> Result<()> {
    use std::io::Write;

    let parent = dest
        .parent()
        .ok_or_else(|| CloakError::GeoIp("geoip dir has no parent".into()))?
        .to_path_buf();
    tokio::fs::create_dir_all(&parent).await?;
    info!("Downloading GeoIP database (~70MB) from {GEOIP_DB_URL}");

    // Async client: follow redirects, 300s timeout.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| CloakError::GeoIp(format!("build download client: {e}")))?;

    let resp = client
        .get(GEOIP_DB_URL)
        .send()
        .await
        .map_err(|e| CloakError::GeoIp(format!("download request: {e}")))?
        .error_for_status()
        .map_err(|e| CloakError::GeoIp(format!("download status: {e}")))?;

    let total = resp.content_length();

    // Tempfile in the SAME dir so the final rename is atomic. Buffer 64KiB chunks.
    let tmp = tokio::task::spawn_blocking(move || tempfile::NamedTempFile::new_in(&parent))
        .await
        .map_err(|e| CloakError::GeoIp(format!("tempfile task: {e}")))?
        .map_err(|e| CloakError::GeoIp(format!("create tempfile: {e}")))?;

    // Write on the blocking pool while streaming bytes on the async side.
    let (mut tmp, mut writer_buf, dest_owned) = (tmp, Vec::<u8>::new(), dest.to_path_buf());
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut next_progress = 10u64; // log every >=10%.

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CloakError::GeoIp(format!("read chunk: {e}")))?;
        // Accumulate into 64KiB batches before flushing to disk.
        writer_buf.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        if writer_buf.len() >= 65536 {
            let to_write = std::mem::take(&mut writer_buf);
            tmp = tokio::task::spawn_blocking(move || -> Result<tempfile::NamedTempFile> {
                tmp.as_file().write_all(&to_write)?;
                Ok(tmp)
            })
            .await
            .map_err(|e| CloakError::GeoIp(format!("write task: {e}")))??;
        }
        if let Some(total) = total {
            if total > 0 {
                let pct = downloaded * 100 / total;
                if pct >= next_progress {
                    debug!("GeoIP download progress: {pct}%");
                    next_progress = (pct / 10 + 1) * 10;
                }
            }
        }
    }

    // Final flush + atomic persist on the blocking pool.
    tokio::task::spawn_blocking(move || -> Result<()> {
        if !writer_buf.is_empty() {
            tmp.as_file().write_all(&writer_buf)?;
        }
        tmp.as_file().flush()?;
        tmp.persist(&dest_owned)
            .map_err(|e| CloakError::GeoIp(format!("persist mmdb: {e}")))?;
        Ok(())
    })
    .await
    .map_err(|e| CloakError::GeoIp(format!("persist task: {e}")))??;

    info!("GeoIP database ready at {}", dest.display());
    Ok(())
}

/// If the DB is older than 30 days, spawn a detached background re-download (try-lock).
fn maybe_trigger_update(db_path: PathBuf) {
    // (1) Age check; any metadata error → return.
    let age = match db_path
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
    {
        Some(age) => age,
        None => return,
    };
    if age < GEOIP_UPDATE_INTERVAL {
        return;
    }

    // (2) Detached background task; try-lock so we never block a live request.
    tokio::spawn(async move {
        let guard = match GEOIP_DOWNLOAD_LOCK.try_lock() {
            Ok(g) => g,
            Err(_) => return, // another download in progress.
        };
        if let Err(e) = download_geoip_db(&db_path).await {
            debug!("Background GeoIP update failed: {e}");
        }
        drop(guard);
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_map_has_expected_entries() {
        // A representative spread from the verbatim map.
        assert_eq!(COUNTRY_LOCALE_MAP.get("US").copied(), Some("en-US"));
        assert_eq!(COUNTRY_LOCALE_MAP.get("BR").copied(), Some("pt-BR"));
        assert_eq!(COUNTRY_LOCALE_MAP.get("JP").copied(), Some("ja-JP"));
        assert_eq!(COUNTRY_LOCALE_MAP.get("XK").copied(), Some("sq-XK"));
        assert_eq!(COUNTRY_LOCALE_MAP.get("PR").copied(), Some("es-PR"));
        // Full expected count from the spec's map.
        assert_eq!(COUNTRY_LOCALE_MAP.len(), 132);
        // Unmapped country → None.
        assert_eq!(COUNTRY_LOCALE_MAP.get("ZZ").copied(), None);
    }

    #[test]
    fn timeout_parsing_env_cases() {
        // Serialize env mutation across cases; env is process-global.
        let key = GEOIP_TIMEOUT_ENV;
        let prev = std::env::var(key).ok();

        std::env::remove_var(key);
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        std::env::set_var(key, "");
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        std::env::set_var(key, "   ");
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        std::env::set_var(key, "12.5");
        assert_eq!(get_geoip_timeout_seconds(), 12.5);

        std::env::set_var(key, "-3");
        assert_eq!(get_geoip_timeout_seconds(), 0.0);

        std::env::set_var(key, "not-a-number");
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        std::env::set_var(key, "inf");
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        std::env::set_var(key, "nan");
        assert_eq!(get_geoip_timeout_seconds(), DEFAULT_GEOIP_TIMEOUT_SECONDS);

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn deadline_math_none_and_expired() {
        assert!(deadline_from_timeout(0.0).is_none());
        assert!(deadline_from_timeout(-1.0).is_none());
        assert!(remaining_seconds(None).is_none());
        assert!(!deadline_expired(None));

        let past = Instant::now() - Duration::from_secs(1);
        assert!(deadline_expired(Some(past)));
        assert_eq!(remaining_seconds(Some(past)), Some(0.0));

        let future = deadline_from_timeout(60.0);
        assert!(!deadline_expired(future));
        assert!(remaining_seconds(future).unwrap() > 0.0);
    }

    #[test]
    fn resolve_proxy_ip_literals() {
        assert_eq!(
            resolve_proxy_ip("http://1.2.3.4:8080"),
            Some("1.2.3.4".to_string())
        );
        assert_eq!(resolve_proxy_ip("http://[::1]:8080"), Some("::1".to_string()));
        assert_eq!(resolve_proxy_ip("not a url"), None);
    }

    #[test]
    fn is_private_ip_cases() {
        assert!(is_private_ip("10.0.0.1"));
        assert!(is_private_ip("192.168.1.1"));
        assert!(is_private_ip("127.0.0.1"));
        assert!(!is_private_ip("8.8.8.8"));
        assert!(!is_private_ip("garbage"));
    }
}
