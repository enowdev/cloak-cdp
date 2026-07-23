//! Mouse movement, click, and idle — PURE math over the [`RawMouse`] trait.
//!
//! 1:1 port of `human/mouse.py`. All movement math (cubic bezier, ease-in-out,
//! wobble, burst pauses, overshoot) is expressed against the async [`RawMouse`]
//! trait so it contains no browser-specific code.

use super::config::{rand_range, sleep_ms, Box, HumanConfig, Point};
use futures::future::BoxFuture;
use rand::Rng;

/// Low-level mouse sink. Coordinates are absolute page pixels; the driver rounds
/// to integers when dispatching CDP events.
///
/// Methods return a [`BoxFuture`] so the trait stays object-safe (`&dyn
/// RawMouse`) without depending on `async-trait`.
pub trait RawMouse: Sync {
    /// Move the pointer to `(x, y)` (absolute page coords).
    fn move_to(&self, x: f64, y: f64) -> BoxFuture<'_, ()>;
    /// Press the primary button. `click_count` is 1 (single) or 2 (double).
    fn down(&self, click_count: u32) -> BoxFuture<'_, ()>;
    /// Release the primary button. `click_count` matches the corresponding down.
    fn up(&self, click_count: u32) -> BoxFuture<'_, ()>;
    /// Dispatch a wheel event with the given deltas.
    fn wheel(&self, delta_x: f64, delta_y: f64) -> BoxFuture<'_, ()>;
}

/// `_ease_in_out` — cubic ease. Matches Python verbatim.
#[inline]
pub fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Evaluate a cubic Bezier at parameter `t` over four control points.
#[inline]
pub fn bezier(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let uu = u * u;
    let uuu = uu * u;
    let tt = t * t;
    let ttt = tt * t;
    Point {
        x: uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x,
        y: uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y,
    }
}

/// Compute the two interior control points (perpendicular offsets at 25% / 75%).
/// Returns `(cp1, cp2)`. Pure given the two random biases.
fn control_points(start: Point, end: Point, bias1: f64, bias2: f64) -> (Point, Point) {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let mut dist = dx.hypot(dy);
    if dist == 0.0 {
        dist = 1.0;
    }
    let px = -dy / dist;
    let py = dx / dist;
    let b1 = bias1 * dist;
    let b2 = bias2 * dist;
    let cp1 = Point {
        x: start.x + dx * 0.25 + px * b1,
        y: start.y + dy * 0.25 + py * b1,
    };
    let cp2 = Point {
        x: start.x + dx * 0.75 + px * b2,
        y: start.y + dy * 0.75 + py * b2,
    };
    (cp1, cp2)
}

/// `human_move` — traverse a humanized cubic-bezier path from start to end.
///
/// PURE over [`RawMouse`]. Draws all randomness from `rng`.
pub async fn human_move(
    raw: &dyn RawMouse,
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    cfg: &HumanConfig,
    rng: &mut (impl Rng + Send),
) {
    let start = Point {
        x: start_x,
        y: start_y,
    };
    let end = Point { x: end_x, y: end_y };
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let dist = dx.hypot(dy);
    if dist < 1.0 {
        return;
    }

    let bias1 = rng.gen_range(-0.3..=0.3);
    let bias2 = rng.gen_range(-0.3..=0.3);
    let (cp1, cp2) = control_points(start, end, bias1, bias2);

    // steps = clamp(round(dist / divisor), min, max)
    let raw_steps = (dist / cfg.mouse_steps_divisor).round();
    let steps = raw_steps
        .max(cfg.mouse_min_steps)
        .min(cfg.mouse_max_steps) as i64;

    // burst counter setup
    let mut counter = 0i64;
    let mut burst_size = rng.gen_range(
        cfg.mouse_burst_size.0.round() as i64..=cfg.mouse_burst_size.1.round() as i64,
    );

    for i in 0..=steps {
        let progress = i as f64 / steps as f64;
        let eased_t = ease_in_out(progress);
        let pt = bezier(start, cp1, cp2, end, eased_t);

        // wobble
        let wobble_amp = (std::f64::consts::PI * progress).sin() * cfg.mouse_wobble_max;
        let wx = pt.x + (rng.gen::<f64>() - 0.5) * 2.0 * wobble_amp;
        let wy = pt.y + (rng.gen::<f64>() - 0.5) * 2.0 * wobble_amp;
        raw.move_to(wx.round(), wy.round()).await;

        // burst pause
        counter += 1;
        if counter >= burst_size && i < steps {
            let pause = rand_range(rng, cfg.mouse_burst_pause);
            sleep_ms(pause).await;
            counter = 0;
            burst_size = rng.gen_range(
                cfg.mouse_burst_size.0.round() as i64..=cfg.mouse_burst_size.1.round() as i64,
            );
        }
    }

    // overshoot
    if rng.gen::<f64>() < cfg.mouse_overshoot_chance {
        let overshoot_dist = rand_range(rng, cfg.mouse_overshoot_px);
        let angle = dy.atan2(dx); // original start -> end direction
        let ox = end.x + angle.cos() * overshoot_dist;
        let oy = end.y + angle.sin() * overshoot_dist;
        raw.move_to(ox.round(), oy.round()).await;
        sleep_ms(rng.gen_range(30.0..=70.0)).await;
        let bx = end.x + (rng.gen::<f64>() - 0.5) * 4.0;
        let by = end.y + (rng.gen::<f64>() - 0.5) * 4.0;
        raw.move_to(bx.round(), by.round()).await;
    }
}

/// `click_target` — pick a randomized click point inside a bounding box.
pub fn click_target(b: Box, is_input: bool, cfg: &HumanConfig, rng: &mut impl Rng) -> Point {
    let (x_frac, y_frac) = if is_input {
        let x = rand_range(rng, cfg.click_input_x_range);
        let y = rand_range(rng, (0.30, 0.70));
        (x, y)
    } else {
        let x = rand_range(rng, (0.35, 0.65));
        let y = rand_range(rng, (0.35, 0.65));
        (x, y)
    };
    Point {
        x: (b.x + b.width * x_frac).round(),
        y: (b.y + b.height * y_frac).round(),
    }
}

/// `human_click` — aim delay, press, hold, release. `double` issues a
/// `click_count = 2` press/release with a short inter-press hold.
pub async fn human_click(
    raw: &dyn RawMouse,
    is_input: bool,
    double: bool,
    cfg: &HumanConfig,
    rng: &mut (impl Rng + Send),
) {
    let aim_range = if is_input {
        cfg.click_aim_delay_input
    } else {
        cfg.click_aim_delay_button
    };
    let aim_delay = rand_range(rng, aim_range);
    sleep_ms(aim_delay).await;

    if double {
        raw.down(2).await;
        sleep_ms(rng.gen_range(30.0..=60.0)).await;
        raw.up(2).await;
        return;
    }

    let hold_range = if is_input {
        cfg.click_hold_input
    } else {
        cfg.click_hold_button
    };
    let hold = rand_range(rng, hold_range);
    raw.down(1).await;
    sleep_ms(hold).await;
    raw.up(1).await;
}

/// `human_idle` — small random drift around `(cx, cy)` for `seconds`.
///
/// Returns the final cursor position so the caller can update its state.
pub async fn human_idle(
    raw: &dyn RawMouse,
    seconds: f64,
    cx: f64,
    cy: f64,
    cfg: &HumanConfig,
    rng: &mut (impl Rng + Send),
) -> Point {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds.max(0.0));
    let mut x = cx;
    let mut y = cy;
    while std::time::Instant::now() < deadline {
        x += (rng.gen::<f64>() - 0.5) * 2.0 * cfg.idle_drift_px;
        y += (rng.gen::<f64>() - 0.5) * 2.0 * cfg.idle_drift_px;
        raw.move_to(x.round(), y.round()).await;
        let pause = rand_range(rng, cfg.idle_pause_range);
        sleep_ms(pause).await;
    }
    Point { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_endpoints_and_midpoint() {
        assert!((ease_in_out(0.0) - 0.0).abs() < 1e-12);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-12);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-12);
        // monotone-ish sanity: 0.25 < 0.5 output
        assert!(ease_in_out(0.25) < 0.5);
        assert!(ease_in_out(0.75) > 0.5);
    }

    #[test]
    fn bezier_hits_endpoints() {
        let p0 = Point { x: 0.0, y: 0.0 };
        let p1 = Point { x: 10.0, y: 5.0 };
        let p2 = Point { x: 20.0, y: 15.0 };
        let p3 = Point { x: 30.0, y: 30.0 };
        let a = bezier(p0, p1, p2, p3, 0.0);
        let b = bezier(p0, p1, p2, p3, 1.0);
        assert!((a.x - p0.x).abs() < 1e-12 && (a.y - p0.y).abs() < 1e-12);
        assert!((b.x - p3.x).abs() < 1e-12 && (b.y - p3.y).abs() < 1e-12);
    }

    #[test]
    fn control_points_zero_dist_no_nan() {
        let s = Point { x: 5.0, y: 5.0 };
        let (cp1, cp2) = control_points(s, s, 0.1, -0.1);
        assert!(cp1.x.is_finite() && cp1.y.is_finite());
        assert!(cp2.x.is_finite() && cp2.y.is_finite());
    }
}
