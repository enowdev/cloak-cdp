//! Humanize behavioral layer (mouse/keyboard/scroll). See docs/SPEC-human.md.
//!
//! The pure core (config, mouse math, scroll math, keyboard cadence) is a 1:1
//! port over the `RawMouse`/`RawKeyboard` traits; the chromiumoxide driver
//! implements those traits over CDP `Input.*`.
//!
//! STUB — to be implemented. Submodules will live under `human/`.

/// Behavioral tuning config (mirrors Python `HumanConfig`).
#[derive(Debug, Clone)]
pub struct HumanConfig {
    // fields filled in by implementation — see docs/SPEC-human.md
    _private: (),
}

impl Default for HumanConfig {
    fn default() -> Self {
        HumanConfig { _private: () }
    }
}

/// A 2D point in absolute page coordinates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
