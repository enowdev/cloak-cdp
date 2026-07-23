//! Optional chromiumoxide driver: [`CdpMouse`] / [`CdpKeyboard`] implementing
//! the pure [`RawMouse`] / [`RawKeyboard`] traits over a chromiumoxide `Page`
//! via CDP `Input.*`.
//!
//! ---------------------------------------------------------------------------
//! CLEARLY-MARKED DRIVER SECTION — gated OFF by `#[cfg(any())]`.
//! ---------------------------------------------------------------------------
//! The pure behavioral core ([`super::config`], [`super::mouse`],
//! [`super::keyboard`], [`super::scroll`]) is fully self-contained and compiles
//! independently of chromiumoxide. This module wires those traits to real CDP
//! events. The exact builder method names for chromiumoxide 0.7's generated
//! `DispatchKeyEventParams` (optional fields: `code`, `windows_virtual_key_code`,
//! `unmodified_text`, `text`, `modifiers`) are not verified in this environment,
//! so the whole module is compiled out via `#[cfg(any())]` to guarantee the
//! pure core builds. It is retained verbatim as the reference wiring: flip the
//! gate to `#[cfg(all())]` (or move behind a real cargo feature) once the CDP
//! Input builder surface is confirmed against the resolved chromiumoxide.
//!
//! CDP mapping (per SPEC-human.md §RawMouse/RawKeyboard):
//! - move  → Input.dispatchMouseEvent {type:"mouseMoved", x, y}
//! - down  → {type:"mousePressed",  button:"left", clickCount}
//! - up    → {type:"mouseReleased", button:"left", clickCount}
//! - wheel → {type:"mouseWheel", deltaX, deltaY}
//! - key   → Input.dispatchKeyEvent {type:"keyDown"|"keyUp", ...}
//! - insert→ Input.insertText

#![cfg(any())] // whole module disabled; see header.

use super::keyboard::{KeyEvent, KeyEventType, RawKeyboard};
use super::mouse::RawMouse;
use chromiumoxide::Page;
use chromiumoxide_cdp::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams, MouseButton,
};
use futures::future::BoxFuture;

/// [`RawMouse`] over a chromiumoxide [`Page`].
pub struct CdpMouse<'p> {
    pub page: &'p Page,
}

impl<'p> RawMouse for CdpMouse<'p> {
    fn move_to(&self, x: f64, y: f64) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self
                .page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MouseMoved)
                        .x(x)
                        .y(y)
                        .build()
                        .unwrap(),
                )
                .await;
        })
    }

    fn down(&self, click_count: u32) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self
                .page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MousePressed)
                        .button(MouseButton::Left)
                        .click_count(click_count as i64)
                        .build()
                        .unwrap(),
                )
                .await;
        })
    }

    fn up(&self, click_count: u32) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self
                .page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MouseReleased)
                        .button(MouseButton::Left)
                        .click_count(click_count as i64)
                        .build()
                        .unwrap(),
                )
                .await;
        })
    }

    fn wheel(&self, delta_x: f64, delta_y: f64) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self
                .page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MouseWheel)
                        .x(0.0)
                        .y(0.0)
                        .delta_x(delta_x)
                        .delta_y(delta_y)
                        .build()
                        .unwrap(),
                )
                .await;
        })
    }
}

/// [`RawKeyboard`] over a chromiumoxide [`Page`].
pub struct CdpKeyboard<'p> {
    pub page: &'p Page,
}

impl<'p> CdpKeyboard<'p> {
    async fn key(&self, ty: DispatchKeyEventType, key: &str) {
        let _ = self
            .page
            .execute(
                DispatchKeyEventParams::builder()
                    .r#type(ty)
                    .key(key.to_string())
                    .build()
                    .unwrap(),
            )
            .await;
    }
}

impl<'p> RawKeyboard for CdpKeyboard<'p> {
    fn down(&self, key: &str) -> BoxFuture<'_, ()> {
        let key = key.to_string();
        Box::pin(async move {
            self.key(DispatchKeyEventType::KeyDown, &key).await;
        })
    }

    fn up(&self, key: &str) -> BoxFuture<'_, ()> {
        let key = key.to_string();
        Box::pin(async move {
            self.key(DispatchKeyEventType::KeyUp, &key).await;
        })
    }

    fn type_text(&self, text: &str) -> BoxFuture<'_, ()> {
        self.insert_text(text)
    }

    fn insert_text(&self, text: &str) -> BoxFuture<'_, ()> {
        let text = text.to_string();
        Box::pin(async move {
            let _ = self
                .page
                .execute(InsertTextParams::builder().text(text).build().unwrap())
                .await;
        })
    }

    fn dispatch_key_event(&self, ev: KeyEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let ty = match ev.event_type {
                KeyEventType::Down => DispatchKeyEventType::KeyDown,
                KeyEventType::Up => DispatchKeyEventType::KeyUp,
            };
            let mut b = DispatchKeyEventParams::builder()
                .r#type(ty)
                .modifiers(ev.modifiers)
                .key(ev.key);
            if let Some(code) = ev.code {
                b = b.code(code);
            }
            if let Some(kc) = ev.windows_virtual_key_code {
                b = b.windows_virtual_key_code(kc);
            }
            if let Some(text) = ev.text {
                b = b.text(text);
            }
            if let Some(u) = ev.unmodified_text {
                b = b.unmodified_text(u);
            }
            let _ = self.page.execute(b.build().unwrap()).await;
        })
    }
}
