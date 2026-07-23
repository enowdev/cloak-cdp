# Rust Port Spec — cloakserve (CDP multiplexer) + widevine

## PART 1 — cloakserve (bin/cloakserve, 883 lines, aiohttp + websockets)

### Architecture
Single HTTP+WS server fronting many Chrome processes. Each fingerprint seed → own Chrome (own --user-data-dir, own --remote-debugging-port), clients talk to one public port (default 9222). Server proxies CDP HTTP (/json/version, /json/list) and CDP WebSockets to correct backend by seed.

Flow:
1. Client connect_over_cdp("http://host:9222?fingerprint=<seed>&timezone=...&locale=...").
2. Playwright GETs /json/version (query preserved).
3. handle_json_version parses params, pool.get_or_launch(seed,...): reuse live process for seed_key (first-launch params win) OR spawn new: allocate free port from BASE_CDP_PORT=5100 up, per-seed user-data-dir, Popen Chrome, poll http://127.0.0.1:{port}/json/version until 200 (≤10s).
4. Fetch Chrome's /json/version, REWRITE webSocketDebuggerUrl to point back at multiplexer (preserve GUID, inject seed into path).
5. Client opens rewritten WS → handle_ws_seed (/fingerprint/{seed}/devtools/{path}) or handle_ws_default (/devtools/{path}) → re-resolve process, bidirectionally proxy frames to ws://127.0.0.1:{cdp_port}/devtools/{path}.
6. Refcount (connect/disconnect) drives idle reaping: refcount 0 + idle_timeout>0 → delayed terminate + delete user-data-dir.

Framework: aiohttp (web) server; websockets lib for outbound WS to Chrome; aiohttp.ClientSession outbound HTTP. Rust: hyper/axum + tokio-tungstenite + reqwest.
Server: web.run_app(app, host, port). host = "0.0.0.0" if container (/.dockerenv or /run/.containerenv) else "127.0.0.1".

### Routes (order matters — seed WS before default WS)
```
GET /
GET /json/version, /json/version/
GET /json/list, /json/list/, /json, /json/
GET /fingerprint/{seed}/devtools/{path:.+}   (greedy)
GET /devtools/{path:.+}
```

### Connect URL / params
`http://host:9222?fingerprint=12345&timezone=America/New_York&locale=en-US`
parse_connection_params (parse_qs keep_blank_values=False, values[0]):
- fingerprint→seed, timezone, locale, proxy, geoip (bool: value.lower() in ("true","1","yes"))
- SPECIAL_PARAMS = {fingerprint, proxy, geoip, locale, timezone}. Any OTHER key → extra_args append f"--fingerprint-{key}={val}".

### / (handle_root) status JSON
```json
{"status":"ok","active":<count>,"idle_timeout":<float>,"processes":{"<seed_key>":{"pid","port","seed","connections","idle_cleanup_pending","timezone","locale","proxy"}}}
```
Only processes where poll() is None (alive).

### Spawning Chrome directly
```python
BASE_CHROME_ARGS = ["--no-first-run","--no-default-browser-check","--disable-dev-shm-usage","--disable-extensions","--disable-popup-blocking","--disable-background-networking","--metrics-recording-only","--ignore-gpu-blocklist"]
BASE_CDP_PORT = 5100
```
_allocate_port: start _next_port (init 5100), increment each attempt, socket.bind(("127.0.0.1",port)) test, ≤100 attempts else RuntimeError. Monotonic (never resets down).
Seed→port: NO fixed arithmetic. Dynamic per launch, stored on ChromeProcess. Stable key = seed_key (seed string or "__default__"). _processes: dict[str, ChromeProcess].

Full argv:
```python
full_args = [binary] + BASE_CHROME_ARGS + chrome_args + global_args + [f"--remote-debugging-port={port}", "--remote-debugging-address=127.0.0.1", f"--user-data-dir={user_data_dir}"]
```
- user_data_dir = data_dir/seed_key, makedirs exist_ok.
- Popen(full_args, stdout=DEVNULL) (stderr inherited).
- chrome_args from cloakbrowser.browser.build_args(stealth_args=True, extra_args=fp_extra, timezone, locale, headless). fp_extra=[f"--fingerprint={actual_seed}"]+extra_args, +f"--proxy-server={_normalize_socks_string_url(proxy)}" if proxy, run through _resolve_webrtc_args, +f"--fingerprint-webrtc-ip={exit_ip}" if geoip exit IP and no webrtc flag.
actual_seed vs seed_key: no seed → seed_key="__default__", actual_seed=str(randint(10000,99999)). with seed → both = seed.

_wait_for_cdp: poll http://127.0.0.1:{port}/json/version; 200=success; timeout 10s; per-req 1s; backoff 0.1s doubling cap 1.0s. Fail → kill, wait(5), rm user-data-dir, raise HTTPBadGateway {"error":"Chrome failed to start"}.

Lifecycle:
- One asyncio.Lock per seed_key (_get_lock) guards launch.
- Reuse if seed_key in _processes and poll() is None. Dead → _cleanup_process then relaunch.
- _cleanup_process: cancel idle task, pop, terminate() then wait(5) else kill(); _safe_rmtree(user_data_dir); clear _default, lock, connection count.
- _safe_rmtree: resolve; REFUSE if == data_dir or not relative to data_dir (traversal guard); rmtree(path, True).
- connect(seed_key): cancel pending idle, _connections[seed_key]++. disconnect: --, ≤0 → pop, schedule idle cleanup.
- idle cleanup only if idle_timeout>0. _cleanup_after_idle sleeps idle_timeout, if refcount 0 and process present → _cleanup_process. Tasks in _idle_tasks, cancellable.
- shutdown (on_shutdown): cancel+await idle tasks, _cleanup_process all.

### Security
```python
SAFE_SEED_RE = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
RESERVED_SEEDS = {"__default__"}
TRUSTED_WS_ORIGINS = {"devtools://devtools", "chrome-devtools://devtools"}
```
Seed must match SAFE_SEED_RE and not in RESERVED_SEEDS else HTTPBadRequest {"error":"Invalid fingerprint seed"}. Invalid CLI --fingerprint → sys.exit(1).

_host_port_from_netloc(netloc, default_port) -> (host_lower, port)|None:
- None if comma in netloc.
- urlparse(f"//{netloc.strip()}"); authority = parsed.netloc.rsplit("@",1)[-1].
- None if: no hostname; username/password present; authority ends ":"; any path/params/query/fragment nonempty.
- else (parsed.hostname.lower(), parsed.port if not None else default_port). None on ValueError.

_is_loopback_host(hostname): strip [], strip trailing ".", lower. True if "localhost"; else ip_address(hostname).is_loopback; False on error.

_origin_is_allowed(origin, host, request_scheme="http"):
- origin None → allowed (non-browser CDP clients omit Origin).
- empty or "null" (ci) → rejected.
- in TRUSTED_WS_ORIGINS → allowed.
- urlparse; scheme must be http/https; no path/params/query/fragment.
- origin_default_port = 443 if https else 80.
- request_scheme: before comma, strip, lower; request_default_port = 443 if in ("https","wss") else 80.
- origin_host=_host_port_from_netloc(origin.netloc, origin_default_port); request_host=_host_port_from_netloc(host or "", request_default_port); either None → rejected.
- request host must be loopback else rejected.
- allowed iff origin_host == request_host (host AND port equal).

_reject_untrusted_origin(request): Origin, Host, scheme from X-Forwarded-Proto (fallback request.scheme). allowed → None; else warn + 403 "Forbidden: untrusted WebSocket origin\n". Called at top of both WS handlers.

### CDP WS proxying + URL rewriting
_ws_scheme(request): X-Forwarded-Proto (fallback request.scheme), before comma/strip/lower; "wss" if "https" else "ws".
_external_host(request): X-Forwarded-Host (first comma-seg, strip) if nonempty; else Host header; else f"localhost:{port}".

/json/version rewrite:
```python
host=_external_host(request); seed_key=params["seed"]
ws_path = f"fingerprint/{seed_key}/devtools/browser" if seed_key else "devtools/browser"
orig_ws=data.get("webSocketDebuggerUrl","")
guid = orig_ws.rsplit("/",1)[-1] if "/devtools/" in orig_ws else ""
scheme=_ws_scheme(request)
data["webSocketDebuggerUrl"]=f"{scheme}://{host}/{ws_path}/{guid}"
```
/json/list rewrite: each entry with webSocketDebuggerUrl → split on "/devtools/" take tail (ws_tail); rebuild f"{scheme}://{host}/fingerprint/{seed_key}/devtools/{ws_tail}" (seeded) or f"{scheme}://{host}/devtools/{ws_tail}" (default).
Both HTTP handlers call get_or_launch first (spawns Chrome), then aiohttp GET http://127.0.0.1:{cdp_port}/json/{version|list} timeout 5s; fail → 502 {"error":"CDP endpoint unreachable"}.

WS proxy targets:
- handle_ws_seed: seed=match["seed"], path=match["path"], cp=get_or_launch(seed), WebSocketResponse, connect(seed), target ws://127.0.0.1:{cdp_port}/devtools/{path}, proxy, finally disconnect(seed).
- handle_ws_default: seed=None, seed_key="__default__", target ws://127.0.0.1:{cdp_port}/devtools/{path}.
proxy_cdp_websocket(client_ws, target_url, label): websockets.connect(target_url, max_size=None, ping_interval=None, ping_timeout=None). client_to_cdp: TEXT+BINARY → cdp_ws.send(msg.data); CLOSE → break. cdp_to_client: str→send_str, else send_bytes. asyncio.wait FIRST_COMPLETED, cancel pending.

### CLI (parse_cli_args)
Defaults: {port:9222, headless:True, data_dir:None, default_seed:None, default_locale:None, default_timezone:None, idle_timeout:_default_idle_timeout()}.
--port=int, --data-dir=path, --idle-timeout=val (consumed). --headless=false/False → headless=False AND appended to passthrough. Consumed prefixes: --port=, --data-dir=, --idle-timeout=, --remote-debugging-port=, --remote-debugging-address=. --fingerprint-locale=, --fingerprint-timezone=, --fingerprint= → defaults (not passthrough). Else → passthrough (global_args).
_parse_idle_timeout(v): strip; {"0","false","off","none","disabled"} → 0.0; else float, >=0 else ValueError.
_default_idle_timeout: env CLOAKSERVE_IDLE_TIMEOUT; unset → 0.0. <=0 disables reaping.
_default_data_dir: container → /tmp/cloakserve; else ~/.cloakbrowser/cloakserve. ChromePool default data_dir="/tmp/cloakserve".
main: binary=ensure_binary(); validate default_seed; ChromePool; routes; web.run_app.

### Rust notes
axum (or hyper+tokio-tungstenite). Outbound CDP WS: tokio-tungstenite no size cap, pings disabled. Preserve route order + greedy {path:.+} (/devtools/*path). Per-seed async mutex (tokio Mutex in DashMap). Monotonic port counter from 5100, bind-test 127.0.0.1. Regexes verbatim. Compact JSON.

## PART 2 — Widevine

### cloakbrowser/widevine.py — hint-file seeding (does NOT download). Linux-only.
Pre-seeds Chromium Widevine CDM "hint file" into persistent profile so sideloaded CDM works on first launch (component updater disabled by --disable-component-update). Never downloads — only writes hint at already-present CDM.
_HINT_FILENAME = "latest-component-updated-widevine-cdm".
_seeding_disabled(): env CLOAKBROWSER_WIDEVINE in ("0","false","off","no") (strip/lower).
resolve_widevine_cdm_dir(binary_path):
1. env CLOAKBROWSER_WIDEVINE_CDM — if set, used exclusively. Empty/ws → None. Valid iff <dir>/manifest.json is file; Path(custom).resolve() else None.
2. <dir of chrome binary>/WidevineCdm.
3. <cache dir>/WidevineCdm (~/.cloakbrowser/WidevineCdm).
   2&3: dir counts only if contains manifest.json; first hit, .resolve()d.
seed_widevine_hint(user_data_dir, binary_path):
- no-op if not Linux, _seeding_disabled(), or user_data_dir empty.
- resolve CDM dir; None → log (warn if env set-but-invalid, debug else); return.
- write into <user_data_dir>/WidevineCdm/latest-component-updated-widevine-cdm.
- content = json.dumps({"Path": str(cdm_dir)}, separators=(",",":"), ensure_ascii=False) → compact {"Path":"<abs dir>"}, UTF-8, no spaces.
- idempotent: if exists and content == target, return.
- try/except — NEVER raises; warn on error.
Rust: serde_json::to_string on struct {Path: String} = exact compact form.

### bin/fetch-widevine.py — CDM fetcher. Linux x86-64 only.
Downloads Widevine CDM from Google component-update server → layout where resolve path 3 expects.
Output: <out>/manifest.json, <out>/_platform_specific/linux_<arch>/libwidevinecdm.so.
```python
APP_ID = "oimompecagnajdejgnnjijobebaeigek"
UPDATE_URL = "https://update.googleapis.com/service/update2/json"
INSTALLED_VERSION = "1.4.9.1088"
XSSI_PREFIX = ")]}'"
```
_arch: x86_64/amd64/x64 → "x64". arm64 → SystemExit "not published for linux arm64". else SystemExit.
_default_out: (CLOAKBROWSER_CACHE_DIR or ~/.cloakbrowser)/WidevineCdm.
CLI: --out (default _default_out), --force, --quiet. Print abs out dir on success. Cache hit: <out>/manifest.json exists and not --force → "already present", print, return 0. SystemExit if not Linux.
_resolve_crx POST JSON to UPDATE_URL:
```python
payload = {"request": {"@os":"", "@updater":"", "acceptformat":"crx3,download,puff,run,xz,zucc", "apps":[{"appid":APP_ID, "installsource":"ondemand", "updatecheck":{}, "version":INSTALLED_VERSION}], "dedup":"cr", "ismachine":False, "arch":arch, "os":{"arch":arch, "platform":"linux"}, "protocol":"4.0", "updaterversion":"142.0.7444.175"}}
```
POST urllib headers {"User-Agent":"Mozilla/5.0","Content-Type":"application/json"} timeout 30s. Strip XSSI prefix )]}' before JSON parse. Navigate resp["response"]["apps"][0]["updatecheck"]. status present and !="ok" → SystemExit. version=uc.get("nextversion","?"). Iterate uc["pipelines"][*]["operations"][*]; first op with urls (filter url starts "https") → (version, urls[0], op["out"]["sha256"]). None → SystemExit.
_download: urlopen(url, timeout=120), read blob. If server sha256, verify hashlib.sha256(blob).hexdigest() (ci) else SystemExit.
Integrity (_verify_crx3): require at least one positive: sig_ok+sha both, sig_ok only, sha only. neither → SystemExit.
_verify_crx3(blob):
- False if cryptography not importable (fall back TLS+SHA256).
- CRX3: magic b"Cr24" [0:4], version <I [4:8]==3, header_len <I [8:12], header=[12:12+len], archive=rest.
- Parse header protobuf (_parse_pb: minimal reader wire types 0/1/2/5; field→list length-delimited for wire 2).
- signed_header=fields[10000][0]. signed payload = b"CRX3 SignedData\x00" + struct.pack("<I", len(signed_header)) + signed_header + archive. declared_id=_parse_pb(signed_header)[1][0].
- For proof in fields[2]: pub_der=p[1][0], sig=p[2][0]. app id via _crx_appid: SHA256(pubkey_der)[:16], each byte high/low nibble → chr(0x61+nibble) (a-p). Skip if appid != APP_ID. If declared_id set and != digest16 → SystemExit. Verify RSASSA-PKCS1-v1_5/SHA256 (PKCS1v15, SHA256) over payload; InvalidSignature → SystemExit. Success → True. No matching proof → SystemExit.
_extract: so_member=f"_platform_specific/linux_{arch}/libwidevinecdm.so". zipfile.ZipFile(io.BytesIO(crx_bytes)) (zip central dir from end, CRX header ignored). Require manifest.json and so_member else SystemExit. Extract both to mkdtemp(prefix=".widevine.tmp.", dir=parent of out), chmod so 0o644, then swap: if out exists rmtree then rename(tmp, out). Exception → rmtree tmp; re-raise.
Guard: SystemExit re-raised; else print stderr, exit(1).
Rust: little-endian struct reads, minimal protobuf length-delimited parser, RSA PKCS1v15/SHA256 (rsa + sha2), a-p app-id nibble encoding, strip XSSI, zip crate opens CRX directly.
