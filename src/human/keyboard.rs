//! Per-character humanized typing — PURE over the [`RawKeyboard`] trait.
//!
//! 1:1 port of `human/keyboard.py`: mistypes via QWERTY adjacency
//! ([`NEARBY_KEYS`]), shifted uppercase letters, and shift-symbols dispatched as
//! CDP-style key events with the correct `code` / `windowsVirtualKeyCode`.

use super::config::{rand_range, sleep_ms, HumanConfig};
use futures::future::BoxFuture;
use once_cell::sync::Lazy;
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;

/// The Shift modifier bit for `Input.dispatchKeyEvent`.
pub const SHIFT_MODIFIER: i64 = 8;

/// A CDP-style key event, produced by the pure typing logic and dispatched by
/// the driver via `Input.dispatchKeyEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEvent {
    /// `"keyDown"` or `"keyUp"`.
    pub event_type: KeyEventType,
    /// Modifier bitmask (0 or [`SHIFT_MODIFIER`]).
    pub modifiers: i64,
    /// `key` field (e.g. `"@"`, `"a"`, `"Shift"`, `"Backspace"`).
    pub key: String,
    /// DOM `code` (e.g. `"Digit2"`), if applicable.
    pub code: Option<String>,
    /// `windowsVirtualKeyCode`, if applicable.
    pub windows_virtual_key_code: Option<i64>,
    /// `text` field (present only for keyDown of printable chars).
    pub text: Option<String>,
    /// `unmodifiedText` field.
    pub unmodified_text: Option<String>,
}

/// Key event direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Down,
    Up,
}

/// Low-level keyboard sink. Mirrors Playwright's keyboard plus a raw CDP escape
/// hatch for shift-symbols (needed for `isTrusted` events).
///
/// Returns [`BoxFuture`]s so the trait is object-safe without `async-trait`.
pub trait RawKeyboard: Sync {
    /// Press a key down. `key` is a char or a name like `"Shift"`/`"Backspace"`.
    fn down(&self, key: &str) -> BoxFuture<'_, ()>;
    /// Release a key.
    fn up(&self, key: &str) -> BoxFuture<'_, ()>;
    /// Type a run of text (Playwright `keyboard.type`).
    fn type_text(&self, text: &str) -> BoxFuture<'_, ()>;
    /// Insert text without key events (Playwright `keyboard.insertText`).
    fn insert_text(&self, text: &str) -> BoxFuture<'_, ()>;
    /// Dispatch a raw CDP key event (used for shift-symbols). Default impl
    /// falls back to `insert_text` of the event's `text` on keyDown so pure
    /// implementations that lack CDP still type the character.
    fn dispatch_key_event(&self, ev: KeyEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if ev.event_type == KeyEventType::Down {
                if let Some(t) = ev.text {
                    self.insert_text(&t).await;
                }
            }
        })
    }
}

/// QWERTY adjacency table (verbatim from Python). Keys are lowercase.
pub static NEARBY_KEYS: Lazy<HashMap<char, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert('a', "sqwz");
    m.insert('b', "vghn");
    m.insert('c', "xdfv");
    m.insert('d', "sfecx");
    m.insert('e', "wrsdf");
    m.insert('f', "dgrtcv");
    m.insert('g', "fhtyb");
    m.insert('h', "gjybn");
    m.insert('i', "ujko");
    m.insert('j', "hkunm");
    m.insert('k', "jloi");
    m.insert('l', "kop");
    m.insert('m', "njk");
    m.insert('n', "bhjm");
    m.insert('o', "iklp");
    m.insert('p', "ol");
    m.insert('q', "wa");
    m.insert('r', "edft");
    m.insert('s', "awedxz");
    m.insert('t', "rfgy");
    m.insert('u', "yhji");
    m.insert('v', "cfgb");
    m.insert('w', "qase");
    m.insert('x', "zsdc");
    m.insert('y', "tghu");
    m.insert('z', "asx");
    m.insert('1', "2q");
    m.insert('2', "13qw");
    m.insert('3', "24we");
    m.insert('4', "35er");
    m.insert('5', "46rt");
    m.insert('6', "57ty");
    m.insert('7', "68yu");
    m.insert('8', "79ui");
    m.insert('9', "80io");
    m.insert('0', "9p");
    m
});

/// Set of shift-symbols routed through the CDP shift-symbol path.
pub const SHIFT_SYMBOLS: &str = "@#!$%^&*()_+{}|:\"<>?~";

/// `_SHIFT_SYMBOL_CODES` — DOM `code` for each shift-symbol.
pub static SHIFT_SYMBOL_CODES: Lazy<HashMap<char, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert('!', "Digit1");
    m.insert('@', "Digit2");
    m.insert('#', "Digit3");
    m.insert('$', "Digit4");
    m.insert('%', "Digit5");
    m.insert('^', "Digit6");
    m.insert('&', "Digit7");
    m.insert('*', "Digit8");
    m.insert('(', "Digit9");
    m.insert(')', "Digit0");
    m.insert('_', "Minus");
    m.insert('+', "Equal");
    m.insert('{', "BracketLeft");
    m.insert('}', "BracketRight");
    m.insert('|', "Backslash");
    m.insert(':', "Semicolon");
    m.insert('"', "Quote");
    m.insert('<', "Comma");
    m.insert('>', "Period");
    m.insert('?', "Slash");
    m.insert('~', "Backquote");
    m
});

/// `_SHIFT_SYMBOL_KEYCODES` — `windowsVirtualKeyCode` for each shift-symbol.
pub static SHIFT_SYMBOL_KEYCODES: Lazy<HashMap<char, i64>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert('!', 49);
    m.insert('@', 50);
    m.insert('#', 51);
    m.insert('$', 52);
    m.insert('%', 53);
    m.insert('^', 54);
    m.insert('&', 55);
    m.insert('*', 56);
    m.insert('(', 57);
    m.insert(')', 48);
    m.insert('_', 189);
    m.insert('+', 187);
    m.insert('{', 219);
    m.insert('}', 221);
    m.insert('|', 220);
    m.insert(':', 186);
    m.insert('"', 222);
    m.insert('<', 188);
    m.insert('>', 190);
    m.insert('?', 191);
    m.insert('~', 192);
    m
});

/// `_get_nearby_key` — pick a QWERTY neighbor of `ch`, preserving case. Returns
/// `ch` unchanged if it has no adjacency entry.
pub fn get_nearby_key(ch: char, rng: &mut impl Rng) -> char {
    let lower = ch.to_ascii_lowercase();
    match NEARBY_KEYS.get(&lower) {
        Some(neighbors) => {
            let bytes: Vec<char> = neighbors.chars().collect();
            let wrong = *bytes.choose(rng).unwrap();
            if ch.is_ascii_uppercase() {
                wrong.to_ascii_uppercase()
            } else {
                wrong
            }
        }
        None => ch,
    }
}

fn is_alnum(ch: char) -> bool {
    ch.is_alphanumeric()
}

async fn type_normal_char(kb: &dyn RawKeyboard, ch: char, cfg: &HumanConfig, rng: &mut impl Rng) {
    let s = ch.to_string();
    kb.down(&s).await;
    sleep_ms(rand_range(rng, cfg.key_hold)).await;
    kb.up(&s).await;
}

async fn type_shifted_char(kb: &dyn RawKeyboard, ch: char, cfg: &HumanConfig, rng: &mut impl Rng) {
    let s = ch.to_string();
    kb.down("Shift").await;
    sleep_ms(rand_range(rng, cfg.shift_down_delay)).await;
    kb.down(&s).await;
    sleep_ms(rand_range(rng, cfg.key_hold)).await;
    kb.up(&s).await;
    sleep_ms(rand_range(rng, cfg.shift_up_delay)).await;
    kb.up("Shift").await;
}

async fn type_shift_symbol(kb: &dyn RawKeyboard, ch: char, cfg: &HumanConfig, rng: &mut impl Rng) {
    let code = SHIFT_SYMBOL_CODES.get(&ch).copied();
    let key_code = SHIFT_SYMBOL_KEYCODES.get(&ch).copied();
    let s = ch.to_string();

    kb.down("Shift").await;
    sleep_ms(rand_range(rng, cfg.shift_down_delay)).await;

    kb.dispatch_key_event(KeyEvent {
        event_type: KeyEventType::Down,
        modifiers: SHIFT_MODIFIER,
        key: s.clone(),
        code: code.map(|c| c.to_string()),
        windows_virtual_key_code: key_code,
        text: Some(s.clone()),
        unmodified_text: Some(s.clone()),
    })
    .await;

    sleep_ms(rand_range(rng, cfg.key_hold)).await;

    kb.dispatch_key_event(KeyEvent {
        event_type: KeyEventType::Up,
        modifiers: SHIFT_MODIFIER,
        key: s.clone(),
        code: code.map(|c| c.to_string()),
        windows_virtual_key_code: key_code,
        text: None,
        unmodified_text: None,
    })
    .await;

    sleep_ms(rand_range(rng, cfg.shift_up_delay)).await;
    kb.up("Shift").await;
}

/// Dispatch the correct character (step 3 of the per-char loop).
async fn dispatch_char(kb: &dyn RawKeyboard, ch: char, cfg: &HumanConfig, rng: &mut impl Rng) {
    if ch.is_ascii_uppercase() {
        type_shifted_char(kb, ch, cfg, rng).await;
    } else if SHIFT_SYMBOLS.contains(ch) {
        type_shift_symbol(kb, ch, cfg, rng).await;
    } else {
        type_normal_char(kb, ch, cfg, rng).await;
    }
}

/// Inter-character delay (step 4). 10% "thinking" pause, else jittered cadence.
async fn inter_char_delay(cfg: &HumanConfig, rng: &mut impl Rng) {
    if rng.gen::<f64>() < cfg.typing_pause_chance {
        sleep_ms(rand_range(rng, cfg.typing_pause_range)).await;
    } else {
        let delay =
            cfg.typing_delay + (rng.gen::<f64>() - 0.5) * 2.0 * cfg.typing_delay_spread;
        sleep_ms(delay.max(10.0)).await;
    }
}

/// `human_type` — type `text` one character at a time with typos, shifted
/// letters, shift-symbols, and inter-character cadence. PURE over
/// [`RawKeyboard`].
pub async fn human_type(
    kb: &dyn RawKeyboard,
    text: &str,
    cfg: &HumanConfig,
    rng: &mut (impl Rng + Send),
) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for (i, &ch) in chars.iter().enumerate() {
        // 1. Non-ASCII: insert_text directly.
        if !ch.is_ascii() {
            sleep_ms(rand_range(rng, cfg.key_hold)).await;
            kb.insert_text(&ch.to_string()).await;
            if i + 1 < n {
                inter_char_delay(cfg, rng).await;
            }
            continue;
        }

        // 2. Mistype: type wrong neighbor, notice, backspace, correct.
        if rng.gen::<f64>() < cfg.mistype_chance && is_alnum(ch) {
            let wrong = get_nearby_key(ch, rng);
            dispatch_char(kb, wrong, cfg, rng).await;
            sleep_ms(rand_range(rng, cfg.mistype_delay_notice)).await;
            kb.down("Backspace").await;
            sleep_ms(rand_range(rng, cfg.key_hold)).await;
            kb.up("Backspace").await;
            sleep_ms(rand_range(rng, cfg.mistype_delay_correct)).await;
        }

        // 3. Dispatch correct char.
        dispatch_char(kb, ch, cfg, rng).await;

        // 4. Inter-char delay (skip after last char).
        if i + 1 < n {
            inter_char_delay(cfg, rng).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn nearby_map_values_verbatim() {
        assert_eq!(NEARBY_KEYS[&'a'], "sqwz");
        assert_eq!(NEARBY_KEYS[&'d'], "sfecx");
        assert_eq!(NEARBY_KEYS[&'2'], "13qw");
        assert_eq!(NEARBY_KEYS[&'0'], "9p");
        assert_eq!(NEARBY_KEYS.len(), 36); // 26 letters + 10 digits
    }

    #[test]
    fn nearby_key_preserves_case_and_membership() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..200 {
            let out = get_nearby_key('A', &mut rng);
            assert!(out.is_ascii_uppercase());
            assert!("sqwz".contains(out.to_ascii_lowercase()));
        }
        // char with no adjacency entry is returned unchanged
        assert_eq!(get_nearby_key(' ', &mut rng), ' ');
        assert_eq!(get_nearby_key('.', &mut rng), '.');
    }

    #[test]
    fn shift_symbol_tables_match() {
        assert_eq!(SHIFT_SYMBOL_CODES[&'@'], "Digit2");
        assert_eq!(SHIFT_SYMBOL_KEYCODES[&'@'], 50);
        assert_eq!(SHIFT_SYMBOL_CODES[&')'], "Digit0");
        assert_eq!(SHIFT_SYMBOL_KEYCODES[&')'], 48);
        assert_eq!(SHIFT_SYMBOL_CODES[&'~'], "Backquote");
        assert_eq!(SHIFT_SYMBOL_KEYCODES[&'~'], 192);
        assert_eq!(SHIFT_SYMBOLS.chars().count(), 21);
        // every shift symbol has both a code and a keycode
        for ch in SHIFT_SYMBOLS.chars() {
            assert!(SHIFT_SYMBOL_CODES.contains_key(&ch), "missing code {ch}");
            assert!(SHIFT_SYMBOL_KEYCODES.contains_key(&ch), "missing kc {ch}");
        }
    }
}
