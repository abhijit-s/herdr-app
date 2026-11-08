//! Floating panes on screen: drawn above the shown tab's tiled panes, back to
//! front, and the `pane.float*` commands. What floats and how a pane docks
//! back is the model's (`crate::float_pane`, `Model::float_pane`).
//!
//! Mouse move and resize (phase 2) start from `render_float_pane`'s frame and
//! go through `float_bounds`; until then a press on a float focuses and raises it.

use super::*;
use crate::float_pane::{DEFAULT_RECT, FloatPane, MIN_COLS, MIN_ROWS, Rect};
use crate::layout::Edge;

pub(super) const COMMANDS: [&str; 8] =
    ["pane.float", "pane.dock", "pane.float_toggle", "pane.float_move", "pane.float_resize", "pane.raise", "pane.lower", "pane.float_cycle"];

/// How far `pane.float_move` and `pane.float_resize` step without `cells`: columns, rows.
const STEP: (f64, f64) = (2.0, 1.0);

/// A float's corner radius when the layout frames no panes.
const RADIUS: f32 = 8.0;

/// Where the floating layer's bounds (the tab area) are kept.
fn layer_slot(tab: &str) -> String {
    format!("float-layer:{tab}")
}

impl AppWindow {
    /// The layout's pane frame, for floating panes: always framed, even alone.
    pub(super) fn float_frame(&self) -> Option<&PaneFrame> {
        self.chrome.visible_blocks().into_iter().find_map(|n| match n.content() {
            Some(Content::Context { pane, .. }) => pane.as_ref(),
            _ => None,
        })
    }

    /// A tab: its tiled panes, and its floating ones above them. A tab without
    /// floats draws exactly as before.
    pub(super) fn render_tab(&self, tab: &Tab, focused: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        let tiled = self.render_node(tab.shown(), focused, self.pane_frame(), cx);
        if tab.floating.is_empty() {
            return tiled;
        }
        let key = layer_slot(&tab.id);
        let area = self.bounds.borrow().get(&key).map(|b| b.size);
        let measure = {
            let bounds_map = self.bounds.clone();
            canvas(
                move |b, window, _| {
                    // Floats are placed from the area last drawn: when it changes, draw again at once.
                    if area != Some(b.size) {
                        window.request_animation_frame();
                    }
                    bounds_map.borrow_mut().insert(key, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        };
        let zoomed = tab.zoomed.as_deref();
        let floats: Vec<AnyElement> = tab
            .shown_floats()
            .map(|f| {
                let rect = if zoomed == Some(f.pane.id.as_str()) { [0.0, 0.0, 1.0, 1.0] } else { f.rect };
                self.render_float_pane(f, rect, area, focused, cx)
            })
            .collect();
        div().relative().size_full().flex().child(tiled).child(measure).children(floats).into_any_element()
    }

    fn render_float_pane(&self, f: &FloatPane, rect: Rect, area: Option<Size<Pixels>>, focused: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        let frame = self.float_frame();
        let mut el = div().absolute().flex().occlude();
        el = match area {
            Some(area) => {
                let b = self.float_bounds(rect, area, frame);
                el.left(b.origin.x).top(b.origin.y).w(b.size.width).h(b.size.height)
            }
            // Not measured yet (its first frame): fractions, unsnapped.
            None => el.left(relative(rect[0] as f32)).top(relative(rect[1] as f32)).w(relative(rect[2] as f32)).h(relative(rect[3] as f32)),
        };
        // Opaque, so the tiled panes never show through a translucent terminal.
        el = el.rounded(settings::z(frame.and_then(|f| f.radius).unwrap_or(RADIUS))).bg(c("background")).shadow_lg();
        if frame.is_none() {
            let on = focused.contains(&f.pane.id);
            el = el.overflow_hidden().border_1().border_color(c(if on { "accent" } else { "border" }));
        }
        el.child(self.render_node(&f.pane, focused, frame, cx)).into_any_element()
    }

    /// A float's rect in px inside the tab area: at least `MIN_COLS` × `MIN_ROWS`
    /// cells (or the whole area), its terminal a whole number of cells, inside the area.
    pub(super) fn float_bounds(&self, rect: Rect, area: Size<Pixels>, frame: Option<&PaneFrame>) -> Bounds<Pixels> {
        let cell = self.float_cell.get();
        let zoom = settings::ui_scale();
        let inset = frame.map_or(1.0, pane_size::frame_inset) * zoom;
        let head = frame.and_then(|f| f.header.as_ref()).filter(|h| !h.hover()).map_or(0.0, pane_size::header_height) * zoom;
        let (pad_x, pad_y) = settings::get().terminal_padding_px();
        let fit = |len: f32, chrome: f32, cell: f32, min: f64, max: f32| {
            // The whole area (`to: "full"`, a zoomed float) is the area, as for a lone tiled pane.
            if len >= max {
                return max;
            }
            let cell = cell.max(1.0);
            let len = len.max(chrome + min as f32 * cell).min(max);
            let snapped = chrome + ((len - chrome) / cell).floor() * cell;
            if snapped > chrome { snapped } else { len }
        };
        let (aw, ah) = (f32::from(area.width), f32::from(area.height));
        let w = fit(rect[2] as f32 * aw, 2.0 * (inset + pad_x), f32::from(cell.width), MIN_COLS, aw);
        let h = fit(rect[3] as f32 * ah, 2.0 * (inset + pad_y) + head, f32::from(cell.height), MIN_ROWS, ah);
        let x = (rect[0] as f32 * aw).round().min(aw - w).max(0.0);
        let y = (rect[1] as f32 * ah).round().min(ah - h).max(0.0);
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    /// The front floating pane of the shown tab drawn under `at`.
    pub(super) fn float_at(&self, at: Point<Pixels>) -> Option<String> {
        let tab = self.model.active_tab()?;
        let bounds = self.bounds.borrow();
        tab.shown_floats().rev().find(|f| bounds.get(&f.pane.id).is_some_and(|b| b.contains(&at))).map(|f| f.pane.id.clone())
    }

    /// The `pane.float*` commands.
    pub(super) fn float_command(&mut self, name: &str, a: &Value, window: &Window) -> Result<(), String> {
        if name == "pane.float_cycle" {
            // `pane.float_cycle:true` (keys.json's shorthand) gives a string.
            let reverse = matches!(a.get("reverse"), Some(Value::Bool(true))) || Self::arg(a, "reverse") == Some("true");
            return self.cycle_floats(reverse);
        }
        let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane()).ok_or("no pane")?;
        let tab = self.model.tab_of(&pane).ok_or_else(|| format!("no pane {pane:?}"))?;
        let rect = tab.float(&pane).map(|f| f.rect);
        let tiled = |name: &str| format!("{name}: pane {pane:?} doesn't float (float it with pane.float)");
        match name {
            "pane.float" => self.model.float_pane(&pane, rect_arg(a)?.unwrap_or(DEFAULT_RECT))?,
            "pane.dock" => {
                let edge = match a.get("edge").filter(|e| !e.is_null()) {
                    Some(e) => Some(serde_json::from_value::<Edge>(e.clone()).map_err(|_| format!("pane.dock edge is left, right, top or bottom, not {e}"))?),
                    None => None,
                };
                let target = match (Self::arg(a, "target"), edge) {
                    (Some(t), edge) => Some((t, edge.unwrap_or(Edge::Right))),
                    (None, Some(_)) => return Err("pane.dock edge needs a target".into()),
                    (None, None) => None,
                };
                self.model.dock_pane(&pane, target)?;
            }
            "pane.float_toggle" if rect.is_some() => self.model.dock_pane(&pane, None)?,
            "pane.float_toggle" => self.model.float_pane(&pane, DEFAULT_RECT)?,
            "pane.raise" | "pane.lower" => {
                rect.ok_or_else(|| tiled(name))?;
                let raise = name == "pane.raise";
                let step = match (raise, Self::arg(a, "to")) {
                    (true, None) => 1,
                    (true, Some("front")) => isize::MAX,
                    (false, None) => -1,
                    (false, Some("back")) => isize::MIN,
                    (_, Some(to)) => return Err(format!("{name} to is {:?}, not {to:?}", if raise { "front" } else { "back" })),
                };
                self.model.restack_pane(&pane, step);
                return Ok(());
            }
            _ => {
                let rect = rect.ok_or_else(|| tiled(name))?;
                let step = self.float_step(&pane, a, window)?;
                let next = if name == "pane.float_move" { moved(rect, a, step)? } else { resized(rect, a, step)? };
                self.model.set_float_rect(&pane, next);
                return Ok(());
            }
        }
        self.focused_block = None;
        self.focused_pane = None;
        Ok(())
    }

    /// `cells` (default `STEP`) columns and rows as fractions of `pane`'s tab area.
    fn float_step(&self, pane: &str, a: &Value, window: &Window) -> Result<(f64, f64), String> {
        let (cols, rows) = match a.get("cells").filter(|c| !c.is_null()) {
            Some(c) => {
                let n = c.as_f64().filter(|n| *n > 0.0).ok_or_else(|| format!("cells is a number above 0, not {c}"))?;
                (n, n)
            }
            None => STEP,
        };
        let tab = self.model.tab_of(pane).map(|t| layer_slot(&t.id));
        let area = tab.and_then(|k| self.bounds.borrow().get(&k).map(|b| b.size)).filter(|s| s.width > px(0.0) && s.height > px(0.0));
        let cell = crate::terminal::cell_size(window);
        Ok(match area {
            Some(a) => (cols * f64::from(f32::from(cell.width) / f32::from(a.width)), rows * f64::from(f32::from(cell.height) / f32::from(a.height))),
            // Never drawn: a typical 120 × 40 grid.
            None => (cols / 120.0, rows / 40.0),
        })
    }

    /// `pane.float_cycle`: from a tiled pane the front float; then the back one
    /// (focusing brings it to the front), or with `reverse` the one behind the front.
    fn cycle_floats(&mut self, reverse: bool) -> Result<(), String> {
        let tab = self.model.active_tab().ok_or("no tab")?;
        let front = tab.floating.last().map(|f| f.pane.id.clone()).ok_or("the shown tab has no floating panes")?;
        let on_float = tab.focused.as_deref().is_some_and(|p| tab.float(p).is_some());
        let next = match (on_float, reverse) {
            (false, _) => front,
            (true, false) => tab.floating[0].pane.id.clone(),
            (true, true) => {
                let behind = tab.floating.iter().rev().nth(1).map_or_else(|| front.clone(), |f| f.pane.id.clone());
                self.model.restack_pane(&front, isize::MIN);
                behind
            }
        };
        self.model.focus_pane(&next);
        self.focused_block = None;
        self.focused_pane = None;
        Ok(())
    }
}

fn rect_arg(a: &Value) -> Result<Option<Rect>, String> {
    match a.get("rect").filter(|r| !r.is_null()) {
        None => Ok(None),
        Some(r) => serde_json::from_value::<Rect>(r.clone()).map(Some).map_err(|_| format!("rect is [x, y, w, h], fractions of the tab area, not {r}")),
    }
}

/// `pane.float_move`: exactly one of direction (by `step`), rect, or to. A
/// direction that isn't left, right, up or down is a `to` (keys.json's
/// `pane.float_move:center` shorthand fills direction).
fn moved(r: Rect, a: &Value, (dx, dy): (f64, f64)) -> Result<Rect, String> {
    let given = ["direction", "rect", "to"].into_iter().filter(|k| a.get(*k).is_some_and(|v| !v.is_null())).count();
    if given != 1 {
        return Err("pane.float_move needs exactly one of direction, rect or to".into());
    }
    if let Some(rect) = rect_arg(a)? {
        return Ok(rect);
    }
    let [x, y, w, h] = r;
    let direction = a.get("direction").and_then(Value::as_str);
    let snap = direction.filter(|d| !matches!(*d, "left" | "right" | "up" | "down"));
    if let Some(to) = a.get("to").and_then(Value::as_str).or(snap) {
        return Ok(match to {
            "center" => [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h],
            "full" => [0.0, 0.0, 1.0, 1.0],
            "left" => [0.0, 0.0, 0.5, 1.0],
            "right" => [0.5, 0.0, 0.5, 1.0],
            "top" => [0.0, 0.0, 1.0, 0.5],
            "bottom" => [0.0, 0.5, 1.0, 0.5],
            "top-left" => [0.0, 0.0, 0.5, 0.5],
            "top-right" => [0.5, 0.0, 0.5, 0.5],
            "bottom-left" => [0.0, 0.5, 0.5, 0.5],
            "bottom-right" => [0.5, 0.5, 0.5, 0.5],
            other => return Err(format!("pane.float_move to is center, full, left, right, top, bottom or a corner (top-left…), not {other:?}")),
        });
    }
    Ok(match direction {
        Some("left") => [x - dx, y, w, h],
        Some("right") => [x + dx, y, w, h],
        Some("up") => [x, y - dy, w, h],
        Some("down") => [x, y + dy, w, h],
        other => return Err(format!("pane.float_move direction is left, right, up or down, not {other:?}")),
    })
}

/// `pane.float_resize`: right and down grow (up to the tab's edge), left and
/// up shrink, the top-left corner staying.
fn resized([x, y, w, h]: Rect, a: &Value, (dx, dy): (f64, f64)) -> Result<Rect, String> {
    Ok(match a.get("direction").and_then(Value::as_str) {
        Some("right") => [x, y, (w + dx).min(1.0 - x), h],
        Some("left") => [x, y, w - dx, h],
        Some("down") => [x, y, w, (h + dy).min(1.0 - y)],
        Some("up") => [x, y, w, h - dy],
        other => return Err(format!("pane.float_resize direction is left, right, up or down, not {other:?}")),
    })
}

#[cfg(test)]
mod tests {
    use super::{moved, resized};
    use serde_json::json;

    #[test]
    fn moves_need_exactly_one_way_and_snap_to_halves_quarters_and_full() {
        let r = [0.2, 0.2, 0.4, 0.4];
        assert_eq!(moved(r, &json!({"direction": "right"}), (0.1, 0.05)).unwrap(), [0.30000000000000004, 0.2, 0.4, 0.4]);
        assert_eq!(moved(r, &json!({"to": "center"}), (0.0, 0.0)).unwrap(), [0.3, 0.3, 0.4, 0.4]);
        assert_eq!(moved(r, &json!({"direction": "full"}), (0.0, 0.0)).unwrap(), [0.0, 0.0, 1.0, 1.0], "the shorthand's snap");
        assert_eq!(moved(r, &json!({"to": "bottom-right"}), (0.0, 0.0)).unwrap(), [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(moved(r, &json!({"rect": [0.1, 0.1, 0.2, 0.2]}), (0.0, 0.0)).unwrap(), [0.1, 0.1, 0.2, 0.2]);
        assert!(moved(r, &json!({}), (0.0, 0.0)).is_err());
        assert!(moved(r, &json!({"to": "full", "direction": "up"}), (0.0, 0.0)).is_err());
        assert!(moved(r, &json!({"to": "middle"}), (0.0, 0.0)).unwrap_err().contains("middle"));
        assert!(moved(r, &json!({"rect": [1, 2]}), (0.0, 0.0)).unwrap_err().contains("[x, y, w, h]"));
    }

    #[test]
    fn resizes_keep_the_top_left_corner() {
        let r = [0.2, 0.2, 0.4, 0.4];
        assert_eq!(resized(r, &json!({"direction": "down"}), (0.1, 0.05)).unwrap(), [0.2, 0.2, 0.4, 0.45]);
        assert_eq!(resized(r, &json!({"direction": "left"}), (0.1, 0.05)).unwrap(), [0.2, 0.2, 0.30000000000000004, 0.4]);
        assert!(resized(r, &json!({"direction": "out"}), (0.1, 0.05)).is_err());
    }
}
