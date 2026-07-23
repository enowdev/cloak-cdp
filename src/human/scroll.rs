//! Scroll-into-view math — PURE over [`RawMouse`], driven by a `get_box`
//! callback and a viewport size. 1:1 port of `human/scroll.py`.

use super::config::{rand_range, sleep_ms, Box as HBox, HumanConfig};
use super::mouse::{human_move, RawMouse};
use futures::future::BoxFuture;
use rand::Rng;

/// Viewport size in CSS pixels.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// `_is_in_viewport` — element fully inside the configured target zone.
pub fn is_in_viewport(bounds: HBox, vh: f64, cfg: &HumanConfig) -> bool {
    bounds.y >= vh * cfg.scroll_target_zone.0
        && bounds.y + bounds.height <= vh * cfg.scroll_target_zone.1
}

/// `_smooth_wheel` — emit `delta` in 20–40px chunks with small pauses.
/// `delta` may be negative (scroll up).
pub async fn smooth_wheel(
    raw: &dyn RawMouse,
    delta: f64,
    rng: &mut (impl Rng + Send),
) {
    let sign = if delta < 0.0 { -1.0 } else { 1.0 };
    let total = delta.abs();
    let mut sent = 0.0;
    while sent < total {
        let step: f64 = rng.gen_range(20.0..=40.0);
        let chunk = step.min(total - sent);
        raw.wheel(0.0, (chunk.round()) * sign).await;
        sent += chunk;
        sleep_ms(rng.gen_range(8.0..=20.0)).await;
    }
}

/// Result of a scroll-into-view: the final box, cursor position, and whether a
/// scroll actually occurred.
#[derive(Debug, Clone, Copy)]
pub struct ScrollResult {
    pub bounds: HBox,
    pub cursor_x: f64,
    pub cursor_y: f64,
    pub did_scroll: bool,
}

/// `human_scroll_into_view` — accelerate → cruise → decelerate → optional
/// overshoot, re-querying `get_box` periodically.
///
/// `get_box` returns the element's current bounding box (post-scroll). PURE over
/// [`RawMouse`] + the callback + the supplied `viewport`.
pub async fn human_scroll_into_view<'a, F>(
    raw: &dyn RawMouse,
    mut get_box: F,
    viewport: Viewport,
    cursor_x: f64,
    cursor_y: f64,
    cfg: &HumanConfig,
    rng: &mut (impl Rng + Send),
) -> ScrollResult
where
    F: FnMut() -> BoxFuture<'a, HBox>,
{
    let vw = viewport.width;
    let vh = viewport.height;

    // 2. Early return if already in viewport.
    let mut bounds = get_box().await;
    if is_in_viewport(bounds, vh, cfg) {
        return ScrollResult {
            bounds,
            cursor_x,
            cursor_y,
            did_scroll: false,
        };
    }

    // 3. Move cursor into the scroll area.
    let area_cx = (vw * rng.gen_range(0.3..=0.7)).round();
    let area_cy = (vh * rng.gen_range(0.3..=0.7)).round();
    human_move(raw, cursor_x, cursor_y, area_cx, area_cy, cfg, rng).await;
    sleep_ms(rand_range(rng, cfg.scroll_pre_move_delay)).await;

    // 4. Plan the scroll.
    let target_y = vh * rng.gen_range(cfg.scroll_target_zone.0..=cfg.scroll_target_zone.1);
    let element_center = bounds.y + bounds.height / 2.0;
    let distance = element_center - target_y;
    let direction = if distance < 0.0 { -1.0 } else { 1.0 };
    let abs_distance = distance.abs();
    let avg_delta = (80.0 + 130.0) / 2.0; // 105
    let total_clicks = ((abs_distance / avg_delta).ceil() as i64).max(3);
    let accel_steps = rng.gen_range(
        cfg.scroll_accel_steps.0.round() as i64..=cfg.scroll_accel_steps.1.round() as i64,
    );
    let decel_steps = rng.gen_range(
        cfg.scroll_decel_steps.0.round() as i64..=cfg.scroll_decel_steps.1.round() as i64,
    );

    // 5. Scroll loop.
    let mut scrolled = 0.0;
    for i in 0..total_clicks {
        let (mut delta, pause) = if i < accel_steps {
            // accel — HARDCODED range, not config.
            (rng.gen_range(80.0..=100.0), rand_range(rng, cfg.scroll_pause_slow))
        } else if i >= total_clicks - decel_steps {
            // decel — HARDCODED range, not config.
            (rng.gen_range(60.0..=90.0), rand_range(rng, cfg.scroll_pause_slow))
        } else {
            // cruise.
            (
                rand_range(rng, cfg.scroll_delta_base),
                rand_range(rng, cfg.scroll_pause_fast),
            )
        };
        delta *= 1.0 + (rng.gen::<f64>() - 0.5) * 2.0 * cfg.scroll_delta_variance;
        delta = delta.round() * direction;

        smooth_wheel(raw, delta, rng).await;
        scrolled += delta.abs();
        sleep_ms(pause).await;

        // every 3rd (i%3==2) or last: re-check.
        if i % 3 == 2 || i == total_clicks - 1 {
            bounds = get_box().await;
            if is_in_viewport(bounds, vh, cfg) {
                break;
            }
        }
        if scrolled >= abs_distance * 1.1 {
            break;
        }
    }

    // 6. Overshoot.
    if rng.gen::<f64>() < cfg.scroll_overshoot_chance {
        let over = rand_range(rng, cfg.scroll_overshoot_px).round() * direction;
        smooth_wheel(raw, over, rng).await;
        sleep_ms(rand_range(rng, cfg.scroll_settle_delay)).await;
        let corrections = rng.gen_range(1..=2);
        for _ in 0..corrections {
            let corr = rng.gen_range::<f64, _>(40.0..=80.0).round() * -direction;
            smooth_wheel(raw, corr, rng).await;
            sleep_ms(rng.gen_range(100.0..=250.0)).await;
        }
    }

    // 7. Settle + final box.
    sleep_ms(rand_range(rng, cfg.scroll_settle_delay)).await;
    bounds = get_box().await;

    ScrollResult {
        bounds,
        cursor_x: area_cx,
        cursor_y: area_cy,
        did_scroll: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_zone_bounds() {
        let cfg = HumanConfig::default();
        // zone is (0.20, 0.80) of a 1000px viewport => [200, 800].
        let inside = HBox { x: 0.0, y: 300.0, width: 10.0, height: 100.0 };
        assert!(is_in_viewport(inside, 1000.0, &cfg));
        let too_high = HBox { x: 0.0, y: 100.0, width: 10.0, height: 50.0 };
        assert!(!is_in_viewport(too_high, 1000.0, &cfg));
        let too_low = HBox { x: 0.0, y: 750.0, width: 10.0, height: 100.0 };
        assert!(!is_in_viewport(too_low, 1000.0, &cfg));
    }
}
