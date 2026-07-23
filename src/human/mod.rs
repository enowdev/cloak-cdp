//! Humanize behavioral layer (mouse/keyboard/scroll). See docs/SPEC-human.md.
//!
//! The pure core ([`config`], [`mouse`], [`scroll`], [`keyboard`]) is a 1:1 port
//! of the Python `human/` package over the [`RawMouse`] / [`RawKeyboard`]
//! traits: it contains no browser-specific code, only math + randomness +
//! `tokio` sleeps. The optional chromiumoxide driver ([`driver`]) implements
//! those traits over CDP `Input.*`; it is currently compiled out (see that
//! module's header) so the pure core builds independently.

pub mod config;
pub mod keyboard;
pub mod mouse;
pub mod scroll;

// Optional CDP driver (clearly-marked, gated off until the chromiumoxide 0.7
// Input builder surface is confirmed — see `driver.rs`).
pub mod driver;

// --- Re-exports: the public surface described in SPEC-human.md ---

pub use config::{
    merge_config, rand_int_range, rand_range, resolve_config, sleep_ms, sleep_secs, Box,
    HumanConfig, HumanConfigOverrides, Point, Preset, Range,
};

pub use mouse::{
    bezier, click_target, ease_in_out, human_click, human_idle, human_move, RawMouse,
};

pub use keyboard::{
    get_nearby_key, human_type, KeyEvent, KeyEventType, RawKeyboard, NEARBY_KEYS,
    SHIFT_MODIFIER, SHIFT_SYMBOLS, SHIFT_SYMBOL_CODES, SHIFT_SYMBOL_KEYCODES,
};

pub use scroll::{
    human_scroll_into_view, is_in_viewport, smooth_wheel, ScrollResult, Viewport,
};
