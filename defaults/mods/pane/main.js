// A pane's header, for layouts that frame their panes:
//   { "tab": "active", "pane": { "header": { "content": { "mod": "pane" } } } }
// Shows the pane's icon (its `icon`: one chosen, or its automatic name's;
// else the program's from the app's program table), the pane's label (its
// name, automatic name, telling title, session, else program: see `label`
// in AGENTS.md), where an agent or a bare shell unnamed shows its program's name, the folder,
// the pane's own text size while pane.zoom_text_* sets it, the agent's
// `pane_header_info` (Herdr's `header_info`: as many segments as fit, the
// whole line in its tooltip) and its status; buttons split, zoom and close this pane.
// Props from the app: { pane, terminal }. Click: focus the pane; right-click: its menu.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, props, run, theme, home, settings, fitHeaderInfo, headerTextWidth } from "herdr";

const TEXT = 12;
const GAP = 8;
// What the header's other items take, in px, for what's left to the info.
const BUTTONS = 4 * (14 + 6 + GAP);
const STATUS = { working: 12, blocked: 70, done: 7 };

// About the width `text` takes at the header's text size, as fitHeaderInfo estimates it.
const estimate = (text) => headerTextWidth(text, TEXT);

// The width left to the info: the header less its padding and every other item but the
// folder (which gives way first), the buttons too while hovered.
function infoRoom(width, items, hovered) {
  if (width == null) return 0;
  const taken = items.filter((w) => w != null);
  return width - 20 - taken.reduce((a, w) => a + w + GAP, GAP) - (hovered ? BUTTONS : 0);
}

// A separator as its mark alone (` · ` as ·), the same padding either side
// whatever spaces the template wrote, in the info's colour: even gaps.
const SEP_PAD = 4;
function separator(t, sep) {
  return div().flex_none().px(SEP_PAD).text_color(t.dim).child(sep.trim() || sep);
}

// The info that fits beside everything else: the name keeps its width, the
// buttons theirs while hovered; segments go from the right (Herdr's
// fitHeaderInfo), the last one left ends in an ellipsis.
function info(t, p, room) {
  const shown = fitHeaderInfo(p.header_info.segments, room, TEXT);
  if (!shown.length) return null;
  const last = shown.length - 1;
  return h_flex()
    .id(`pane-info-${p.pane}`)
    .flex_shrink_1()
    .min_w(0)
    .overflow_hidden()
    .tooltip(p.header_info.text)
    .children(shown.flatMap((s, i) => [
      ...(s.sep ? [separator(t, s.sep)] : []),
      div().when(i === last, (el) => el.min_w(0).truncate()).when(i !== last, (el) => el.flex_none().whitespace_nowrap()).text_color(s.ask ? t.blocked : t.dim).child(s.text),
    ]));
}

// The person's working_spinner: the ring, or frames in the terminal font,
// centred in a fixed width so nothing beside it moves as they change.
function working(t) {
  const s = settings().working_spinner;
  if (!s) return svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900);
  return div().min_w(12).flex_none().flex().justify_center().font_family(settings().font_family).text_color(t.working).frames(s.frames, s.interval_ms).child(s.frames[0]);
}

function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}

// A pane's own icon (`icon`): a Nerd Font glyph, an emoji or a letter, sized as the program's.
function own(ic, size, color) {
  return div().flex_none().text_size(size).text_color(color)
    .when(ic.kind === "glyph", (el) => el.font_family("Symbols Nerd Font"))
    .when(ic.kind === "letter", (el) => el.font_bold())
    .child(ic.text);
}

function folder(cwd) {
  if (!cwd) return "";
  const h = home();
  return cwd === h ? "~" : cwd.startsWith(h + "/") ? "~" + cwd.slice(h.length) : cwd;
}

export default class Pane extends View {
  init() { this.hovered = false; this.width = null; }

  me() {
    return panes().find((p) => p.pane === props()?.pane);
  }

  button(t, id, icon, command, args) {
    return div()
      .id(id)
      .p(3)
      .rounded(4)
      .text_color(t.dim)
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => {
        cx.stop_propagation();
        run(command, args);
      })
      .child(svg(`mods/pane/${icon}.svg`).size(14));
  }

  render(cx) {
    const t = theme();
    const p = this.me();
    if (!p) return div();
    const name = (p.program?.agent || p.program?.shell) && !p.name && !p.auto_name ? p.program.name : p.label;
    const pic = p.icon ? own(p.icon, 13, t.dim) : icon(p.program, 14, t.dim);
    const st = p.status;
    const pane = p.pane;
    const textSize = p.text_size != null ? `${Math.round(p.text_size * 10) / 10}pt` : null;
    const extra = p.header_info
      ? info(t, p, infoRoom(this.width, [pic ? 14 : null, Math.min(260, estimate(name)), textSize ? estimate(textSize) : null, STATUS[st] ?? null], this.hovered))
      : null;
    return h_flex()
      .id(`pane-head-${pane}`)
      .size_full()
      .px(10)
      .gap(8)
      .items_center()
      .bg(t.background)
      .border_b(1)
      .border_color(t.border)
      .text_size(12)
      .on_hover((hovered, cx) => { this.hovered = hovered; cx.notify(); })
      .when(Boolean(p.header_info), (el) => el.on_size_change((size, cx) => { this.width = size.width; cx.notify(); }))
      .on_click(() => run("pane.focus", { pane }))
      // The pane's menu (the layout's terminal_menu), as a right-click on its terminal opens it, with no selection.
      .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); run("pane.menu", { pane }); })
      .when(Boolean(pic), (el) => el.child(pic))
      .child(div().flex_none().truncate().max_w(260).font_medium().text_color(t.foreground).child(name))
      .child(div().flex_1().truncate().text_color(t.dim).child(folder(p.cwd)))
      .when(Boolean(extra), el => el.child(extra))
      .when(p.text_size != null, el => el.child(div().id(`pane-text-size-${pane}`).flex_none().text_size(10.5).text_color(t.dim).child(textSize)))
      .when(st === "working", el => el.child(working(t)))
      .when(st === "blocked", el => el.child(div().flex_none().px(6).h(18).flex().items_center().rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).child("Needs you")))
      .when(st === "done", el => el.child(div().size(7).flex_none().rounded(4).bg(t.done)))
      .when(this.hovered, el => el
        .child(this.button(t, `ph-r-${pane}`, "split-right", "pane.split", { direction: "right", pane }))
        .child(this.button(t, `ph-d-${pane}`, "split-down", "pane.split", { direction: "down", pane }))
        .child(this.button(t, `ph-z-${pane}`, p.zoomed ? "unzoom" : "zoom", "pane.zoom", { pane }))
        .child(this.button(t, `ph-x-${pane}`, "close", "pane.close", { pane })));
  }
}
