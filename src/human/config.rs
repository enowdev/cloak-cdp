//! Behavioral tuning config (`HumanConfig`), presets, and RNG/sleep helpers.
//!
//! PURE port of `human/config.py`. All timing values are milliseconds except
//! `idle_between_duration` which is seconds. Ranges are inclusive `(lo, hi)`
//! pairs sampled with `rand::Rng::gen_range(lo..=hi)`.

use rand::Rng;
use std::time::Duration;

/// An inclusive `(lo, hi)` range sampled uniformly.
pub type Range = (f64, f64);

/// A 2D point in absolute page coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// An element bounding box in absolute page coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Box {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Behavioral tuning config (mirrors Python `HumanConfig`).
///
/// Construct via [`HumanConfig::default`] (the `default` preset),
/// [`HumanConfig::careful`], or [`resolve_config`].
#[derive(Debug, Clone, PartialEq)]
pub struct HumanConfig {
    // --- typing ---
    pub typing_delay: f64,
    pub typing_delay_spread: f64,
    pub typing_pause_chance: f64,
    pub typing_pause_range: Range,
    pub shift_down_delay: Range,
    pub shift_up_delay: Range,
    pub key_hold: Range,
    pub mistype_chance: f64,
    pub mistype_delay_notice: Range,
    pub mistype_delay_correct: Range,
    pub field_switch_delay: Range,

    // --- mouse ---
    pub mouse_steps_divisor: f64,
    pub mouse_min_steps: f64,
    pub mouse_max_steps: f64,
    pub mouse_wobble_max: f64,
    pub mouse_overshoot_chance: f64,
    pub mouse_overshoot_px: Range,
    pub mouse_burst_size: Range,
    pub mouse_burst_pause: Range,

    // --- click ---
    pub click_aim_delay_input: Range,
    pub click_aim_delay_button: Range,
    pub click_hold_input: Range,
    pub click_hold_button: Range,
    pub click_input_x_range: Range,

    // --- idle ---
    pub idle_drift_px: f64,
    pub idle_pause_range: Range,

    // --- scroll ---
    pub scroll_delta_base: Range,
    pub scroll_delta_variance: f64,
    pub scroll_pause_fast: Range,
    pub scroll_pause_slow: Range,
    pub scroll_accel_steps: Range,
    pub scroll_decel_steps: Range,
    pub scroll_overshoot_chance: f64,
    pub scroll_overshoot_px: Range,
    pub scroll_settle_delay: Range,
    pub scroll_target_zone: Range,
    pub scroll_pre_move_delay: Range,

    // --- cursor init ---
    pub initial_cursor_x: Range,
    pub initial_cursor_y: Range,

    // --- idle-between-actions ---
    pub idle_between_actions: bool,
    pub idle_between_duration: Range,
}

impl Default for HumanConfig {
    /// The `default` preset.
    fn default() -> Self {
        HumanConfig {
            typing_delay: 70.0,
            typing_delay_spread: 40.0,
            typing_pause_chance: 0.1,
            typing_pause_range: (400.0, 1000.0),
            shift_down_delay: (30.0, 70.0),
            shift_up_delay: (20.0, 50.0),
            key_hold: (15.0, 35.0),
            mistype_chance: 0.02,
            mistype_delay_notice: (100.0, 300.0),
            mistype_delay_correct: (50.0, 150.0),
            field_switch_delay: (800.0, 1500.0),

            mouse_steps_divisor: 8.0,
            mouse_min_steps: 25.0,
            mouse_max_steps: 80.0,
            mouse_wobble_max: 1.5,
            mouse_overshoot_chance: 0.15,
            mouse_overshoot_px: (3.0, 6.0),
            mouse_burst_size: (3.0, 5.0),
            mouse_burst_pause: (8.0, 18.0),

            click_aim_delay_input: (60.0, 140.0),
            click_aim_delay_button: (80.0, 200.0),
            click_hold_input: (40.0, 100.0),
            click_hold_button: (60.0, 150.0),
            click_input_x_range: (0.05, 0.30),

            idle_drift_px: 3.0,
            idle_pause_range: (300.0, 1000.0),

            scroll_delta_base: (80.0, 130.0),
            scroll_delta_variance: 0.2,
            scroll_pause_fast: (30.0, 80.0),
            scroll_pause_slow: (80.0, 200.0),
            scroll_accel_steps: (2.0, 3.0),
            scroll_decel_steps: (2.0, 3.0),
            scroll_overshoot_chance: 0.1,
            scroll_overshoot_px: (50.0, 150.0),
            scroll_settle_delay: (300.0, 600.0),
            scroll_target_zone: (0.20, 0.80),
            scroll_pre_move_delay: (100.0, 300.0),

            initial_cursor_x: (400.0, 700.0),
            initial_cursor_y: (45.0, 60.0),

            idle_between_actions: false,
            idle_between_duration: (0.3, 0.8),
        }
    }
}

impl HumanConfig {
    /// The `careful` preset (slower, more deliberate cadence).
    pub fn careful() -> Self {
        HumanConfig {
            typing_delay: 100.0,
            typing_delay_spread: 50.0,
            typing_pause_chance: 0.15,
            typing_pause_range: (500.0, 1200.0),
            shift_down_delay: (40.0, 90.0),
            shift_up_delay: (30.0, 70.0),
            key_hold: (20.0, 45.0),
            mistype_chance: 0.02,
            mistype_delay_notice: (100.0, 300.0),
            mistype_delay_correct: (50.0, 150.0),
            field_switch_delay: (1000.0, 2000.0),

            mouse_steps_divisor: 8.0,
            mouse_min_steps: 25.0,
            mouse_max_steps: 80.0,
            mouse_wobble_max: 1.5,
            mouse_overshoot_chance: 0.10,
            mouse_overshoot_px: (3.0, 6.0),
            mouse_burst_size: (3.0, 5.0),
            mouse_burst_pause: (12.0, 25.0),

            click_aim_delay_input: (80.0, 180.0),
            click_aim_delay_button: (120.0, 280.0),
            click_hold_input: (60.0, 140.0),
            click_hold_button: (80.0, 200.0),
            click_input_x_range: (0.05, 0.30),

            idle_drift_px: 3.0,
            idle_pause_range: (300.0, 1000.0),

            scroll_delta_base: (80.0, 130.0),
            scroll_delta_variance: 0.2,
            scroll_pause_fast: (100.0, 200.0),
            scroll_pause_slow: (250.0, 600.0),
            scroll_accel_steps: (2.0, 3.0),
            scroll_decel_steps: (2.0, 3.0),
            scroll_overshoot_chance: 0.1,
            scroll_overshoot_px: (50.0, 150.0),
            scroll_settle_delay: (400.0, 800.0),
            scroll_target_zone: (0.20, 0.80),
            scroll_pre_move_delay: (150.0, 400.0),

            initial_cursor_x: (400.0, 700.0),
            initial_cursor_y: (45.0, 60.0),

            idle_between_actions: true,
            idle_between_duration: (0.4, 1.0),
        }
    }
}

/// Overrides applied on top of a preset. Every field is optional; unset fields
/// leave the base value unchanged. Mirrors the "unknown keys ignored" semantics
/// of the Python `merge_config` (there simply are no unknown keys here).
#[derive(Debug, Clone, Default)]
pub struct HumanConfigOverrides {
    pub typing_delay: Option<f64>,
    pub typing_delay_spread: Option<f64>,
    pub typing_pause_chance: Option<f64>,
    pub typing_pause_range: Option<Range>,
    pub shift_down_delay: Option<Range>,
    pub shift_up_delay: Option<Range>,
    pub key_hold: Option<Range>,
    pub mistype_chance: Option<f64>,
    pub mistype_delay_notice: Option<Range>,
    pub mistype_delay_correct: Option<Range>,
    pub field_switch_delay: Option<Range>,

    pub mouse_steps_divisor: Option<f64>,
    pub mouse_min_steps: Option<f64>,
    pub mouse_max_steps: Option<f64>,
    pub mouse_wobble_max: Option<f64>,
    pub mouse_overshoot_chance: Option<f64>,
    pub mouse_overshoot_px: Option<Range>,
    pub mouse_burst_size: Option<Range>,
    pub mouse_burst_pause: Option<Range>,

    pub click_aim_delay_input: Option<Range>,
    pub click_aim_delay_button: Option<Range>,
    pub click_hold_input: Option<Range>,
    pub click_hold_button: Option<Range>,
    pub click_input_x_range: Option<Range>,

    pub idle_drift_px: Option<f64>,
    pub idle_pause_range: Option<Range>,

    pub scroll_delta_base: Option<Range>,
    pub scroll_delta_variance: Option<f64>,
    pub scroll_pause_fast: Option<Range>,
    pub scroll_pause_slow: Option<Range>,
    pub scroll_accel_steps: Option<Range>,
    pub scroll_decel_steps: Option<Range>,
    pub scroll_overshoot_chance: Option<f64>,
    pub scroll_overshoot_px: Option<Range>,
    pub scroll_settle_delay: Option<Range>,
    pub scroll_target_zone: Option<Range>,
    pub scroll_pre_move_delay: Option<Range>,

    pub initial_cursor_x: Option<Range>,
    pub initial_cursor_y: Option<Range>,

    pub idle_between_actions: Option<bool>,
    pub idle_between_duration: Option<Range>,
}

/// Merge `overrides` on top of `base`, returning a new config. Never mutates
/// `base`. Mirrors Python `merge_config`.
pub fn merge_config(base: &HumanConfig, overrides: &HumanConfigOverrides) -> HumanConfig {
    let mut c = base.clone();
    macro_rules! ov {
        ($f:ident) => {
            if let Some(v) = overrides.$f {
                c.$f = v;
            }
        };
    }
    ov!(typing_delay);
    ov!(typing_delay_spread);
    ov!(typing_pause_chance);
    ov!(typing_pause_range);
    ov!(shift_down_delay);
    ov!(shift_up_delay);
    ov!(key_hold);
    ov!(mistype_chance);
    ov!(mistype_delay_notice);
    ov!(mistype_delay_correct);
    ov!(field_switch_delay);
    ov!(mouse_steps_divisor);
    ov!(mouse_min_steps);
    ov!(mouse_max_steps);
    ov!(mouse_wobble_max);
    ov!(mouse_overshoot_chance);
    ov!(mouse_overshoot_px);
    ov!(mouse_burst_size);
    ov!(mouse_burst_pause);
    ov!(click_aim_delay_input);
    ov!(click_aim_delay_button);
    ov!(click_hold_input);
    ov!(click_hold_button);
    ov!(click_input_x_range);
    ov!(idle_drift_px);
    ov!(idle_pause_range);
    ov!(scroll_delta_base);
    ov!(scroll_delta_variance);
    ov!(scroll_pause_fast);
    ov!(scroll_pause_slow);
    ov!(scroll_accel_steps);
    ov!(scroll_decel_steps);
    ov!(scroll_overshoot_chance);
    ov!(scroll_overshoot_px);
    ov!(scroll_settle_delay);
    ov!(scroll_target_zone);
    ov!(scroll_pre_move_delay);
    ov!(initial_cursor_x);
    ov!(initial_cursor_y);
    ov!(idle_between_actions);
    ov!(idle_between_duration);
    c
}

/// A preset selector for [`resolve_config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Default,
    Careful,
}

impl Preset {
    /// Parse a preset name. Mirrors Python `ValueError` on unknown preset.
    pub fn parse(name: &str) -> crate::Result<Preset> {
        match name {
            "default" => Ok(Preset::Default),
            "careful" => Ok(Preset::Careful),
            other => Err(crate::CloakError::other(format!(
                "unknown human preset: {other}"
            ))),
        }
    }

    fn base(self) -> HumanConfig {
        match self {
            Preset::Default => HumanConfig::default(),
            Preset::Careful => HumanConfig::careful(),
        }
    }
}

/// Resolve a preset by name and apply optional overrides.
///
/// Mirrors Python `resolve_config(preset, overrides)` — errors on an unknown
/// preset name.
pub fn resolve_config(
    preset: &str,
    overrides: Option<&HumanConfigOverrides>,
) -> crate::Result<HumanConfig> {
    let base = Preset::parse(preset)?.base();
    Ok(match overrides {
        Some(ov) => merge_config(&base, ov),
        None => base,
    })
}

// --------------------------------------------------------------------------
// RNG / sleep helpers (PURE)
// --------------------------------------------------------------------------

/// Sample a uniform float in the inclusive range `(lo, hi)`.
#[inline]
pub fn rand_range(rng: &mut impl Rng, range: Range) -> f64 {
    let (lo, hi) = range;
    if hi <= lo {
        return lo;
    }
    rng.gen_range(lo..=hi)
}

/// Sample an inclusive integer in `(lo, hi)` (Python `randint`), returned as an
/// `i64`. Endpoints are rounded to the nearest integer.
#[inline]
pub fn rand_int_range(rng: &mut impl Rng, range: Range) -> i64 {
    let lo = range.0.round() as i64;
    let hi = range.1.round() as i64;
    if hi <= lo {
        return lo;
    }
    rng.gen_range(lo..=hi)
}

/// `sleep_ms`: sleep for `ms` milliseconds, but only when `ms > 0`.
pub async fn sleep_ms(ms: f64) {
    if ms > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(ms / 1000.0)).await;
    }
}

/// Sleep for a number of seconds (used by idle-between-actions).
pub async fn sleep_secs(secs: f64) {
    if secs > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_careful_differ() {
        let d = HumanConfig::default();
        let c = HumanConfig::careful();
        assert_eq!(d.typing_delay, 70.0);
        assert_eq!(c.typing_delay, 100.0);
        assert!(!d.idle_between_actions);
        assert!(c.idle_between_actions);
        assert_eq!(d.mouse_overshoot_chance, 0.15);
        assert_eq!(c.mouse_overshoot_chance, 0.10);
    }

    #[test]
    fn resolve_unknown_preset_errors() {
        assert!(resolve_config("nope", None).is_err());
        assert!(resolve_config("default", None).is_ok());
        assert!(resolve_config("careful", None).is_ok());
    }

    #[test]
    fn merge_does_not_mutate_base() {
        let base = HumanConfig::default();
        let ov = HumanConfigOverrides {
            typing_delay: Some(999.0),
            ..Default::default()
        };
        let merged = merge_config(&base, &ov);
        assert_eq!(base.typing_delay, 70.0);
        assert_eq!(merged.typing_delay, 999.0);
        // untouched field carried through
        assert_eq!(merged.key_hold, base.key_hold);
    }
}
