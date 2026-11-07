// The icon picker for a pane or a tab, opened by the `menu` mod's "Change icon…"
// with props { pane } or { tab_id }: a Nerd Font glyph, an emoji or the first
// letter of its name, kept with the pane or tab (pane.set_icon, tab.set_icon).
// "Clear icon" gives it its automatic icon again. The sets are the ones space
// avatars pick from, so a pane can wear its space's icon.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run, props, closeOverlay, theme, panes, spaces } from "herdr";

const GLYPH_FONT = "Symbols Nerd Font";
// Nerd Font code points, one colour each (none reads as a status).
const GLYPHS = [
  0xf135, 0xf06c, 0xf0c3, 0xf13d, 0xf140b, 0xf1bb, 0xf0176, 0xf1b0, 0xf219, 0xf001,
  0xf072, 0xf206, 0xf14e, 0xf1b2, 0xf084, 0xf0f65, 0xf0717, 0xf0e9, 0xf076, 0xf12e,
  0xf091, 0xf06b, 0xf030, 0xedf7, 0xee41, 0xedea, 0xeef8, 0xeefe, 0xee0d, 0xef01,
  0xedeb, 0xedec, 0xe25f, 0xefcc, 0xef4e, 0xf025, 0xef5f, 0xee99, 0xed7d, 0xeda7,
  0xef08, 0xef23, 0xf21a, 0xf238, 0xedfd, 0xf011a, 0xf09ff, 0xeeff, 0xf0b4e, 0xedf8,
  0xeeed, 0xef0a, 0xf03d2, 0xedff, 0xef3b, 0xef8c, 0xf0db5, 0xf07df, 0xf0a26, 0xf0ec8,
].map((code) => String.fromCodePoint(code));
// Single-code-point emoji with emoji presentation of their own.
const EMOJI = [
  "🐙", "🦀", "🐢", "🦋", "🐝", "🐞", "🦉", "🐧", "🐳", "🦒",
  "🦓", "🦔", "🦦", "🦩", "🐌", "🦜", "🦕", "🐘", "🦘", "🦈",
  "🌵", "🌻", "🌲", "🍀", "🌷", "🍄", "🌴", "🌸", "🍁", "🌾",
  "🍉", "🍋", "🍓", "🍒", "🥑", "🥕", "🌽", "🍕", "🍩", "🧁",
  "🌮", "🍣", "🥐", "🧀", "🍇", "🚀", "⚓", "🎈", "🎨", "🎸",
  "🎲", "🔭", "🧭", "💎", "🔑", "🧩", "🎁", "🧪", "⛵", "🪐",
];
const TABS = [["glyph", "Glyph"], ["emoji", "Emoji"], ["letter", "Letter"]];

// As Herdr works it out for a pane (a tab's comes as its `letter`): the first letter or digit, upper case.
const letterOf = (name) => [...(name ?? "")].find((c) => /[\p{L}\p{N}]/u.test(c))?.toUpperCase() ?? "?";

// What the picker is for: { kind: "pane" | "tab", id, name, icon, own }, null when it's gone.
function subject(p) {
  if (p?.pane) {
    const pane = panes().find((x) => x.pane === p.pane);
    return pane && { kind: "pane", id: pane.pane, name: pane.label, icon: pane.icon, own: pane.own_icon };
  }
  const tab = spaces().flatMap((s) => s.tabs ?? []).find((c) => c.id === p?.tab_id);
  if (!tab) return null;
  const focused = panes().find((x) => x.tab === tab.id && x.focused) ?? panes().find((x) => x.tab === tab.id);
  return { kind: "tab", id: tab.id, name: tab.named ? tab.name : (focused?.label ?? tab.name), letter: tab.letter, icon: tab.icon, own: tab.own_icon };
}

function face(ic, size) {
  return div().text_size(size).when(ic.kind === "glyph", (el) => el.font_family(GLYPH_FONT)).when(ic.kind === "letter", (el) => el.font_bold()).child(ic.text);
}

export default class IconPicker extends View {
  init(_props, cx) {
    this.tab = null;
    this.handle = cx.focus_handle();
    this.handle.focus();
  }

  pick(s, choice) {
    if (s.kind === "pane") run("pane.set_icon", { pane: s.id, icon: choice });
    else run("tab.set_icon", { id: s.id, icon: choice });
    closeOverlay(props()?.overlay);
  }

  render() {
    const t = theme();
    const s = subject(props());
    if (!s) return div().id("icon-picker").p(16).bg(t.popover).text_color(t.dim).child("That pane or tab is gone.");
    const own = s.own;
    const tab = this.tab ?? (TABS.find(([k]) => k === own?.kind)?.[0] ?? "glyph");
    const tile = (key, text, kind, on, choice) => h_flex().id(key).size(34).flex_none().rounded(7).items_center().justify_center().cursor_pointer()
      .bg(t.raised).border(on ? 2 : 1).border_color(on ? t.ring : t.border).text_color(t.foreground)
      .hover((el) => el.opacity(0.8))
      .on_click(() => this.pick(s, choice))
      .child(face({ kind, text }, kind === "letter" ? 16 : 17));
    const grid = (all, kind) => h_flex().flex_wrap().gap(6).children(all.map((text, i) => tile(`icon-${kind}-${i}`, text, kind, own?.kind === kind && own.value === text, { kind, value: text })));
    const body = tab === "glyph" ? grid(GLYPHS, "glyph")
      : tab === "emoji" ? grid(EMOJI, "emoji")
      : h_flex().gap(12).items_center()
        .child(tile("icon-letter", s.letter ?? letterOf(s.name), "letter", own?.kind === "letter", { kind: "letter" }))
        .child(div().text_color(t.muted).child("Its first letter"));
    return v_flex().id("icon-picker").track_focus(this.handle).size_full().p(14).gap(12).bg(t.popover).text_size(12.8).text_color(t.foreground)
      .on_key_down((e) => { if (e.key === "escape") closeOverlay(props()?.overlay); })
      .child(h_flex().gap(10).items_center()
        .child(div().flex_1().truncate().font_bold().child(`Icon for ${s.name}`))
        .child(div().text_size(11.5).text_color(t.dim).child(own ? "Chosen" : "Automatic")))
      .child(h_flex().gap(4).children(TABS.map(([k, word]) => div().id(`icon-tab-${k}`).px(10).py(4).rounded(6).cursor_pointer()
        .when(k === tab, (el) => el.bg(t.selected).text_color(t.foreground))
        .when(k !== tab, (el) => el.text_color(t.muted).hover((h) => h.bg(t.hover)))
        .on_click((_e, cx) => { this.tab = k; cx.notify(); })
        .child(word))))
      .child(div().flex_1().min_h(0).overflow_y_scrollbar().child(body))
      .child(h_flex().gap(10).items_center()
        .child(div().text_size(11.5).text_color(t.dim).child("Now: "))
        .when(Boolean(s.icon), (el) => el.child(face(s.icon, 14)))
        .when(!s.icon, (el) => el.child(div().text_size(11.5).text_color(t.dim).child(s.kind === "pane" ? "its program's" : "its pane's")))
        .child(div().flex_1())
        .when(Boolean(own), (el) => el.child(div().id("icon-clear").px(10).py(5).rounded(6).border(1).border_color(t.border).cursor_pointer()
          .hover((h) => h.bg(t.hover))
          .on_click(() => this.pick(s, null))
          .child("Clear icon"))));
  }
}
