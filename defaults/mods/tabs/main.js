import { View, div, svg, image } from "gpui-kit";
import { h_flex, ScrollHandle } from "gpui-base";
import { spaces, panes, run, openOverlay, theme, window, shared, layout, settings } from "herdr";

// The person's working_spinner: the ring, or frames in the terminal font,
// centred in a fixed width so nothing beside it moves as they change.
function working(t) {
  const s = settings().working_spinner;
  if (!s) return svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900);
  return div().min_w(12).flex_none().flex().justify_center().font_family(settings().font_family).text_color(t.working).frames(s.frames, s.interval_ms).child(s.frames[0]);
}

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// As upstream's tab bar: drops a status glyph, keeps a leading bracket or quote.
const clean = (s) => (s ?? "").replace(/^[^\p{L}\p{N}\p{Ps}\p{Pi}]+/u, "").replace(/^\[ [!.] \] /, "").trim();

// A tab Herdr named (unnamed, or after its program) follows its agent's
// label (its pane's given name, telling title or session name: pane_label.rs),
// as the agents list does, a title's leading status glyph dropped; a tab name
// the person gave stays.
function label(tab, lead) {
  const prog = lead?.program?.name;
  const base = tab.named ? tab.name : prog ?? "Terminal";
  if (!lead?.program?.agent) return base;
  // A name someone gave the pane (pane.rename) shows as given; only a title is cleaned.
  const own = lead.name && lead.label === lead.name ? lead.label : clean(lead.label);
  return (tab.name === prog || !tab.named) && own && own !== prog && own !== lead.terminal ? own : base;
}

// Dragging a tab: a press becomes a drag once the pointer moves this far (px).
export const DRAG_THRESHOLD = 4;

// A drop slot is the gap before tab `slot` (tabs.length: after the last).
// Over a tab, its left half is the slot before it and its right half the one after.
export function slotOver(i, x, width) {
  return x < width / 2 ? i : i + 1;
}

// The tab.move index (1 = first) that puts the tab at `from` into `slot`,
// or null when that leaves it where it is.
export function dropIndex(from, slot) {
  if (slot === null || slot === from || slot === from + 1) return null;
  return slot < from ? slot + 1 : slot;
}

export default class Tabs extends View {
  init(_props, cx) {
    // press: the tab the left button went down on; drag: once it moved far enough.
    this.press = null;
    this.drag = null;
    this.handle = cx.focus_handle();
    this.scroll = ScrollHandle.new();
    try { this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}"); } catch { this.panels = {}; }
    if (!this.panels || typeof this.panels !== "object" || Array.isArray(this.panels)) this.panels = {};
    this.swipe = { acc: 0, steps: 0, at: 0, last: 0 };
  }

  // One tab per wheel notch. A touchpad or smooth-scrolling mouse sends many
  // small events per gesture: a short swipe or one smoothed notch (about
  // 25 px) switches one tab, a longer swipe keeps going a tab per 60 px, at
  // most one every 100 ms so each shows.
  wheel(e) {
    if (e.delta_lines) {
      if (e.delta_lines.y) run(e.delta_lines.y > 0 ? "tab.prev" : "tab.next");
      return;
    }
    const s = this.swipe, now = Date.now();
    // Phases often arrive only as moves (a macOS touchpad included): a pause
    // or a turn starts a new gesture.
    if (e.touch_phase === "started" || now - s.at > 250 || e.delta.y * s.acc < 0) Object.assign(s, { acc: 0, steps: 0 });
    s.at = now;
    s.acc += e.delta.y;
    const need = s.steps ? 60 : 20;
    if (Math.abs(s.acc) < need || now - s.last < 100) return;
    s.acc -= Math.sign(s.acc) * need;
    s.steps++;
    s.last = now;
    run(e.delta.y > 0 ? "tab.prev" : "tab.next");
  }

  pressTab(c, e) {
    // A drag still going missed its release: it ends here, as it would have then.
    if (this.drag) this.endDrag();
    this.press = { id: c.id, x: e.position.x, y: e.position.y, grab: e.local_position?.x ?? 0, width: e.bounds?.width ?? 0 };
  }

  // Every pointer move over the strip; `slot` is where a drop would land, or
  // undefined to keep the last one (the gaps between tabs).
  movePointer(e, slot, cx) {
    if (!this.press) return;
    // The left button came up where the strip didn't hear it (a lost mouse-up).
    if (e.pressed_button !== "left") return this.endDrag(cx);
    if (!this.drag) {
      if (Math.max(Math.abs(e.position.x - this.press.x), Math.abs(e.position.y - this.press.y)) <= DRAG_THRESHOLD) return;
      // Esc cancels, so the keyboard comes here until the drop; it goes back after.
      this.drag = { id: this.press.id, x: e.position.x, slot: null };
      this.handle.focus();
    }
    if (slot !== undefined) this.drag.slot = slot;
    this.drag.x = e.position.x;
    cx.notify();
  }

  // Outside the row of tabs (the strip's padding, the buttons): the ends.
  edgeSlot(x, shown) {
    const row = this.row;
    if (row && x < row.x) return 0;
    if (row && x > row.x + row.width) return shown.length;
    return undefined;
  }

  drop(shown) {
    const from = shown.findIndex(c => c.id === this.drag.id);
    const index = from === -1 ? null : dropIndex(from, this.drag.slot);
    if (index !== null) run("tab.move", { id: this.drag.id, index });
  }

  endDrag(cx) {
    if (this.drag && this.handle.is_focused()) run("pane.refocus");
    this.press = null;
    this.drag = null;
    cx?.notify();
  }

  // The side panel is remembered per space: switching shows that space's choice
  // (closed if it never had one), and opening or closing it records it.
  syncPanel(space, panel, body, enabled) {
    if (!space || !panel) return;
    const now = panel.hidden ? null : body?.active ?? null;
    if (space.id !== this.space) {
      const first = this.space === undefined;
      this.space = space.id; this.pending = null;
      if (!(first && !(space.id in this.panels))) {
        const stored = this.panels[space.id] ?? null, want = enabled.includes(stored) ? stored : null;
        if (want !== now) {
          this.pending = { want, until: Date.now() + 1000 };
          if (want) { run("layout.select", { stack: "panel-body", child: want }); run("layout.show", { id: "panel" }); }
          else run("layout.hide", { id: "panel" });
          return;
        }
      }
    }
    // Until the restore lands (or something else won), the old state isn't a choice.
    if (this.pending) {
      if (this.pending.want !== now && Date.now() < this.pending.until) return;
      this.pending = null;
    }
    if (this.panels[space.id] === now) return;
    this.panels[space.id] = now;
    localStorage.setItem("panels", JSON.stringify(this.panels));
  }

  render() {
    const t = theme();
    const space = spaces().find(p => p.active);
    const shown = space?.tabs ?? [];
    // Read once: every read copies all panes.
    const ps = panes();
    const active = shown.findIndex(tab => tab.active);
    const selected = shown[active]?.id;
    if (selected !== this.selected || active !== this.selectedIndex) {
      this.selected = selected;
      this.selectedIndex = active;
      if (active >= 0) this.scroll.scroll_to_item(active);
    }
    const stop = (_e, cx) => cx.stop_propagation();
    const button = (id, label, action) => div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", stop).on_click(action).child(label);
    const drag = this.drag, from = drag ? shown.findIndex(x => x.id === drag.id) : -1;
    const marker = from === -1 || dropIndex(from, drag.slot) === null ? null : drag.slot;
    // What a tab looks like; the dragged one's ghost is drawn the same, without its controls.
    const face = (c, ghost) => {
      const lead = ps.find(p => p.tab === c.id && p.program?.agent) ?? ps.find(p => p.tab === c.id);
      const program = lead?.program;
      const name = label(c, lead);
      return h_flex().h(29).px(10).gap(7).rounded(7).items_center().flex_none().text_size(12.8)
        .border(1).border_color(c.active || ghost ? t.border : t.chrome).bg(c.active || ghost ? t.raised : t.chrome).text_color(c.active ? t.foreground : t.muted)
        .when(Boolean(c.icon), el => el.child(div().flex_none().text_size(13).text_color(t.muted).when(c.icon.kind === "glyph", g => g.font_family("Symbols Nerd Font")).when(c.icon.kind === "letter", l => l.font_bold()).child(c.icon.text)))
        .when(!c.icon && Boolean(program?.icon), el => el.child(program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14)))
        .child(div().max_w(170).truncate().child(c.zoomed ? `${name} Z` : name))
        .when(c.status === "working", el => el.child(working(t)))
        .when(c.status === "blocked" || c.status === "done", el => el.child(div().size(6).rounded(3).bg(t[c.status])))
        .when(c.active && !ghost, el => el.child(div().id(`close-${c.id}`).text_color(t.dim).on_mouse_down("left", stop).on_click((_e, cx) => { cx.stop_propagation(); run("tab.close", { id: c.id }); }).child(svg("mods/pane/close.svg").size(12))));
    };
    // Where a drop lands: a line in the gap, centred in it (inside the strip's edge at the first tab).
    const line = (side, first) => div().absolute().top(4).h(21).w(2).rounded(1).bg(t.accent)
      .when(side === "left", el => el.left(first ? 0 : -2.5)).when(side === "right", el => el.right(-2.5));
    const item = (c, i) => face(c, false).id(`tab-${c.id}`).relative()
        // A rename or status dot on this or an earlier tab can push the active one out.
        .when(i <= active, el => el.on_size_change((_size, cx) => { this.scroll.scroll_to_item(active); cx.notify(); }))
        .when(c.id === drag?.id, el => el.opacity(0.4))
        .when(!drag, el => el.hover(s => s.bg(t.hover)))
        .on_mouse_down("left", (e, cx) => { cx.stop_propagation(); this.pressTab(c, e); if (e.click_count === 2) run("tab.rename", { id: c.id }); })
        .on_mouse_move((e, cx) => this.movePointer(e, slotOver(i, e.local_position.x, e.bounds.width), cx))
        .on_mouse_up("left", () => { if (this.press?.id === c.id && !this.drag) run("tab.select", { id: c.id }); })
        .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); openOverlay({ mod: "menu", kind: "popup", width: 200, props: { tab: "tab", target: { id: c.id, space: space.id } } }); })
        .when(marker === i, el => el.child(line("left", i === 0)))
        .when(marker === shown.length && i === shown.length - 1, el => el.child(line("right", false)));
    const ghost = () => {
      const c = shown[from], box = this.strip, w = this.press?.width ?? 0;
      const x = Math.max(0, Math.min(box.width - w, drag.x - box.x - (this.press?.grab ?? 0)));
      return div().absolute().left(x).top(Math.max(0, (box.height - 29) / 2)).opacity(0.9).child(face(c, true));
    };
    const root = layout(), sidebar = find(root, "sidebar"), w = window();
    // Old saved rails and a dragged-narrow sidebar can be smaller than the window buttons.
    const lightsPadding = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights
      ? Math.max(10, (root.traffic_lights[0] + 64) / settings().ui_scale - (sidebar?.hidden ? 0 : sidebar?.size?.px ?? 0))
      : 10;
    const panel = find(root, "panel"), body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(space, panel, body, enabled);
    const panelButton = (id, label, icon) => {
      const on = panel && !panel.hidden && body?.active === id;
      return h_flex().id(`panel-${id}`).h(26).px(8).gap(5).flex_none().items_center().rounded(6)
        .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
        .hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
        .on_click(() => {
          if (on) run("layout.hide", { id: "panel" });
          else { run("layout.select", { stack: "panel-body", child: id }); run("layout.show", { id: "panel" }); }
        }).child(svg(icon).size(14)).child(label);
    };
    return h_flex().id("tab-strip").relative().size_full().min_w(0).px(10).pl(lightsPadding).gap(3).items_center().bg(t.chrome).border_b(1).border_color(t.border)
      .on_mouse_move((e, cx) => { this.strip = e.bounds; this.movePointer(e, this.edgeSlot(e.position.x, shown), cx); })
      .on_mouse_up("left", (_e, cx) => { if (this.drag) this.drop(shown); this.endDrag(cx); })
      // Leaving the strip (released outside it, or not) cancels a drag.
      .on_hover((on, cx) => { if (!on && this.press) this.endDrag(cx); })
      .track_focus(this.handle)
      .on_key_down((e, cx) => { if (e.key === "escape" && this.drag) { cx.stop_propagation(); this.endDrag(cx); } })
      // A drag keeps its tabs where they are.
      .on_scroll_wheel(e => { if (!this.drag) this.wheel(e); })
      .child(h_flex().id("tab-scroll").flex_1().min_w(0).h_full().items_center().gap(3).overflow_x_scrollbar().track_scroll(this.scroll)
        .on_size_change((_size, cx) => { if (active >= 0) { this.scroll.scroll_to_item(active); cx.notify(); } })
        .on_mouse_move(e => { this.row = e.bounds; }).children(shown.map(item))
        // Past the last tab: drop at the end.
        .child(div().id("tab-tail").flex_1().min_w(24).h_full().on_mouse_move((e, cx) => this.movePointer(e, shown.length, cx))))
      .child(button("tab-new", "+", () => openOverlay({ mod: "new-menu", kind: "popup", width: 250, height: 512 })))
      .when(enabled.includes("files"), el => el.child(panelButton("files", "Files", "mods/files/file.svg")))
      .when(enabled.includes("changes"), el => el.child(panelButton("changes", "Changes", "mods/changes/changes.svg")))
      .when(w?.buttons, el => el.child(button("minimize", w.platform === "windows" ? svg("mods/tabs/win-min.svg").size(16) : "−", () => run("window.minimize")))
        .child(button("maximize", w.platform === "windows" ? svg(`mods/tabs/${w.maximized ? "win-restore" : "win-max"}.svg`).size(16) : "□", () => run("window.maximize")))
        .child(button("close-window", w.platform === "windows" ? svg("mods/tabs/win-close.svg").size(16) : svg("mods/pane/close.svg").size(13), () => run("window.close"))))
      .when(from !== -1 && Boolean(this.strip), el => el.child(ghost()));
  }
}
