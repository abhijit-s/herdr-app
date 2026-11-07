// The icon picker, opened by the `menu` mod's "Change icon…" with props { pane }
// or { tab_id }: a Nerd Font glyph, an emoji or the first letter of its name,
// kept with the pane or tab (pane.set_icon, tab.set_icon). "Clear icon" gives it
// its automatic icon again.
//
// Other mods open it for what they keep themselves with props { shared, id, name },
// and optionally { letter, now, extra }: the choice is written to shared(shared)[id]
// as { kind, value? } (Clear icon deletes it). `now` is the icon shown today
// ({ kind, text }); `extra` adds tiles of the mod's own kinds beside the letter
// ([{ kind, text, word }], `text` a glyph).
//
// Suggested is a short list; Glyphs and Emoji hold every name in names/ (read
// when the picker opens), and the search box looks through all of them.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, v_virtual_list, VirtualListScrollHandle, Scrollbar } from "gpui-base";
import { run, props, closeOverlay, theme, panes, spaces, shared, setShared, appHome, readTextFile } from "herdr";
import { parseGlyphs, parseEmoji, search, rows, setLabel, SETS } from "./search.js";

const GLYPH_FONT = "Symbols Nerd Font";
const NAMES = "mods/icon-picker/names";
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
const TABS = [["suggested", "Suggested"], ["glyph", "Glyphs"], ["emoji", "Emoji"], ["letter", "Letter"]];
const SUGGESTED = { glyph: "Suggested glyphs", emoji: "Suggested emoji" };
const PER_ROW = 10;
const CELL = 36;
const ROW_H = CELL + 4;
const HEAD_H = 24;

// As Herdr works it out for a pane (a tab's comes as its `letter`): the first letter or digit, upper case.
const letterOf = (name) => [...(name ?? "")].find((c) => /[\p{L}\p{N}]/u.test(c))?.toUpperCase() ?? "?";

// What the picker is for: { kind: "pane" | "tab" | "shared", id, name, icon, own }, null when it's gone.
function subject(p) {
  if (p?.shared) {
    const own = (shared(p.shared) ?? {})[p.id] ?? null;
    return { kind: "shared", key: p.shared, id: p.id, name: p.name ?? "", letter: p.letter, icon: p.now ?? null, own, extra: p.extra ?? [] };
  }
  if (p?.pane) {
    const pane = panes().find((x) => x.pane === p.pane);
    return pane && { kind: "pane", id: pane.pane, name: pane.label, icon: pane.icon, own: pane.own_icon, extra: [] };
  }
  const tab = spaces().flatMap((s) => s.tabs ?? []).find((c) => c.id === p?.tab_id);
  if (!tab) return null;
  const focused = panes().find((x) => x.tab === tab.id && x.focused) ?? panes().find((x) => x.tab === tab.id);
  return { kind: "tab", id: tab.id, name: tab.named ? tab.name : (focused?.label ?? tab.name), letter: tab.letter, icon: tab.icon, own: tab.own_icon, extra: [] };
}

function face(ic, size) {
  return div().text_size(size).when(ic.kind === "glyph", (el) => el.font_family(GLYPH_FONT)).when(ic.kind === "letter", (el) => el.font_bold()).child(ic.text);
}

// The row holding entry `at`, -1 when none does.
const rowOf = (rs, at) => rs.findIndex((r) => r.cells && at >= r.first && at < r.first + r.cells.length);

export default class IconPicker extends View {
  init(_props, cx) {
    this.tab = null;
    this.chip = { glyph: null, emoji: null };
    this.query = "";
    this.at = 0;
    this.glyphs = null;
    this.emoji = null;
    this.groups = [];
    this.named = new Map();
    this.cache = null;
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.input = InputState.new({ placeholder: "Search every glyph and emoji…" });
    this.input.on("change", (_e, cx) => this.find(this.input.value(), cx));
    this.input.on("submit", () => this.pickAt());
    this.input.focus();
    this.failed = false;
    cx.spawn(async (cx) => {
      const home = appHome();
      try {
        const [g, e] = await Promise.all([readTextFile(`${home}/${NAMES}/glyphs.json`), readTextFile(`${home}/${NAMES}/emoji.json`)]);
        this.loaded(g, e);
        this.failed = !g && !e;
      } catch {
        this.failed = true;
      }
      cx.notify();
    });
  }

  // The name files' text, once read.
  loaded(glyphs, emoji) {
    if (glyphs) this.glyphs = parseGlyphs(glyphs);
    if (emoji) ({ groups: this.groups, entries: this.emoji } = parseEmoji(emoji));
    for (const e of [...(this.glyphs ?? []), ...(this.emoji ?? [])]) if (!this.named.has(e.text)) this.named.set(e.text, e.name);
  }

  find(query, cx) {
    this.query = query;
    this.at = 0;
    this.scroll.scroll_to_item(0);
    cx.notify();
  }

  pick(s, choice) {
    if (s.kind === "pane") run("pane.set_icon", { pane: s.id, icon: choice });
    else if (s.kind === "tab") run("tab.set_icon", { id: s.id, icon: choice });
    else {
      const all = { ...(shared(s.key) ?? {}) };
      if (choice) all[s.id] = choice;
      else delete all[s.id];
      setShared(s.key, all);
    }
    closeOverlay(props()?.overlay);
  }

  pickAt() {
    const s = subject(props());
    const e = s && this.shown(s)?.entries[this.at];
    if (e) this.pick(s, { kind: e.kind, value: e.text });
  }

  tabOf(s) {
    return this.tab ?? (s.own?.kind === "letter" || s.extra.some((x) => x.kind === s.own?.kind) ? "letter" : "suggested");
  }

  // The tiles on show: { entries, rows }, kept until what decides them changes.
  shown(s) {
    const tab = this.tabOf(s);
    if (tab === "letter") return null;
    const chip = tab === "suggested" ? null : this.chip[tab];
    const query = this.query.trim();
    const key = [tab, chip, query, this.glyphs?.length, this.emoji?.length].join("\u0000");
    if (this.cache?.key === key) return this.cache;
    const all = tab === "glyph" ? (this.glyphs ?? []) : tab === "emoji" ? (this.emoji ?? []) : [...(this.glyphs ?? []), ...(this.emoji ?? [])];
    const entries = tab === "suggested" && !query
      ? [...GLYPHS.map((text) => this.suggested("glyph", text)), ...EMOJI.map((text) => this.suggested("emoji", text))]
      : search(all, query, chip);
    this.cache = { key, entries, rows: rows(entries, PER_ROW, !query && !chip) };
    return this.cache;
  }

  suggested(kind, text) {
    return { kind, text, name: this.named.get(text) ?? "", set: SUGGESTED[kind], keys: [], words: [] };
  }

  // Arrow keys over the grid: along a row, or to the same column a row up or down.
  move(key, view) {
    const rs = view.rows;
    const at = rowOf(rs, this.at);
    if (at < 0) return;
    if (key === "left") this.at = Math.max(0, this.at - 1);
    else if (key === "right") this.at = Math.min(view.entries.length - 1, this.at + 1);
    else {
      const step = key === "down" ? 1 : -1;
      let to = at + step;
      while (rs[to] && !rs[to].cells) to += step;
      if (!rs[to]) return;
      this.at = rs[to].first + Math.min(this.at - rs[at].first, rs[to].cells.length - 1);
    }
    const row = rowOf(rs, this.at);
    this.scroll.scroll_to_item(row > 0 && !rs[row - 1].cells ? row - 1 : row);
  }

  render() {
    const t = theme();
    const s = subject(props());
    if (!s) return div().id("icon-picker").p(16).bg(t.popover).text_color(t.dim).child(props()?.shared ? "That one is gone." : "That pane or tab is gone.");
    const own = s.own;
    const tab = this.tabOf(s);
    const view = this.shown(s);
    if (view && this.at >= view.entries.length) this.at = Math.max(0, view.entries.length - 1);
    const sel = view?.entries[this.at];
    const tile = (key, text, kind, on, choice, selected = false, tip = null) => h_flex().id(key).size(CELL).flex_none().rounded(7).items_center().justify_center().cursor_pointer()
      .bg(selected ? t.selected : t.raised).border(on ? 2 : 1).border_color(on ? t.ring : t.border).text_color(t.foreground)
      .hover((el) => el.opacity(0.8))
      .when(Boolean(tip), (el) => el.tooltip(tip))
      .on_click(() => this.pick(s, choice))
      .child(face({ kind, text }, kind === "letter" ? 16 : 17));
    const cell = (e, i) => tile(`icon-${e.kind}-${i}`, e.text, e.kind, own?.kind === e.kind && own.value === e.text, { kind: e.kind, value: e.text },
      i === this.at, e.name && `${e.name} · ${e.kind === "glyph" ? setLabel(e.set) : e.set}`);
    const head = (set) => div().h(HEAD_H).pt(6).text_size(11).font_semibold().text_color(t.dim).child(SETS.some(([c]) => c === set) ? setLabel(set) : set);
    const body = !view
      ? v_flex().id("icon-letter-tab").flex_1().gap(10)
        .child(h_flex().gap(12).items_center()
          .child(tile("icon-letter", s.letter ?? letterOf(s.name), "letter", own?.kind === "letter", { kind: "letter" }))
          .child(div().text_color(t.muted).child("Its first letter")))
        .children(s.extra.map((x) => h_flex().gap(12).items_center()
          .child(tile(`icon-${x.kind}`, x.text, "glyph", own?.kind === x.kind, { kind: x.kind }))
          .child(div().text_color(t.muted).child(x.word))))
      : view.entries.length
        ? div().id("icon-grid-box").relative().flex_1().min_h(0)
          .child(v_virtual_list("icon-grid", view.rows.length, view.rows.map((r) => (r.cells ? ROW_H : HEAD_H)),
            (i) => (view.rows[i].cells ? `r:${view.rows[i].first}` : `h:${view.rows[i].head}`),
            (range) => view.rows.slice(range.start, range.end).map((r) => (r.cells ? h_flex().h(ROW_H).gap(4).children(r.cells.map((e, k) => cell(e, r.first + k))) : head(r.head))))
            .track_scroll(this.scroll).size_full())
          .child(Scrollbar.vertical("icon-grid").absolute().inset_0())
        : div().id("icon-none").flex_1().py(24).text_color(t.dim).child(this.glyphs || this.emoji ? "Nothing by that name." : this.failed ? "Couldn't read the icon names." : "Reading names…");
    const sets = tab === "glyph" ? SETS.map(([c, word]) => [c, word]) : tab === "emoji" ? this.groups.map((g) => [g, g]) : null;
    return v_flex().id("icon-picker").track_focus(this.handle).size_full().p(14).gap(8).bg(t.popover).text_size(12.8).text_color(t.foreground)
      .on_key_down((e, cx) => {
        if (e.key === "escape") closeOverlay(props()?.overlay);
        else if (e.key === "enter") this.pickAt();
        else if (view && (e.key === "up" || e.key === "down" || ((e.key === "left" || e.key === "right") && !this.query))) this.move(e.key, view);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(h_flex().gap(10).items_center()
        .child(div().flex_1().truncate().font_bold().child(`Icon for ${s.name}`))
        .child(div().text_size(11.5).text_color(t.dim).child(own ? "Chosen" : "Automatic")))
      .child(h_flex().h(30).flex_none().px(8).items_center().rounded(8).border_1().border_color(t.border).bg(t.background)
        .child(Input.new(this.input).flex_1().h_full().text_size(12.5).text_color(t.foreground)))
      .child(h_flex().gap(4).flex_none().children(TABS.map(([k, word]) => div().id(`icon-tab-${k}`).px(10).py(3).rounded(6).cursor_pointer()
        .when(k === tab, (el) => el.bg(t.selected).text_color(t.foreground))
        .when(k !== tab, (el) => el.text_color(t.muted).hover((h) => h.bg(t.hover)))
        .on_click((_e, cx) => { this.tab = k; this.find(this.query, cx); })
        .child(word))))
      .when(Boolean(sets), (el) => el.child(h_flex().id("icon-sets").gap(4).flex_none().overflow_x_scroll()
        .children([[null, "All"], ...sets].map(([c, word]) => div().id(`icon-set-${c ?? "all"}`).flex_none().px(8).py(2).rounded(10).text_size(11).cursor_pointer()
          .when(c === this.chip[tab], (x) => x.bg(t.selected).text_color(t.foreground))
          .when(c !== this.chip[tab], (x) => x.text_color(t.muted).hover((h) => h.bg(t.hover)))
          .on_click((_e, cx) => { this.chip[tab] = c; this.find(this.query, cx); })
          .child(word)))))
      .child(body)
      .child(h_flex().gap(8).items_center().flex_none()
        .child(div().text_size(11.5).text_color(t.dim).child("Now: "))
        .when(Boolean(s.icon), (el) => el.child(face(s.icon, 14)))
        .when(!s.icon, (el) => el.child(div().text_size(11.5).text_color(t.dim).child(s.kind === "pane" ? "its program's" : s.kind === "tab" ? "its pane's" : "automatic")))
        .child(div().flex_1().truncate().text_size(11.5).text_color(t.dim).child(sel?.name ?? ""))
        .when(Boolean(own), (el) => el.child(div().id("icon-clear").px(10).py(4).rounded(6).border(1).border_color(t.border).cursor_pointer()
          .hover((h) => h.bg(t.hover))
          .on_click(() => this.pick(s, null))
          .child("Clear icon"))));
  }
}
