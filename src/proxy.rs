//! Proxy config + WebRTC arg resolution. See docs/SPEC-geoip-proxy.md (PART B).
//!
//! Ports `browser.py`'s proxy / WebRTC helpers. The load-bearing subtlety is the
//! URL reconstruction path: credentials are percent-encoded with `quote(safe="")`
//! and the **empty-password-colon distinction** must be preserved exactly —
//! `enc_pass == None` emits *no* colon (`user@host`) while `enc_pass == Some("")`
//! emits a trailing colon (`user:@host`). Python's `urlparse`/`urlunparse` model
//! this via `None`-vs-`""` on `.password`; the Rust `url` crate collapses that
//! distinction, so the reconstruction here uses targeted string assembly instead.

use crate::Proxy;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

/// Resolved proxy configuration: chromiumoxide-level settings + extra chrome args.
///
/// At most one of the two is non-empty (mirrors Python's `(proxy_kwargs, extra_chrome_args)`).
#[derive(Debug, Default, Clone)]
pub struct ResolvedProxy {
    /// Proxy dict for the launcher (server/username/password/bypass), if any.
    pub proxy_settings: Option<crate::ProxySettings>,
    /// Extra `--proxy-server` / `--proxy-bypass-list` args, if any.
    pub extra_args: Vec<String>,
}

// ---------------------------------------------------------------------------
// Percent-encoding: quote(s, safe="") — encode every char that is NOT one of
// the RFC 3986 "unreserved" set (A-Za-z0-9 - _ . ~).
// ---------------------------------------------------------------------------

/// Everything except ASCII alphanumerics and the unreserved marks `-_.~`.
///
/// `percent_encoding` encodes the characters *in* the set, so we start from
/// `CONTROLS` and add every other non-unreserved ASCII byte. Non-ASCII bytes are
/// UTF-8 encoded and always percent-escaped by `utf8_percent_encode`.
const QUOTE_SAFE_EMPTY: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// `quote(s, safe="")` — percent-encode all non-unreserved characters.
fn quote_all(s: &str) -> String {
    utf8_percent_encode(s, QUOTE_SAFE_EMPTY).to_string()
}

/// `unquote(s)` — decode percent-escapes (lossy on invalid UTF-8, like Python's default).
fn unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

// ---------------------------------------------------------------------------
// A thin URL view that preserves the None-vs-"" password distinction that the
// `url` crate discards. Only the pieces PART B needs are modelled.
// ---------------------------------------------------------------------------

/// Parsed proxy URL retaining the `password: None` vs `Some("")` distinction.
struct ParsedProxyUrl {
    scheme: String,
    /// Raw (still percent-encoded, as they appear in the URL) userinfo pieces.
    username: Option<String>,
    password: Option<String>,
    /// Host without brackets (IPv6 literal has brackets stripped).
    hostname: Option<String>,
    port: Option<u16>,
    /// Path portion, including a leading `/` if present (may be empty).
    path: String,
    params: String,
    query: String,
    fragment: String,
}

/// Result of parsing that also flags a malformed port (Python `urlparse().port`
/// raising `ValueError`).
enum ParseOutcome {
    Ok(ParsedProxyUrl),
    /// Port field present but not a valid integer in 0..=65535.
    BadPort,
}

/// Parse a URL into the pieces PART B manipulates, mirroring `urllib.parse.urlparse`
/// closely enough for proxy URLs. Preserves `None` vs empty password.
///
/// This does NOT do full RFC parsing; it splits on the same delimiters urlparse
/// uses (`://`, `@`, `:`, `/`, `;`, `?`, `#`) in the same precedence.
fn urlparse(url: &str) -> ParseOutcome {
    // Split scheme.
    let (scheme, rest) = match url.find("://") {
        Some(i) => (url[..i].to_string(), &url[i + 3..]),
        None => (String::new(), url),
    };

    // Split off fragment (#), then query (?), from the tail.
    let (rest, fragment) = split_once_char(rest, '#');
    let (rest, query) = split_once_char(rest, '?');

    // Authority ends at the first '/'.
    let (authority, path_rest) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };

    // params (;) live in the last path segment in urlparse; for proxy URLs the
    // path is virtually always empty, but handle it for fidelity.
    let (path, params) = split_once_char(path_rest, ';');

    // Split userinfo from host on the LAST '@' in the authority.
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };

    let (username, password) = match userinfo {
        None => (None, None),
        Some(ui) => match ui.find(':') {
            Some(i) => (Some(ui[..i].to_string()), Some(ui[i + 1..].to_string())),
            None => (Some(ui.to_string()), None),
        },
    };

    // Host / port. Handle IPv6 bracketed literal.
    let (hostname, port_str): (Option<String>, Option<&str>) =
        if let Some(stripped) = hostport.strip_prefix('[') {
            match stripped.find(']') {
                Some(close) => {
                    let host = &stripped[..close];
                    let after = &stripped[close + 1..];
                    let port = after.strip_prefix(':');
                    (Some(host.to_string()), port.filter(|p| !p.is_empty()))
                }
                None => (Some(hostport.to_string()), None),
            }
        } else {
            match hostport.rfind(':') {
                Some(i) => {
                    let host = &hostport[..i];
                    let port = &hostport[i + 1..];
                    let host_opt = if host.is_empty() {
                        None
                    } else {
                        Some(host.to_string())
                    };
                    (host_opt, if port.is_empty() { None } else { Some(port) })
                }
                None => {
                    let host_opt = if hostport.is_empty() {
                        None
                    } else {
                        Some(hostport.to_string())
                    };
                    (host_opt, None)
                }
            }
        };

    // urlparse lowercases the hostname.
    let hostname = hostname.map(|h| h.to_lowercase());

    let port = match port_str {
        None => None,
        Some(p) => match p.parse::<u32>() {
            Ok(n) if n <= 65535 => Some(n as u16),
            _ => return ParseOutcome::BadPort,
        },
    };

    ParseOutcome::Ok(ParsedProxyUrl {
        scheme,
        username,
        password,
        hostname,
        port,
        path: path.to_string(),
        params: params.to_string(),
        query: query.to_string(),
        fragment: fragment.to_string(),
    })
}

/// Split on the first occurrence of `c`; returns `(before, after)` with `after`
/// empty (and no delimiter) when `c` is absent.
fn split_once_char(s: &str, c: char) -> (&str, &str) {
    match s.find(c) {
        Some(i) => (&s[..i], &s[i + c.len_utf8()..]),
        None => (s, ""),
    }
}

// ---------------------------------------------------------------------------
// URL assembly — the colon-preservation core.
// ---------------------------------------------------------------------------

/// `_assemble_proxy_url` — build a proxy URL from components.
///
/// CRITICAL: `enc_pass = Some("")` keeps the colon (`user:@host`); `enc_pass = None`
/// omits it (`user@host`). This must round-trip byte-exactly.
#[allow(clippy::too_many_arguments)]
fn assemble_proxy_url(
    scheme: &str,
    host: &str,
    port: Option<u16>,
    enc_user: &str,
    enc_pass: Option<&str>,
    path: &str,
    params: &str,
    query: &str,
    fragment: &str,
) -> String {
    // IPv6 literal → bracket it.
    let host_fmt = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    let userinfo = match enc_pass {
        Some(p) => format!("{enc_user}:{p}@"),
        None if !enc_user.is_empty() => format!("{enc_user}@"),
        None => String::new(),
    };

    let mut netloc = format!("{userinfo}{host_fmt}");
    if let Some(p) = port {
        netloc.push_str(&format!(":{p}"));
    }

    urlunparse(scheme, &netloc, path, params, query, fragment)
}

/// `urlunparse((scheme, netloc, path, params, query, fragment))`.
///
/// Follows CPython's `urlunparse`/`urlunsplit`: params joined with `;`, then
/// `urlunsplit` assembles scheme/netloc/path/query/fragment.
fn urlunparse(
    scheme: &str,
    netloc: &str,
    path: &str,
    params: &str,
    query: &str,
    fragment: &str,
) -> String {
    // urlunparse: if params, url = path;params
    let mut url = if !params.is_empty() {
        format!("{path};{params}")
    } else {
        path.to_string()
    };

    // urlunsplit logic:
    if !netloc.is_empty() || (!scheme.is_empty() && url.starts_with("//")) {
        if !url.is_empty() && !url.starts_with('/') {
            url = format!("/{url}");
        }
        url = format!("//{netloc}{url}");
    }
    if !scheme.is_empty() {
        url = format!("{scheme}:{url}");
    }
    if !query.is_empty() {
        url = format!("{url}?{query}");
    }
    if !fragment.is_empty() {
        url = format!("{url}#{fragment}");
    }
    url
}

// ---------------------------------------------------------------------------
// Scheme helpers
// ---------------------------------------------------------------------------

/// `_ensure_proxy_scheme(u)` — prepend `http://` when no scheme present.
fn ensure_proxy_scheme(u: &str) -> String {
    if u.contains("://") {
        u.to_string()
    } else {
        format!("http://{u}")
    }
}

// ---------------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------------

/// `_is_socks_proxy` — server URL starts with `socks5://` or `socks5h://` (case-insensitive).
pub fn is_socks_proxy(proxy: &Proxy) -> bool {
    let url = proxy.server_url().to_lowercase();
    url.starts_with("socks5://") || url.starts_with("socks5h://")
}

/// `_has_credentials` — dict: has a non-empty username; string: contains `@`.
pub fn has_credentials(proxy: &Proxy) -> bool {
    match proxy {
        Proxy::Settings(s) => s
            .username
            .as_deref()
            .map(|u| !u.is_empty())
            .unwrap_or(false),
        Proxy::Url(u) => u.contains('@'),
    }
}

/// `binary_supports_http_proxy_inline_auth` — whether the (free) binary accepts
/// `--proxy-server=http://user:pass@host` inline credentials.
///
/// In upstream CloakBrowser this is version-gated against a per-platform floor,
/// with a `license_key` unlock for older builds. This Rust port targets the
/// **capable free binary**: the pinned Chromium builds shipped by
/// `crate::platform` are all above that floor, so inline HTTP-proxy auth is
/// always supported here. Documented assumption: if a build below the floor is
/// ever pinned, this predicate must become version-aware and return `false`
/// there so HTTP credentials fall back to the launcher proxy dict (branch 4).
pub fn binary_supports_http_proxy_inline_auth(
    _browser_version: Option<&str>,
    _license_key: Option<&str>,
) -> bool {
    true
}

/// `_get_flag_value(args, *keys)` — first `k=value` match across `keys`.
pub fn get_flag_value(args: Option<&[String]>, keys: &[&str]) -> Option<String> {
    let args = args?;
    for a in args {
        for k in keys {
            let prefix = format!("{k}=");
            if let Some(rest) = a.strip_prefix(&prefix) {
                return Some(rest.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// URL reconstruction (dict → normalized URL)
// ---------------------------------------------------------------------------

/// `_reconstruct_socks_url(proxy)` — rebuild a SOCKS URL from structured settings.
pub fn reconstruct_socks_url(settings: &crate::ProxySettings) -> String {
    reconstruct_url_from_settings(settings, false)
}

/// `_reconstruct_http_url(proxy)` — like socks but ensures an http scheme first.
pub fn reconstruct_http_url(settings: &crate::ProxySettings) -> String {
    reconstruct_url_from_settings(settings, true)
}

fn reconstruct_url_from_settings(settings: &crate::ProxySettings, ensure_scheme: bool) -> String {
    let server = &settings.server;
    let username = settings.username.as_deref().unwrap_or("");

    // if not username: return server
    if username.is_empty() {
        return server.clone();
    }

    let to_parse = if ensure_scheme {
        ensure_proxy_scheme(server)
    } else {
        server.clone()
    };

    let parsed = match urlparse(&to_parse) {
        ParseOutcome::Ok(p) => p,
        // urlparse on a bad port would raise in Python; callers pass sane servers.
        // Fall back to returning the server unchanged.
        ParseOutcome::BadPort => return server.clone(),
    };

    let enc_user = quote_all(username);
    // enc_pass = quote(password, safe="") if password else None
    let password = settings.password.as_deref();
    let enc_pass: Option<String> = match password {
        Some(p) if !p.is_empty() => Some(quote_all(p)),
        _ => None,
    };

    assemble_proxy_url(
        &parsed.scheme,
        parsed.hostname.as_deref().unwrap_or(""),
        parsed.port,
        &enc_user,
        enc_pass.as_deref(),
        &parsed.path,
        &parsed.params,
        &parsed.query,
        &parsed.fragment,
    )
}

// ---------------------------------------------------------------------------
// String-URL normalization (idempotent)
// ---------------------------------------------------------------------------

/// `_normalize_socks_string_url(url)` — percent-encode credentials in place.
/// Idempotent (decodes then re-encodes). No regex.
pub fn normalize_socks_string_url(url: &str) -> String {
    normalize_string_url(url, false, "SOCKS5")
}

/// `_normalize_http_string_url(url)` — same, but ensures an http scheme first.
pub fn normalize_http_string_url(url: &str) -> String {
    normalize_string_url(url, true, "HTTP")
}

fn normalize_string_url(url: &str, ensure_scheme: bool, kind: &str) -> String {
    // For HTTP: work on the scheme-ensured URL; on failure, return the ensured form.
    let (working, returned_on_bad): (String, String) = if ensure_scheme {
        let w = if url.contains("://") {
            url.to_string()
        } else {
            format!("http://{url}")
        };
        (w.clone(), w)
    } else {
        (url.to_string(), url.to_string())
    };

    let parsed = match urlparse(&working) {
        ParseOutcome::Ok(p) => p,
        ParseOutcome::BadPort => {
            tracing::warn!("Malformed {} proxy URL; leaving unchanged", kind);
            return returned_on_bad;
        }
    };

    // if username None and password None: return url
    if parsed.username.is_none() && parsed.password.is_none() {
        return working;
    }

    // raw_user = parsed.username or ""; enc_user = quote(unquote(raw_user)) if raw_user else ""
    let raw_user = parsed.username.clone().unwrap_or_default();
    let enc_user = if !raw_user.is_empty() {
        quote_all(&unquote(&raw_user))
    } else {
        String::new()
    };

    // if password is not None: raw_pass=password; enc_pass = quote(unquote(raw_pass)) if raw_pass else ""
    // else: raw_pass=None; enc_pass=None
    let (raw_pass, enc_pass): (Option<String>, Option<String>) = match &parsed.password {
        Some(p) => {
            let e = if !p.is_empty() {
                quote_all(&unquote(p))
            } else {
                String::new()
            };
            (Some(p.clone()), Some(e))
        }
        None => (None, None),
    };

    let normalized = assemble_proxy_url(
        &parsed.scheme,
        parsed.hostname.as_deref().unwrap_or(""),
        parsed.port,
        &enc_user,
        enc_pass.as_deref(),
        &parsed.path,
        &parsed.params,
        &parsed.query,
        &parsed.fragment,
    );

    // if enc_user != raw_user or enc_pass != raw_pass: log
    if enc_user != raw_user || enc_pass != raw_pass {
        tracing::info!("Auto URL-encoded {} proxy credentials", kind);
    }

    normalized
}

// ---------------------------------------------------------------------------
// String-URL → ProxySettings (for launcher dict path)
// ---------------------------------------------------------------------------

/// `_parse_proxy_url(proxy)` — split a bare proxy URL into launcher settings.
pub fn parse_proxy_url(proxy: &str) -> crate::ProxySettings {
    // normalized = proxy; if "@" in proxy and "://" not in proxy: normalized = http://proxy
    let normalized = if proxy.contains('@') && !proxy.contains("://") {
        format!("http://{proxy}")
    } else {
        proxy.to_string()
    };

    let parsed = match urlparse(&normalized) {
        ParseOutcome::Ok(p) => p,
        ParseOutcome::BadPort => {
            return crate::ProxySettings {
                server: proxy.to_string(),
                ..Default::default()
            }
        }
    };

    // if not parsed.username: return {"server": proxy}
    let username = match parsed.username.as_deref() {
        Some(u) if !u.is_empty() => u,
        _ => {
            return crate::ProxySettings {
                server: proxy.to_string(),
                ..Default::default()
            }
        }
    };

    // netloc = hostname; if port: netloc += ":port"
    let mut netloc = parsed.hostname.clone().unwrap_or_default();
    if let Some(p) = parsed.port {
        netloc.push_str(&format!(":{p}"));
    }
    // server = urlunparse((scheme, netloc, path, "", "", ""))
    let server = urlunparse(&parsed.scheme, &netloc, &parsed.path, "", "", "");

    let mut settings = crate::ProxySettings {
        server,
        username: Some(unquote(username)),
        ..Default::default()
    };

    // if password: result["password"] = unquote(password)
    if let Some(pass) = parsed.password.as_deref() {
        if !pass.is_empty() {
            settings.password = Some(unquote(pass));
        }
    }

    settings
}

// ---------------------------------------------------------------------------
// resolve_proxy_config
// ---------------------------------------------------------------------------

/// `_resolve_proxy_config` — split proxy into launcher settings vs chrome flags.
pub fn resolve_proxy_config(proxy: Option<&Proxy>) -> ResolvedProxy {
    resolve_proxy_config_ex(proxy, None, None)
}

/// Full form with version/license inputs for the inline-auth predicate.
pub fn resolve_proxy_config_ex(
    proxy: Option<&Proxy>,
    browser_version: Option<&str>,
    license_key: Option<&str>,
) -> ResolvedProxy {
    let proxy = match proxy {
        None => return ResolvedProxy::default(),
        Some(p) => p,
    };

    // 2. SOCKS5
    if is_socks_proxy(proxy) {
        match proxy {
            Proxy::Settings(s) => {
                let url = reconstruct_socks_url(s);
                let mut extra = vec![format!("--proxy-server={url}")];
                if let Some(bypass) = s.bypass.as_deref() {
                    if !bypass.is_empty() {
                        extra.push(format!("--proxy-bypass-list={bypass}"));
                    }
                }
                return ResolvedProxy {
                    proxy_settings: None,
                    extra_args: extra,
                };
            }
            Proxy::Url(u) => {
                return ResolvedProxy {
                    proxy_settings: None,
                    extra_args: vec![format!(
                        "--proxy-server={}",
                        normalize_socks_string_url(u)
                    )],
                };
            }
        }
    }

    // 3. HTTP with creds & capable binary
    if has_credentials(proxy) && binary_supports_http_proxy_inline_auth(browser_version, license_key)
    {
        match proxy {
            Proxy::Settings(s) => {
                let url = reconstruct_http_url(s);
                let mut extra = vec![format!("--proxy-server={url}")];
                if let Some(bypass) = s.bypass.as_deref() {
                    if !bypass.is_empty() {
                        extra.push(format!("--proxy-bypass-list={bypass}"));
                    }
                }
                return ResolvedProxy {
                    proxy_settings: None,
                    extra_args: extra,
                };
            }
            Proxy::Url(u) => {
                return ResolvedProxy {
                    proxy_settings: None,
                    extra_args: vec![format!(
                        "--proxy-server={}",
                        normalize_http_string_url(u)
                    )],
                };
            }
        }
    }

    // 4. HTTP without creds (or incapable binary)
    match proxy {
        Proxy::Settings(s) => ResolvedProxy {
            proxy_settings: Some(s.clone()),
            extra_args: Vec::new(),
        },
        Proxy::Url(u) => ResolvedProxy {
            proxy_settings: Some(parse_proxy_url(u)),
            extra_args: Vec::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// maybe_resolve_geoip
// ---------------------------------------------------------------------------

/// Merge geoip results with explicit timezone/locale. Returns `(tz, locale, exit_ip)`.
pub async fn maybe_resolve_geoip(
    geoip: bool,
    proxy: Option<&Proxy>,
    mut timezone: Option<String>,
    mut locale: Option<String>,
    args: Option<&[String]>,
) -> (Option<String>, Option<String>, Option<String>) {
    // 1. if not geoip: return (timezone, locale, None)
    if !geoip {
        return (timezone, locale, None);
    }

    // 2. flag fallbacks
    if timezone.is_none() {
        timezone = get_flag_value(args, &["--fingerprint-timezone"]);
    }
    if locale.is_none() {
        locale = get_flag_value(args, &["--lang", "--fingerprint-locale"]);
    }

    // 3. proxy_url
    let proxy_url: Option<String> = proxy.map(|p| p.server_url().to_string());

    // 4. both present → only need exit IP
    if timezone.is_some() && locale.is_some() {
        let exit_ip = match proxy_url.as_deref() {
            Some(u) => crate::geoip::resolve_proxy_exit_ip(Some(u)).await,
            None => None,
        };
        return (timezone, locale, exit_ip);
    }

    // 5. otherwise resolve via geoip
    let (geo_tz, geo_locale, exit_ip) =
        crate::geoip::resolve_proxy_geo_with_ip(proxy_url.as_deref()).await;
    if timezone.is_none() {
        timezone = geo_tz;
    }
    if locale.is_none() {
        locale = geo_locale;
    }
    (timezone, locale, exit_ip)
}

// ---------------------------------------------------------------------------
// WebRTC args
// ---------------------------------------------------------------------------

/// `_resolve_webrtc_args` — replace `--fingerprint-webrtc-ip=auto` with the exit IP,
/// or remove it if no proxy / resolution fails. Always copies before mutating.
pub async fn resolve_webrtc_args(
    args: Option<Vec<String>>,
    proxy: Option<&Proxy>,
) -> Option<Vec<String>> {
    // 1. if not args: return args
    let args = args?;
    if args.is_empty() {
        return Some(args);
    }

    // 2. find exact "--fingerprint-webrtc-ip=auto"
    let idx = match args.iter().position(|a| a == "--fingerprint-webrtc-ip=auto") {
        Some(i) => i,
        None => return Some(args),
    };

    // 3. proxy_url
    let proxy_url: Option<String> = proxy.map(|p| p.server_url().to_string());

    // 4. no proxy → remove
    let proxy_url = match proxy_url {
        Some(u) => u,
        None => {
            tracing::warn!("--fingerprint-webrtc-ip=auto requires a proxy; removing");
            let mut out = args.clone();
            out.remove(idx);
            return Some(out);
        }
    };

    // 5/6. resolve exit IP
    // (resolve_proxy_exit_ip never raises in the Rust port; None → "could not" branch.)
    let exit_ip = crate::geoip::resolve_proxy_exit_ip(Some(&proxy_url)).await;

    let mut out = args.clone();
    match exit_ip {
        Some(ip) if !ip.is_empty() => {
            out[idx] = format!("--fingerprint-webrtc-ip={ip}");
        }
        _ => {
            tracing::warn!("Could not resolve exit IP for --fingerprint-webrtc-ip=auto; removing");
            out.remove(idx);
        }
    }
    Some(out)
}

/// `_append_webrtc_exit_ip` — append `--fingerprint-webrtc-ip=<exit_ip>` unless a
/// `--fingerprint-webrtc-ip` flag is already present.
pub fn append_webrtc_exit_ip(
    args: Option<Vec<String>>,
    exit_ip: Option<&str>,
) -> Option<Vec<String>> {
    let exit_ip = match exit_ip {
        Some(ip) if !ip.is_empty() => ip,
        _ => return args,
    };

    let already_present = args
        .as_ref()
        .map(|a| a.iter().any(|x| x.starts_with("--fingerprint-webrtc-ip")))
        .unwrap_or(false);

    if already_present {
        return args;
    }

    let mut out = args.unwrap_or_default();
    out.push(format!("--fingerprint-webrtc-ip={exit_ip}"));
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProxySettings;

    #[test]
    fn quote_all_encodes_non_unreserved() {
        assert_eq!(quote_all("p@ss w0rd"), "p%40ss%20w0rd");
        assert_eq!(quote_all("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(quote_all("a:b/c"), "a%3Ab%2Fc");
    }

    #[test]
    fn normalize_socks_idempotent() {
        let url = "socks5://user:p@ss word@host:1080";
        let once = normalize_socks_string_url(url);
        let twice = normalize_socks_string_url(&once);
        assert_eq!(once, twice, "normalization must be idempotent");
        assert!(once.contains("p%40ss%20word"), "got: {once}");
        assert!(once.contains("host:1080"), "got: {once}");
        assert!(once.starts_with("socks5://"), "got: {once}");
    }

    #[test]
    fn normalize_socks_no_creds_unchanged() {
        let url = "socks5://host:1080";
        assert_eq!(normalize_socks_string_url(url), url);
    }

    #[test]
    fn normalize_socks_already_encoded_idempotent() {
        let url = "socks5://user:p%40ss@host:1080";
        let once = normalize_socks_string_url(url);
        assert_eq!(once, "socks5://user:p%40ss@host:1080");
        assert_eq!(normalize_socks_string_url(&once), once);
    }

    #[test]
    fn empty_password_keeps_colon() {
        // enc_pass == Some("") must preserve the trailing colon (user:@host).
        let got = assemble_proxy_url(
            "socks5",
            "host",
            Some(1080),
            "user",
            Some(""),
            "",
            "",
            "",
            "",
        );
        assert_eq!(got, "socks5://user:@host:1080");
    }

    #[test]
    fn none_password_omits_colon() {
        // enc_pass == None must omit the colon (user@host).
        let got = assemble_proxy_url(
            "socks5", "host", Some(1080), "user", None, "", "", "", "",
        );
        assert_eq!(got, "socks5://user@host:1080");
    }

    #[test]
    fn normalize_socks_empty_password_kept() {
        // A URL with an explicit empty password (user:@host) must keep the colon.
        let url = "socks5://user:@host:1080";
        let got = normalize_socks_string_url(url);
        assert_eq!(got, "socks5://user:@host:1080");
        // And be idempotent.
        assert_eq!(normalize_socks_string_url(&got), got);
    }

    #[test]
    fn reconstruct_socks_url_from_settings() {
        let s = ProxySettings {
            server: "socks5://host:1080".to_string(),
            username: Some("user".to_string()),
            password: Some("p@ss".to_string()),
            ..Default::default()
        };
        assert_eq!(reconstruct_socks_url(&s), "socks5://user:p%40ss@host:1080");
    }

    #[test]
    fn reconstruct_socks_url_no_username_returns_server() {
        let s = ProxySettings {
            server: "socks5://host:1080".to_string(),
            ..Default::default()
        };
        assert_eq!(reconstruct_socks_url(&s), "socks5://host:1080");
    }

    #[test]
    fn reconstruct_http_url_ensures_scheme() {
        let s = ProxySettings {
            server: "host:8080".to_string(),
            username: Some("u".to_string()),
            password: Some("p".to_string()),
            ..Default::default()
        };
        assert_eq!(reconstruct_http_url(&s), "http://u:p@host:8080");
    }

    #[test]
    fn is_socks_proxy_detects() {
        assert!(is_socks_proxy(&Proxy::Url("socks5://h:1".into())));
        assert!(is_socks_proxy(&Proxy::Url("socks5h://h:1".into())));
        assert!(is_socks_proxy(&Proxy::Url("SOCKS5://h:1".into())));
        assert!(!is_socks_proxy(&Proxy::Url("http://h:1".into())));
    }

    #[test]
    fn has_credentials_detects() {
        assert!(has_credentials(&Proxy::Url("http://u:p@h:1".into())));
        assert!(!has_credentials(&Proxy::Url("http://h:1".into())));
        assert!(has_credentials(&Proxy::Settings(ProxySettings {
            server: "http://h:1".into(),
            username: Some("u".into()),
            ..Default::default()
        })));
        assert!(!has_credentials(&Proxy::Settings(ProxySettings {
            server: "http://h:1".into(),
            username: Some("".into()),
            ..Default::default()
        })));
    }

    #[test]
    fn parse_proxy_url_with_creds() {
        let s = parse_proxy_url("http://user:pass@host:8080");
        assert_eq!(s.server, "http://host:8080");
        assert_eq!(s.username.as_deref(), Some("user"));
        assert_eq!(s.password.as_deref(), Some("pass"));
    }

    #[test]
    fn parse_proxy_url_no_creds() {
        let s = parse_proxy_url("http://host:8080");
        assert_eq!(s.server, "http://host:8080");
        assert!(s.username.is_none());
        assert!(s.password.is_none());
    }

    #[test]
    fn parse_proxy_url_bare_with_at_gets_scheme() {
        let s = parse_proxy_url("user:pass@host:8080");
        assert_eq!(s.server, "http://host:8080");
        assert_eq!(s.username.as_deref(), Some("user"));
        assert_eq!(s.password.as_deref(), Some("pass"));
    }

    #[test]
    fn parse_proxy_url_decodes_creds() {
        let s = parse_proxy_url("http://u%40x:p%40ss@host:8080");
        assert_eq!(s.username.as_deref(), Some("u@x"));
        assert_eq!(s.password.as_deref(), Some("p@ss"));
    }

    #[test]
    fn get_flag_value_matches() {
        let args = vec!["--foo=bar".to_string(), "--lang=en-US".to_string()];
        assert_eq!(
            get_flag_value(Some(&args), &["--lang", "--fingerprint-locale"]),
            Some("en-US".to_string())
        );
        assert_eq!(get_flag_value(Some(&args), &["--missing"]), None);
        assert_eq!(get_flag_value(None, &["--lang"]), None);
    }

    #[test]
    fn resolve_proxy_config_socks_string() {
        let r = resolve_proxy_config(Some(&Proxy::Url("socks5://user:p@ss@host:1080".into())));
        assert!(r.proxy_settings.is_none());
        assert_eq!(
            r.extra_args,
            vec!["--proxy-server=socks5://user:p%40ss@host:1080".to_string()]
        );
    }

    #[test]
    fn resolve_proxy_config_http_no_creds_string() {
        let r = resolve_proxy_config(Some(&Proxy::Url("http://host:8080".into())));
        assert!(r.extra_args.is_empty());
        let s = r.proxy_settings.unwrap();
        assert_eq!(s.server, "http://host:8080");
    }

    #[test]
    fn resolve_proxy_config_http_creds_string_inline() {
        let r = resolve_proxy_config(Some(&Proxy::Url("http://user:pass@host:8080".into())));
        assert!(r.proxy_settings.is_none());
        assert_eq!(
            r.extra_args,
            vec!["--proxy-server=http://user:pass@host:8080".to_string()]
        );
    }

    #[test]
    fn resolve_proxy_config_socks_dict_bypass() {
        let r = resolve_proxy_config(Some(&Proxy::Settings(ProxySettings {
            server: "socks5://host:1080".into(),
            username: Some("u".into()),
            password: Some("p".into()),
            bypass: Some("*.local".into()),
        })));
        assert!(r.proxy_settings.is_none());
        assert_eq!(
            r.extra_args,
            vec![
                "--proxy-server=socks5://u:p@host:1080".to_string(),
                "--proxy-bypass-list=*.local".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_proxy_config_none() {
        let r = resolve_proxy_config(None);
        assert!(r.proxy_settings.is_none());
        assert!(r.extra_args.is_empty());
    }

    #[test]
    fn append_webrtc_appends_when_absent() {
        let out = append_webrtc_exit_ip(Some(vec!["--foo".into()]), Some("1.2.3.4"));
        assert_eq!(
            out.unwrap(),
            vec![
                "--foo".to_string(),
                "--fingerprint-webrtc-ip=1.2.3.4".to_string()
            ]
        );
    }

    #[test]
    fn append_webrtc_skips_when_present() {
        let args = vec!["--fingerprint-webrtc-ip=9.9.9.9".to_string()];
        let out = append_webrtc_exit_ip(Some(args.clone()), Some("1.2.3.4"));
        assert_eq!(out.unwrap(), args);
    }

    #[test]
    fn append_webrtc_none_ip_noop() {
        assert_eq!(
            append_webrtc_exit_ip(Some(vec!["--x".into()]), None).unwrap(),
            vec!["--x".to_string()]
        );
        assert!(append_webrtc_exit_ip(None, None).is_none());
    }

    #[test]
    fn append_webrtc_from_none_args() {
        let out = append_webrtc_exit_ip(None, Some("1.2.3.4"));
        assert_eq!(
            out.unwrap(),
            vec!["--fingerprint-webrtc-ip=1.2.3.4".to_string()]
        );
    }

    #[test]
    fn ipv6_host_bracketed() {
        let got = assemble_proxy_url(
            "socks5",
            "::1",
            Some(1080),
            "user",
            Some("pass"),
            "",
            "",
            "",
            "",
        );
        assert_eq!(got, "socks5://user:pass@[::1]:1080");
    }
}
