# Rust Port Spec — human/ behavioral layer

## Files
- config.py: HumanConfig dataclass, presets, RNG/sleep helpers — PURE.
- mouse.py: bezier move, click, idle — PURE (only RawMouse trait touches browser).
- scroll.py: wheel scroll-into-view — PURE math; drives via RawMouse.wheel + get_box callback + viewport eval fallback.
- keyboard.py: per-char typing, typos, shift symbols — PURE except CDP shift-symbol + eval fallback.
- actionability.py: Playwright-style pre-action checks — re-express against chromiumoxide DOM.
- __init__.py (3051 lines): Playwright monkey-patch orchestration — THROWAWAY, rewrite as chromiumoxide driver.

No numpy/scipy/perlin. Only stdlib math (hypot, sin, cos, atan2, pi, pow, ceil), random (uniform, randint, random, choice), time.monotonic/sleep, asyncio.sleep. Rust: rand crate + std::f64; tokio::time::sleep. RNG non-crypto → rand::thread_rng fine. Write ONE async version (sync/async byte-identical algorithms).

## Public entry points
```
resolve_config(preset="default"|"careful", overrides=None) -> HumanConfig   # ValueError unknown preset
merge_config(base, overrides) -> HumanConfig   # never mutates base; unknown keys ignored
human_move(raw, start_x, start_y, end_x, end_y, cfg)
click_target(box{x,y,width,height}, is_input, cfg) -> Point
human_click(raw, is_input, cfg)
human_idle(raw, seconds, cx, cy, cfg)
human_type(page, raw, text, cfg, cdp_session=None)
human_scroll_into_view(page, raw, get_box, cursor_x, cursor_y, cfg) -> (box, cx, cy, did_scroll)
scroll_to_element(page, raw, selector, cursor_x, cursor_y, cfg, timeout=30000) -> (box,cx,cy,did_scroll)
```

## RawMouse / RawKeyboard (key for portability)
```
RawMouse: move(x,y) [ints, abs page coords]; down()/down(click_count=2); up(); wheel(delta_x, delta_y)
RawKeyboard: down(key); up(key); type(text); insert_text(text)   # key = char or "Shift"/"Backspace"
```
Rust/chromiumoxide: implement over Input.dispatchMouseEvent / Input.dispatchKeyEvent:
- move → Input.dispatchMouseEvent {type:"mouseMoved", x, y}
- down/up → {type:"mousePressed"/"mouseReleased", button:"left", clickCount:1|2}
- wheel → {type:"mouseWheel", deltaX, deltaY}
- key down/up → Input.dispatchKeyEvent {type:"keyDown"/"keyUp", ...}
- insert_text → Input.insertText

## 2.1 Mouse movement — cubic Bezier + ease + wobble + overshoot
```python
def _ease_in_out(t):
    if t < 0.5: return 4*t*t*t
    return 1 - pow(-2*t + 2, 3) / 2
# bezier (4 control pts):
u=1-t; uu=u*u; uuu=uu*u; tt=t*t; ttt=tt*t
x = uuu*p0.x + 3*uu*t*p1.x + 3*u*tt*p2.x + ttt*p3.x   # same for y
# control points (perpendicular offset at 25%, 75%):
dx=end.x-start.x; dy=end.y-start.y; dist=hypot(dx,dy) or 1
px=-dy/dist; py=dx/dist
bias1=rand(-0.3,0.3)*dist; bias2=rand(-0.3,0.3)*dist
cp1=(start.x+dx*0.25+px*bias1, start.y+dy*0.25+py*bias1)
cp2=(start.x+dx*0.75+px*bias2, start.y+dy*0.75+py*bias2)
```
human_move:
- dist=hypot(end-start); if dist<1: return.
- steps = max(25, min(80, round(dist/8)))   # mouse_min_steps, mouse_max_steps, mouse_steps_divisor
- for i in 0..=steps: progress=i/steps; eased_t=_ease_in_out(progress); pt=bezier(eased_t).
- wobble: wobble_amp=sin(pi*progress)*1.5 (mouse_wobble_max); wx=pt.x+(random()-0.5)*2*wobble_amp (same wy). move round(wx),round(wy).
- burst: counter++ each step; when counter>=burst_size (int in mouse_burst_size (3,5)) and i<steps: sleep rand(mouse_burst_pause (8,18)) ms, reset. (careful: (12,25))
- overshoot (prob mouse_overshoot_chance=0.15, careful 0.10): overshoot_dist=rand(mouse_overshoot_px (3,6)); angle=atan2(dy,dx) (ORIGINAL start→end); move overshoot past target; sleep rand(30,70); move back to end+(random()-0.5)*4 each axis.

## 2.2 click_target
```
is_input:  x_frac=rand(0.05,0.30) [click_input_x_range]; y_frac=rand(0.30,0.70)
else:      x_frac=rand(0.35,0.65); y_frac=rand(0.35,0.65)
target=(round(box.x+box.width*x_frac), round(box.y+box.height*y_frac))
```

## 2.3 human_click
aim_delay=rand(is_input ? click_aim_delay_input : _button); sleep_ms(aim_delay). hold=rand(is_input ? click_hold_input : _button); raw.down(); sleep_ms(hold); raw.up().
Defaults: aim_input(60,140), aim_button(80,200), hold_input(40,100), hold_button(60,150).
Double-click: down(click_count=2); sleep rand(30,60); up(click_count=2).

## 2.4 human_idle
Loop until monotonic()+seconds: x+=(random()-0.5)*2*idle_drift_px(3) (same y); move rounded; sleep rand(idle_pause_range (300,1000)) ms.

## 2.5 keyboard typing
Per char, index i:
1. Non-ASCII: sleep rand(key_hold); raw.insert_text(ch); inter-char delay.
2. Mistype (random()<mistype_chance=0.02 AND ch.isalnum()): type wrong neighbor (NEARBY_KEYS, preserve case); sleep rand(mistype_delay_notice (100,300)); down("Backspace"); sleep rand(key_hold); up("Backspace"); sleep rand(mistype_delay_correct (50,150)). Then type correct.
3. Dispatch correct:
   - Uppercase letter → _type_shifted_char: down("Shift"); sleep rand(shift_down_delay (30,70)); down(ch); sleep rand(key_hold (15,35)); up(ch); sleep rand(shift_up_delay (20,50)); up("Shift").
   - ch in SHIFT_SYMBOLS (@#!$%^&*()_+{}|:"<>?~) → _type_shift_symbol (CDP, below).
   - else _type_normal_char: down(ch); sleep rand(key_hold); up(ch).
4. Inter-char delay (if not last):
   if random()<typing_pause_chance(0.1): sleep rand(typing_pause_range (400,1000)).
   else: delay=typing_delay(70)+(random()-0.5)*2*typing_delay_spread(40); sleep max(10, delay).
   ⇒ normal keystroke uniform [30,110]ms clamped>=10; 10% "thinking" pause 400-1000.

NEARBY_KEYS (QWERTY adjacency, verbatim HashMap<char,&str>):
```
a:sqwz b:vghn c:xdfv d:sfecx e:wrsdf f:dgrtcv g:fhtyb h:gjybn i:ujko j:hkunm
k:jloi l:kop m:njk n:bhjm o:iklp p:ol q:wa r:edft s:awedxz t:rfgy u:yhji
v:cfgb w:qase x:zsdc y:tghu z:asx
1:2q 2:13qw 3:24we 4:35er 5:46rt 6:57ty 7:68yu 8:79ui 9:80io 0:9p
```
_get_nearby_key: wrong=choice(neighbors[ch.lower()]); .upper() if orig upper; not in map → unchanged.

## Shift-symbol typing (raw CDP Input.dispatchKeyEvent for isTrusted)
```python
raw.down("Shift"); sleep rand(shift_down_delay)
cdp.send("Input.dispatchKeyEvent", {type:"keyDown", modifiers:8, key:ch, code:code, windowsVirtualKeyCode:key_code, text:ch, unmodifiedText:ch})
sleep rand(key_hold)
cdp.send("Input.dispatchKeyEvent", {type:"keyUp", modifiers:8, key:ch, code:code, windowsVirtualKeyCode:key_code})
sleep rand(shift_up_delay); raw.up("Shift")
```
modifiers:8 = Shift. In Rust: emit these directly for ALL keys (no Playwright-vs-CDP split needed).
_SHIFT_SYMBOL_CODES: !:Digit1 @:Digit2 #:Digit3 $:Digit4 %:Digit5 ^:Digit6 &:Digit7 *:Digit8 (:Digit9 ):Digit0 _:Minus +:Equal {:BracketLeft }:BracketRight |:Backslash ::Semicolon ":Quote <:Comma >:Period ?:Slash ~:Backquote
_SHIFT_SYMBOL_KEYCODES: !:49 @:50 #:51 $:52 %:53 ^:54 &:55 *:56 (:57 ):48 _:189 +:187 {:219 }:221 |:220 ::186 ":222 <:188 >:190 ?:191 ~:192

## 2.6 Scrolling — accelerate → cruise → decelerate → optional overshoot
_is_in_viewport(bounds, vh, cfg): bounds.y >= vh*scroll_target_zone[0](0.20) AND bounds.y+height <= vh*scroll_target_zone[1](0.80).
_smooth_wheel(raw, delta, cfg): while sent<abs(delta): step=rand(20,40); chunk=min(step, abs(delta)-sent); raw.wheel(0, round(chunk)*sign); sent+=chunk; sleep rand(8,20).
human_scroll_into_view:
1. Get viewport (page.viewport_size else eval innerWidth/Height). Rust: CDP Page.getLayoutMetrics or eval.
2. box=get_box(); if in viewport → return (box,cx,cy,False) early.
3. Move cursor into scroll area: (round(vw*rand(0.3,0.7)), round(vh*rand(0.3,0.7))); human_move; sleep rand(scroll_pre_move_delay (100,300)).
4. target_y=vh*rand(scroll_target_zone 0.20..0.80); element_center=box.y+box.height/2; distance=element_center-target_y; direction=sign(distance); avg_delta=(80+130)/2=105; total_clicks=max(3, ceil(abs(distance)/avg_delta)); accel_steps=rand_int(2,3); decel_steps=rand_int(2,3).
5. Loop i in 0..total_clicks:
   - i<accel_steps: delta=rand(80,100), pause=rand(scroll_pause_slow (80,200)).
   - i>=total_clicks-decel_steps: delta=rand(60,90), pause=rand(scroll_pause_slow).
   - else cruise: delta=rand(scroll_delta_base (80,130)), pause=rand(scroll_pause_fast (30,80)).
   - delta*=1+(random()-0.5)*2*scroll_delta_variance(0.2); delta=round(delta)*direction.
   - _smooth_wheel(delta); scrolled+=abs(delta); sleep pause.
   - every 3rd (i%3==2) or last: re-get_box, break if in viewport.
   - break if scrolled>=abs_distance*1.1.
   (accel/decel rand(80,100)/rand(60,90) HARDCODED, not config.)
6. Overshoot (prob scroll_overshoot_chance=0.1): _smooth_wheel(round(rand(scroll_overshoot_px (50,150)))*direction); sleep rand(scroll_settle_delay (300,600)); then rand_int(1,2) corrections: _smooth_wheel(round(rand(40,80))*-direction); sleep rand(100,250).
7. Settle sleep rand(scroll_settle_delay); final get_box; return (box, scroll_area_cx, scroll_area_cy, True).
scroll_to_element: get_box = page.locator(selector).first.bounding_box(timeout).

## 2.7 Actionability (re-express against chromiumoxide DOM)
_BACKOFF_MS = [100, 250, 500, 1000] (clamp at last).
Check sets: CLICK={attached,visible,enabled,pointer_events}; HOVER={attached,visible,pointer_events}; INPUT={attached,visible,enabled,editable,pointer_events}; FOCUS={attached,visible,enabled}; CHECK={attached,visible,enabled,pointer_events}.
ensure_actionable: retry-until-timeout(30000ms): wait_for(attached), is_visible, is_enabled, is_editable.
ensure_stable: two bounding_box 100ms apart; stable when all x,y,width,height differ <=1px.
check_pointer_events: elementFromPoint(x-frameOffsetX, y-frameOffsetY) must be/contain expected; fails-open on None. frameOffset=box.x - getBoundingClientRect().x.
Errors subclass RuntimeError: ElementNot{Attached,Visible,Stable,Enabled,Editable,ReceivingEvents}Error.

## 2.8 High-level composition (from __init__.py)
- click: ensure(CLICK) → optional idle → scroll_to_element → detect is_input → if scrolled ensure_stable+re-box → click_target → check_pointer_events → human_move → human_click.
- type: ensure(INPUT) → sleep rand(field_switch_delay (800,1500)) → click(skip_checks) → sleep rand(100,250) → human_type.
- fill: type but after click+sleep: press(SelectAll: Meta+a darwin else Control+a) → sleep rand(30,80) → press("Backspace") → sleep rand(50,150) → human_type.
- select_option: ensure(FOCUS) → hover → sleep rand(100,300) → native select.
- press: ensure(FOCUS) → click if not focused → sleep rand(50,150) → native press.
- drag_to: move src center → sleep rand(100,200) → down → sleep rand(80,150) → move tgt center → sleep rand(80,150) → up.

## Stealth DOM reads via CDP Isolated Worlds (__init__.py)
Page.getFrameTree → Page.createIsolatedWorld{grantUniveralAccess:true} → Runtime.evaluate{contextId, returnByValue:true}. Avoids detectable page.evaluate. Invalidate on navigation, recreate on stale (2 attempts). Rust: Runtime.evaluate in isolated execution context.

## Cursor state
_CursorState: x:f64, y:f64, initialized:bool. Init once to (rand(initial_cursor_x (400,700)), rand(initial_cursor_y (45,60))) + immediate mouse.move there.

## HumanConfig — all fields (default | careful). Range=(f64,f64). rand_int_range=randint inclusive → gen_range(lo..=hi).
| Field | default | careful |
|---|---|---|
| typing_delay | 70 | 100 |
| typing_delay_spread | 40 | 50 |
| typing_pause_chance | 0.1 | 0.15 |
| typing_pause_range | (400,1000) | (500,1200) |
| shift_down_delay | (30,70) | (40,90) |
| shift_up_delay | (20,50) | (30,70) |
| key_hold | (15,35) | (20,45) |
| mistype_chance | 0.02 | 0.02 |
| mistype_delay_notice | (100,300) | (100,300) |
| mistype_delay_correct | (50,150) | (50,150) |
| field_switch_delay | (800,1500) | (1000,2000) |
| mouse_steps_divisor | 8 | 8 |
| mouse_min_steps | 25 | 25 |
| mouse_max_steps | 80 | 80 |
| mouse_wobble_max | 1.5 | 1.5 |
| mouse_overshoot_chance | 0.15 | 0.10 |
| mouse_overshoot_px | (3,6) | (3,6) |
| mouse_burst_size | (3,5) | (3,5) |
| mouse_burst_pause | (8,18) | (12,25) |
| click_aim_delay_input | (60,140) | (80,180) |
| click_aim_delay_button | (80,200) | (120,280) |
| click_hold_input | (40,100) | (60,140) |
| click_hold_button | (60,150) | (80,200) |
| click_input_x_range | (0.05,0.30) | (0.05,0.30) |
| idle_drift_px | 3 | 3 |
| idle_pause_range | (300,1000) | (300,1000) |
| scroll_delta_base | (80,130) | (80,130) |
| scroll_delta_variance | 0.2 | 0.2 |
| scroll_pause_fast | (30,80) | (100,200) |
| scroll_pause_slow | (80,200) | (250,600) |
| scroll_accel_steps | (2,3) | (2,3) |
| scroll_decel_steps | (2,3) | (2,3) |
| scroll_overshoot_chance | 0.1 | 0.1 |
| scroll_overshoot_px | (50,150) | (50,150) |
| scroll_settle_delay | (300,600) | (400,800) |
| scroll_target_zone | (0.20,0.80) | (0.20,0.80) |
| scroll_pre_move_delay | (100,300) | (150,400) |
| initial_cursor_x | (400,700) | (400,700) |
| initial_cursor_y | (45,60) | (45,60) |
| idle_between_actions | False | True |
| idle_between_duration | (0.3,0.8) | (0.4,1.0) |

sleep_ms(ms): sleep ms/1000.0 only if ms>0. All timing ms except idle_between_duration (seconds).
_SELECT_ALL = Meta+a on darwin else Control+a.

## Verdict
config/mouse/scroll/keyboard = pure computation over RawMouse/RawKeyboard + math/random → 1:1 Rust port. actionability = DOM queries vs chromiumoxide. __init__.py = throwaway, replace with thin chromiumoxide driver (HumanPage struct: cursor state + HumanConfig + CDP client).
JS reference impl exists at js/src/human (Python is authoritative).
