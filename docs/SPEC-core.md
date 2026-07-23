# Rust Port Spec — core: args + launch (config.py + browser.py)

Chromiumoxide is the CDP client. launch() = ensure_binary → build_args → spawn custom Chromium + connect CDP → return Browser handle. No Playwright.

## Stealth args (config.py get_default_stealth_args)
```python
seed = random.randint(10000, 99999)
system = platform.system()
base = ["--no-sandbox", f"--fingerprint={seed}"]
if Darwin:  return base + ["--fingerprint-platform=macos"]   # native mac GPU/UA
else (Linux/Windows): return base + ["--fingerprint-platform=windows"]
```
DEFAULT_VIEWPORT = {width:1920, height:947}  (HEADLESS only; headed uses no_viewport = real window).
IGNORE_DEFAULT_ARGS = ["--enable-automation", "--enable-unsafe-swiftshader"]   # MUST be removed from launch

## build_args(stealth_args, extra_args, timezone=None, locale=None, headless=True, extension_paths=None, start_maximized=False) -> list[str]
Dedup by flag key (everything before '='). Priority: stealth defaults < user args < dedicated params (timezone/locale).
```python
seen = {}   # key → full arg
if stealth_args:
    for arg in get_default_stealth_args(): seen[arg.split("=",1)[0]] = arg
# GPU blocklist bypass: headed (all platforms) OR Windows (all modes)
if not headless or system=="Windows": seen["--ignore-gpu-blocklist"]="--ignore-gpu-blocklist"
if extra_args:
    for arg in extra_args: seen[arg.split("=",1)[0]] = arg   # user overrides
if timezone: seen["--fingerprint-timezone"]=f"--fingerprint-timezone={timezone}"
if locale:
    for key in ("--lang","--fingerprint-locale"): seen[key]=f"{key}={locale}"
if extension_paths:
    abs_paths=[abspath(p) for p in extension_paths]; ext_val=",".join(abs_paths)
    seen["--load-extension"]=f"--load-extension={ext_val}"
    seen["--disable-extensions-except"]=f"--disable-extensions-except={ext_val}"
if start_maximized and not any(k in seen for k in ("--start-maximized","--window-size","--window-position")):
    seen["--start-maximized"]="--start-maximized"
return list(seen.values())
```

## launch(...) flow (sync, browser.py:167)
```python
binary_path = ensure_binary(license_key, browser_version)
timezone, locale, exit_ip = maybe_resolve_geoip(geoip, proxy, timezone, locale, args)
proxy_kwargs, proxy_extra_args = _resolve_proxy_config(proxy, browser_version, license_key)
args = _resolve_webrtc_args(args, proxy)
args = _append_webrtc_exit_ip(args, exit_ip)
chrome_args = build_args(stealth_args, (args or [])+proxy_extra_args, timezone, locale, headless, extension_paths, start_maximized=binary_supports_maximized_window(...) and not _suppress_maximize)
launch_env = build_launch_env(license_key, user_env=env)   # free (no key) → None = inherit parent env
# Playwright: pw.chromium.launch(executable_path=binary_path, headless, args=chrome_args, ignore_default_args=IGNORE_DEFAULT_ARGS, env, **proxy_kwargs)
```
launch() signature params: headless=True, proxy=None, timezone=None, locale=None, geoip=False, args=None, stealth_args=True, extension_paths=None, license_key=None, browser_version=None, env=None, **kwargs.
Variants: launch, launch_async, launch_context, launch_context_async, launch_persistent_context, launch_persistent_context_async.

## Rust/chromiumoxide launch
1. ensure_binary → path.
2. build_args (stealth + geoip tz/locale + proxy args + webrtc).
3. BrowserConfig: .executable(path), args manual, headless.
4. CRITICAL: chromiumoxide injects its own default args including --enable-automation. Must strip --enable-automation and --enable-unsafe-swiftshader from the final launch (mirror ignore_default_args). Use chromiumoxide's arg config to avoid/remove them, or build a fully manual arg set with no default automation flags. Verify navigator.webdriver === false in a test.
5. Return Browser + Handler (spawn handler task).

## config version-gating helpers
- binary_supports_maximized_window(license_key, browser_version): = binary_supports_headless_no_viewport(...). Version floor per platform. For free port, model as predicate (resolved version >= floor). Below floor: don't add --start-maximized.
- build_launch_env free path (no license_key) → None (inherit parent env). Trivial for Rust: no env override needed unless user passes env.

## Public Rust API target
```rust
pub async fn launch(opts: LaunchOptions) -> Result<(Browser, Handler)>;
pub struct LaunchOptions {
    pub headless: bool,            // default true
    pub proxy: Option<Proxy>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub geoip: bool,              // default false
    pub extra_args: Vec<String>,
    pub stealth_args: bool,       // default true
    pub extension_paths: Vec<String>,
    pub browser_version: Option<String>,
    pub start_maximized: bool,
}
```
maybe_resolve_geoip / _resolve_proxy_config / _resolve_webrtc_args / _append_webrtc_exit_ip live in geoip+proxy module. build_args + stealth args live in core args module.
