# Rust Port Spec: geoip + proxy/webrtc helpers

## PART A — cloakbrowser/geoip.py

### Constants
```
GEOIP_DB_URL = "https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-City.mmdb"
GEOIP_DB_FILENAME = "GeoLite2-City.mmdb"
GEOIP_UPDATE_INTERVAL = 30 * 86_400          # 2_592_000 s (30 days)
DEFAULT_GEOIP_TIMEOUT_SECONDS = 5.0
GEOIP_TIMEOUT_ENV = "CLOAKBROWSER_GEOIP_TIMEOUT_SECONDS"
_GEOIP_DOWNLOAD_LOCK = threading.Lock()      # process mutex serializing DB downloads
```
External endpoints:
- GeoIP DB: GET https://github.com/P3TERX/GeoLite.mmdb/raw/download/GeoLite2-City.mmdb (follow redirects, 300s timeout). ~70MB mmdb.
- IP echo (_IP_ECHO_URLS, in order): https://api.ipify.org, https://checkip.amazonaws.com, https://ifconfig.me/ip. GET, no headers/params, body=plaintext IP (.strip()). Through proxy if given.

### COUNTRY_LOCALE_MAP (ISO alpha-2 → BCP47, verbatim)
```
US:en-US GB:en-GB AU:en-AU CA:en-CA NZ:en-NZ IE:en-IE ZA:en-ZA SG:en-SG
DE:de-DE AT:de-AT CH:de-CH FR:fr-FR BE:fr-BE ES:es-ES MX:es-MX AR:es-AR
CO:es-CO CL:es-CL BR:pt-BR PT:pt-PT IT:it-IT NL:nl-NL JP:ja-JP KR:ko-KR
CN:zh-CN TW:zh-TW HK:zh-HK RU:ru-RU UA:uk-UA PL:pl-PL CZ:cs-CZ RO:ro-RO
IL:he-IL TR:tr-TR SA:ar-SA AE:ar-AE EG:ar-EG IN:hi-IN ID:id-ID PH:en-PH
TH:th-TH VN:vi-VN MY:ms-MY SE:sv-SE NO:nb-NO DK:da-DK FI:fi-FI GR:el-GR
HU:hu-HU BG:bg-BG SI:sl-SI SK:sk-SK HR:hr-HR RS:sr-RS LT:lt-LT LV:lv-LV
EE:et-EE IS:is-IS LU:fr-LU MT:en-MT CY:el-CY MD:ro-MD BY:ru-BY GE:ka-GE
AL:sq-AL MK:mk-MK BA:bs-BA PE:es-PE VE:es-VE EC:es-EC UY:es-UY CR:es-CR
DO:es-DO GT:es-GT BO:es-BO PY:es-PY PK:en-PK BD:bn-BD LK:si-LK KZ:ru-KZ
IR:fa-IR IQ:ar-IQ JO:ar-JO LB:ar-LB KW:ar-KW QA:ar-QA OM:ar-OM BH:ar-BH
NG:en-NG KE:en-KE MA:fr-MA DZ:ar-DZ TN:ar-TN GH:en-GH AM:hy-AM AZ:az-AZ
UZ:uz-UZ KG:ky-KG TJ:tg-TJ TM:tk-TM ME:sr-ME XK:sq-XK LI:de-LI MC:fr-MC
AD:ca-AD MM:my-MM KH:km-KH LA:lo-LA MN:mn-MN BN:ms-BN MO:zh-MO YE:ar-YE
SY:ar-SY PS:ar-PS LY:ar-LY ET:am-ET TZ:sw-TZ UG:en-UG SN:fr-SN CI:fr-CI
CM:fr-CM AO:pt-AO MZ:pt-MZ ZM:en-ZM ZW:en-ZW HN:es-HN NI:es-NI SV:es-SV
PA:es-PA JM:en-JM TT:en-TT PR:es-PR
```
No regexes in this file.

### resolve_proxy_geo(proxy_url) -> (tz, locale)
Calls resolve_proxy_geo_with_ip, discards IP.

### resolve_proxy_geo_with_ip(proxy_url) -> (tz, locale, exit_ip). Never raises (except geoip2 import; N/A in Rust).
1. (geoip2 import — Rust: mmdb reader is hard dep.)
2. db_path = _ensure_geoip_db()  [BEFORE deadline — 70MB download not bounded by resolution timeout]. May be None.
3. timeout = _get_geoip_timeout_seconds(); deadline = _deadline_from_timeout(timeout).
4. ip = _resolve_exit_ip(proxy_url, timeout=_remaining_seconds(deadline)).
5. if ip is None and proxy_url and not _deadline_expired: ip = _resolve_proxy_ip(proxy_url).
6. if ip is None or _deadline_expired: warn if expired; return (None,None,None).
7. if db_path is None: return (None,None,ip)  [exit IP still returned for WebRTC].
8. Open mmdb reader, city lookup ip: timezone=resp.location.time_zone; country=resp.country.iso_code; locale=COUNTRY_LOCALE_MAP.get(country); return (timezone, locale, ip).
9. On exception in 8: warn; return (None,None,ip).

### _resolve_proxy_ip(proxy_url) -> ip|None. Never raises.
1. hostname = urlparse(proxy_url).hostname; falsy → None.
2. inet_pton(AF_INET) parses → return hostname (IPv4 literal).
3. inet_pton(AF_INET6) parses → return hostname (IPv6 literal).
4. getaddrinfo(hostname, None, AF_UNSPEC, SOCK_STREAM); non-empty → results[0][4][0]; return ip.
5. exception → warn; None.

### _is_private_ip(ip) -> bool: ipaddress.ip_address(ip).is_private; ValueError → False. (Defined, unused — port for completeness.)

### _get_geoip_timeout_seconds() -> float
raw=env CLOAKBROWSER_GEOIP_TIMEOUT_SECONDS. Empty → 5.0. float(raw); ValueError → NaN. not isfinite → warn, 5.0. Return max(timeout, 0.0).

### Deadlines (monotonic; Rust Instant)
- _deadline_from_timeout(t): t<=0 → None; else monotonic()+t.
- _remaining_seconds(d): None→None; else max(d-monotonic(), 0.0).
- _deadline_expired(d): d is not None and monotonic()>=d.

### resolve_proxy_exit_ip(proxy_url) -> ip|None
timeout=_get_geoip_timeout_seconds(); deadline=_deadline_from_timeout(timeout); ip=_resolve_exit_ip(proxy_url, timeout=timeout); if ip None and expired: warn; return ip.

### _resolve_exit_ip(proxy_url, timeout=None) -> ip|None
deadline=_deadline_from_timeout(timeout or 0). For url in _IP_ECHO_URLS:
- remaining=_remaining_seconds(deadline); if remaining is not None and <=0: return None.
- request_timeout = min(10.0, remaining) if remaining is not None else 10.0.
- resp=httpx.get(url, proxy=proxy_url or None, timeout=request_timeout); raise_for_status.
- ip=resp.text.strip(); validate ipaddress.ip_address(ip); return ip.
- except UnsupportedProtocol (SOCKS w/o socks transport): warn "SOCKS5 proxy requires socksio"; return None IMMEDIATELY (don't try others).
- except Exception: continue.
After loop: warn "Failed to discover exit IP through proxy"; None.
Rust: reqwest per-request proxy; SOCKS-unsupported branch = SOCKS feature not compiled.

### _get_geoip_dir(): get_cache_dir() / "geoip"  (~/.cloakbrowser/geoip/)

### _ensure_geoip_db() -> Path|None
1. db_path = _get_geoip_dir()/"GeoLite2-City.mmdb".
2. exists → _maybe_trigger_update(db_path); return db_path.
3. acquire _GEOIP_DOWNLOAD_LOCK (blocking): if exists → return; try _download_geoip_db; return; except → warn; None.

### _download_geoip_db(dest) atomic streaming
1. dest.parent.mkdir; log "~70MB".
2. tmp = mkstemp(dir=dest.parent, suffix=".tmp") — SAME dir for atomic replace.
3. httpx.stream GET GEOIP_DB_URL follow_redirects timeout=300s; raise_for_status; chunk_size=65536 (64KiB); progress every ≥10%.
4. os.replace(tmp, dest). log ready.
5. exception → tmp.unlink(missing_ok); raise.
Rust: NamedTempFile::new_in(dest.parent), then rename/persist.

### _maybe_trigger_update(db_path) background if >30 days
1. age = time()-mtime; if age < GEOIP_UPDATE_INTERVAL: return. OSError → return.
2. daemon thread _bg: if not LOCK.acquire(blocking=False): return; try _download_geoip_db; except debug; finally release.
Rust: std::thread::spawn detached; try_lock; mtime via metadata().modified().

## PART B — browser.py proxy/webrtc

Imports: urlparse, urlunparse, quote, unquote. No regexes.

### ProxySettings (TypedDict)
Required: server: str. Optional: bypass, username, password: str.
Rust: struct ProxySettings { server: String, bypass/username/password: Option<String> }. proxy param = str|ProxySettings|None → enum Proxy { Url(String), Settings(ProxySettings) } wrapped Option.

### maybe_resolve_geoip(geoip, proxy, timezone, locale, args=None) -> (tz, locale, exit_ip)
1. if not geoip: return (timezone, locale, None).
2. if timezone None: timezone=_get_flag_value(args, "--fingerprint-timezone"). if locale None: locale=_get_flag_value(args, "--lang", "--fingerprint-locale").
3. proxy_url = _extract_proxy_url(proxy) if proxy else None.
4. if timezone and locale both non-None: exit_ip = resolve_proxy_exit_ip(proxy_url) if proxy_url else None; return (timezone, locale, exit_ip).
5. else: geo_tz, geo_locale, exit_ip = resolve_proxy_geo_with_ip(proxy_url); if timezone None: timezone=geo_tz; if locale None: locale=geo_locale; return (timezone, locale, exit_ip).

_get_flag_value(args, *keys): for a in args: for k in keys: if a.startswith(k+"="): return a.split("=",1)[1]. None.

### _resolve_webrtc_args(args, proxy) -> list|None
1. if not args: return args.
2. find idx of exact "--fingerprint-webrtc-ip=auto". none → return args.
3. proxy_url=_extract_proxy_url(proxy).
4. if not proxy_url: warn "requires a proxy; removing"; copy, del args[idx]; return.
5. try exit_ip=resolve_proxy_exit_ip(proxy_url); except: warn "Failed...removing"; copy, del; return.
6. if exit_ip: copy, args[idx]=f"--fingerprint-webrtc-ip={exit_ip}". else: warn "Could not...removing"; copy, del. return.
Always copies before mutating.

### _append_webrtc_exit_ip(args, exit_ip) -> list|None
if exit_ip and not (args and any(a.startswith("--fingerprint-webrtc-ip") for a in args)): args=list(args or []); args.append(f"--fingerprint-webrtc-ip={exit_ip}"). return args.
Ordering: _resolve_webrtc_args first (=auto→=ip), then _append.

### _resolve_proxy_config(proxy, browser_version=None, license_key=None) -> (proxy_kwargs dict, extra_chrome_args list). At most one non-empty.
1. proxy None → ({}, []).
2. SOCKS5 (_is_socks_proxy): dict → url=_reconstruct_socks_url(proxy); extra=[f"--proxy-server={url}"]; if bypass: append f"--proxy-bypass-list={bypass}"; return ({}, extra). string → ({}, [f"--proxy-server={_normalize_socks_string_url(proxy)}"]).
3. HTTP w/ creds & capable binary (_has_credentials and binary_supports_http_proxy_inline_auth): dict → _reconstruct_http_url; string → _normalize_http_string_url. Same shape as 2.
4. HTTP w/o creds (or incapable): dict → ({"proxy": proxy}, []); string → ({"proxy": _parse_proxy_url(proxy)}, []).

Helpers:
- _is_socks_proxy: url = proxy["server"] if dict else proxy; url.lower().startswith(("socks5://","socks5h://")).
- _has_credentials: dict → bool(username); string → "@" in proxy.
- _ensure_proxy_scheme(u): u if "://" in u else f"http://{u}".
- _reconstruct_socks_url(proxy): server/username/password from dict. if not username: return server. parsed=urlparse(server). enc_user=quote(username, safe=""). enc_pass=quote(password, safe="") if password else None. _assemble_proxy_url(scheme, hostname, port, enc_user, enc_pass, path).
- _reconstruct_http_url: same but parsed=urlparse(_ensure_proxy_scheme(server)).
- _assemble_proxy_url(scheme, host, port, enc_user, enc_pass, path="", params="", query="", fragment=""):
    if ":" in host: host=f"[{host}]"  # IPv6
    if enc_pass is not None: userinfo=f"{enc_user}:{enc_pass}@"
    elif enc_user: userinfo=f"{enc_user}@"
    else: userinfo=""
    netloc=f"{userinfo}{host}"; if port is not None: netloc+=f":{port}"
    return urlunparse((scheme, netloc, path, params, query, fragment))
    # enc_pass is None → no colon; enc_pass=="" → colon preserved (user:@host)
- _parse_proxy_url(proxy) -> dict:
    normalized=proxy; if "@" in proxy and "://" not in proxy: normalized=f"http://{proxy}".
    parsed=urlparse(normalized); if not parsed.username: return {"server": proxy}.
    netloc=hostname; if port: netloc+=f":{port}". server=urlunparse((scheme, netloc, path, "","","")).
    result={"server":server, "username":unquote(parsed.username)}; if password: result["password"]=unquote(password). return.
- binary_supports_http_proxy_inline_auth: version-gated; model as injectable predicate (returns resolved version >= platform floor). Below floor → HTTP creds fall back to proxy dict.

### _normalize_socks_string_url(url) -> str. Idempotent. No regex.
1. try parsed=urlparse(url); _=parsed.port. except ValueError: warn "Malformed SOCKS5...unchanged"; return url.
2. if username None and password None: return url.
3. raw_user=parsed.username or ""; enc_user=quote(unquote(raw_user), safe="") if raw_user else "".
4. if password is not None: raw_pass=password; enc_pass=quote(unquote(raw_pass), safe="") if raw_pass else "". else raw_pass=None; enc_pass=None.
5. normalized=_assemble_proxy_url(scheme, hostname, port, enc_user, enc_pass, path, params, query, fragment).
6. if enc_user != raw_user or enc_pass != raw_pass: info "Auto URL-encoded SOCKS5 proxy credentials...".
7. return normalized.

_normalize_http_string_url: identical except normalized=url if "://" in url else f"http://{url}" at top; messages say "HTTP".

## Rust notes
- urlparse semantics: None vs "" for absent-vs-empty password (colon distinction), .hostname lowercasing, .port raising on invalid. url crate differs — may need thin custom parser to preserve enc_pass None (no colon) vs "" (colon).
- quote(s, safe="") = encode all non-unreserved (unreserved = A-Za-z0-9-_.~). Use percent-encoding NON_ALPHANUMERIC + allow -_.~.
- Monotonic time = Instant; wall-clock only for DB mtime.
- One process-global mutex guards DB downloads; blocking for first-use, try_lock for background.
- MaxMind reader: maxminddb crate.
