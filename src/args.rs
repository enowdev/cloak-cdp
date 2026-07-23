//! Stealth argument construction. See docs/SPEC-core.md.

use std::path::Path;

use rand::Rng;

/// `--enable-automation` and `--enable-unsafe-swiftshader` must be removed from
/// the final launch (mirrors Python `IGNORE_DEFAULT_ARGS`).
pub const IGNORE_DEFAULT_ARGS: &[&str] = &["--enable-automation", "--enable-unsafe-swiftshader"];

/// Default headless viewport (headed launches track the real window).
pub const DEFAULT_VIEWPORT: (u32, u32) = (1920, 947);

/// Insert or overwrite `key`'s slot with `value`, preserving insertion order.
fn put(seen: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(slot) = seen.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_string();
    } else {
        seen.push((key.to_string(), value.to_string()));
    }
}

/// Return the flag key of a Chrome arg: everything before the first `=`.
fn flag_key(arg: &str) -> &str {
    match arg.split_once('=') {
        Some((key, _)) => key,
        None => arg,
    }
}

/// Build the default stealth args with a fresh random fingerprint seed.
///
/// Mirrors `config.py get_default_stealth_args`: a random fingerprint seed in
/// `10000..=99999`, `--no-sandbox`, and a `--fingerprint-platform` selected from
/// the host OS (`macos` on macOS, `windows` everywhere else).
pub fn get_default_stealth_args() -> Vec<String> {
    let seed: u32 = rand::thread_rng().gen_range(10000..=99999);
    let mut args = vec!["--no-sandbox".to_string(), format!("--fingerprint={seed}")];
    if cfg!(target_os = "macos") {
        args.push("--fingerprint-platform=macos".to_string());
    } else {
        args.push("--fingerprint-platform=windows".to_string());
    }
    args
}

/// Combine stealth args with user args + locale/timezone/extension flags.
///
/// Dedup by flag key (everything before `=`). Priority, low to high:
/// stealth defaults < user args < dedicated params (timezone/locale). Later
/// insertions with the same key overwrite earlier ones while preserving the
/// original slot order.
#[allow(clippy::too_many_arguments)]
pub fn build_args(
    stealth_args: bool,
    extra_args: Option<Vec<String>>,
    timezone: Option<&str>,
    locale: Option<&str>,
    headless: bool,
    extension_paths: Option<Vec<String>>,
    start_maximized: bool,
) -> Vec<String> {
    // Insertion-ordered map: key -> full arg. IndexMap isn't a dep, so emulate
    // insertion-order-preserving overwrite with a Vec of (key, value).
    let mut seen: Vec<(String, String)> = Vec::new();

    if stealth_args {
        for arg in get_default_stealth_args() {
            put(&mut seen, flag_key(&arg), &arg);
        }
    }

    // GPU blocklist bypass: headed (all platforms) OR Windows (all modes).
    if !headless || cfg!(target_os = "windows") {
        put(&mut seen, "--ignore-gpu-blocklist", "--ignore-gpu-blocklist");
    }

    if let Some(extra) = extra_args {
        for arg in extra {
            put(&mut seen, flag_key(&arg), &arg);
        }
    }

    if let Some(tz) = timezone {
        put(
            &mut seen,
            "--fingerprint-timezone",
            &format!("--fingerprint-timezone={tz}"),
        );
    }

    if let Some(loc) = locale {
        for key in ["--lang", "--fingerprint-locale"] {
            put(&mut seen, key, &format!("{key}={loc}"));
        }
    }

    if let Some(paths) = extension_paths {
        if !paths.is_empty() {
            let abs_paths: Vec<String> = paths
                .iter()
                .map(|p| {
                    std::fs::canonicalize(p)
                        .map(|abs| abs.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| absolutize(p))
                })
                .collect();
            let ext_val = abs_paths.join(",");
            put(
                &mut seen,
                "--load-extension",
                &format!("--load-extension={ext_val}"),
            );
            put(
                &mut seen,
                "--disable-extensions-except",
                &format!("--disable-extensions-except={ext_val}"),
            );
        }
    }

    if start_maximized {
        let has_window_flag = ["--start-maximized", "--window-size", "--window-position"]
            .iter()
            .any(|k| seen.iter().any(|(seen_key, _)| seen_key == k));
        if !has_window_flag {
            put(&mut seen, "--start-maximized", "--start-maximized");
        }
    }

    seen.into_iter().map(|(_, v)| v).collect()
}

/// Best-effort absolute path for a non-existent path (canonicalize requires the
/// path to exist). Mirrors Python `os.path.abspath`, which does not.
fn absolutize(p: &str) -> String {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_args_shape() {
        let args = get_default_stealth_args();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "--no-sandbox");
        assert!(args[1].starts_with("--fingerprint="));
        let seed: u32 = args[1]
            .strip_prefix("--fingerprint=")
            .unwrap()
            .parse()
            .unwrap();
        assert!((10000..=99999).contains(&seed));
    }

    #[test]
    fn stealth_platform_branch() {
        let args = get_default_stealth_args();
        let platform_arg = &args[2];
        if cfg!(target_os = "macos") {
            assert_eq!(platform_arg, "--fingerprint-platform=macos");
        } else {
            assert_eq!(platform_arg, "--fingerprint-platform=windows");
        }
    }

    #[test]
    fn dedup_priority_user_overrides_stealth() {
        // User overrides the stealth fingerprint value; only one --fingerprint
        // survives, with the user's value.
        let args = build_args(
            true,
            Some(vec!["--fingerprint=12345".to_string()]),
            None,
            None,
            true,
            None,
            false,
        );
        let fingerprints: Vec<&String> = args
            .iter()
            .filter(|a| flag_key(a) == "--fingerprint")
            .collect();
        assert_eq!(fingerprints.len(), 1);
        assert_eq!(fingerprints[0], "--fingerprint=12345");
    }

    #[test]
    fn dedup_priority_timezone_overrides_user() {
        // Dedicated timezone param wins over a user-supplied timezone flag.
        let args = build_args(
            true,
            Some(vec!["--fingerprint-timezone=America/New_York".to_string()]),
            Some("Europe/London"),
            None,
            true,
            None,
            false,
        );
        let tzs: Vec<&String> = args
            .iter()
            .filter(|a| flag_key(a) == "--fingerprint-timezone")
            .collect();
        assert_eq!(tzs.len(), 1);
        assert_eq!(tzs[0], "--fingerprint-timezone=Europe/London");
    }

    #[test]
    fn locale_sets_lang_and_fingerprint_locale() {
        let args = build_args(false, None, None, Some("en-US"), true, None, false);
        assert!(args.contains(&"--lang=en-US".to_string()));
        assert!(args.contains(&"--fingerprint-locale=en-US".to_string()));
    }

    #[test]
    fn gpu_blocklist_bypass_when_headed() {
        let args = build_args(false, None, None, None, false, None, false);
        assert!(args.contains(&"--ignore-gpu-blocklist".to_string()));
    }

    #[test]
    fn gpu_blocklist_absent_headless_non_windows() {
        let args = build_args(false, None, None, None, true, None, false);
        if !cfg!(target_os = "windows") {
            assert!(!args.contains(&"--ignore-gpu-blocklist".to_string()));
        } else {
            assert!(args.contains(&"--ignore-gpu-blocklist".to_string()));
        }
    }

    #[test]
    fn start_maximized_gate_respects_existing_window_flag() {
        // A user --window-size suppresses the auto --start-maximized.
        let args = build_args(
            false,
            Some(vec!["--window-size=800,600".to_string()]),
            None,
            None,
            true,
            None,
            true,
        );
        assert!(!args.contains(&"--start-maximized".to_string()));
        assert!(args.contains(&"--window-size=800,600".to_string()));
    }

    #[test]
    fn start_maximized_added_when_no_window_flag() {
        let args = build_args(false, None, None, None, true, None, true);
        assert!(args.contains(&"--start-maximized".to_string()));
    }

    #[test]
    fn extension_paths_emit_both_flags() {
        let args = build_args(
            false,
            None,
            None,
            None,
            true,
            Some(vec!["/some/ext".to_string()]),
            false,
        );
        assert!(args.iter().any(|a| a.starts_with("--load-extension=")));
        assert!(args
            .iter()
            .any(|a| a.starts_with("--disable-extensions-except=")));
    }
}
