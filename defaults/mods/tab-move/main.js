// Moves a tab, whole, to another space on its machine (tab.move_to_space). Opened from the
// tab menu's "Move to space…" with props.id (the tab), else on the shown tab. It lists the
// other spaces of the tab's machine: a tab's terminals stay on their own machine.
// Keys: 1-9 picks a shown row, j/k or the arrows move, Enter picks, Esc closes; the
// list scrolls past nine. / or any other printable key searches the spaces by name; while
// searching every character types, and only the arrows, Enter, backspace and Esc act (Esc
// clears the query, then ends the search; backspace on an empty query ends it).
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { spaces, props, run, closeOverlay, theme } from "herdr";

export const ROWS = 9;

// The tab to move and its space: the one props names, else the shown one.
export function chosenTab(p, all = spaces()) {
  for (const space of p?.id ? all : all.filter((s) => s.active)) {
    const tab = (space.tabs ?? []).find((t) => (p?.id ? t.id === p.id : t.active));
    if (tab) return { space, tab };
  }
  return null;
}

// Where it can go: every other space on its space's machine, in the list's order.
export function targets(own, all = spaces()) {
  return own ? all.filter((s) => s.id !== own.id && s.machine === own.machine) : [];
}

// How well a query matches: 0 at the start, 1 inside, 2 as a scattered subsequence, null not at all.
export function matchRank(text, query) {
  const s = text.toLowerCase();
  const q = query.toLowerCase();
  const at = s.indexOf(q);
  if (at >= 0) return at === 0 ? 0 : 1;
  let i = 0;
  for (let j = 0; j < s.length && i < q.length; j++) if (s[j] === q[i]) i++;
  return i === q.length ? 2 : null;
}

// The spaces a query names, better matches first; sort is stable, so ties keep the list's order.
export function searchRows(rows, query) {
  return rows.map((space) => ({ space, rank: matchRank(space.name, query) }))
    .filter((r) => r.rank !== null).sort((a, b) => a.rank - b.rank).map((r) => r.space);
}

export const tabName = (t) => t.name || `Tab ${t.index}`;
const count = (n, what) => `${n} ${what}${n === 1 ? "" : "s"}`;

// A key that types into the query: one printable character with no command chord held.
export function typed(e) {
  const c = e.key_char;
  const m = e.modifiers ?? {};
  return c && [...c].length === 1 && c >= " " && c !== "\u007f" && !m.control && !m.platform && !m.alt ? c : null;
}

export default class TabMove extends View {
  init(_props, cx) {
    const chosen = chosenTab(props());
    this.tab = chosen?.tab ?? null;
    this.spaces = targets(chosen?.space);
    this.query = null; // null: not searching; "" while searching before anything is typed
    this.results = null;
    this.at = 0;
    this.top = 0;
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.timer.after(300, () => this.handle.focus());
  }

  close() { closeOverlay(props().overlay); }

  rows() { return this.query ? this.results : this.spaces; }

  // Moves the cursor, scrolling so it stays among the ROWS shown.
  go(at) {
    this.at = Math.max(0, Math.min(at, this.rows().length - 1));
    this.top = Math.min(Math.max(this.top, this.at - ROWS + 1), this.at);
  }

  // Filtering happens here, once per edit, never in render.
  search(query) {
    this.query = query;
    this.results = query ? searchRows(this.spaces, query) : null;
    this.at = this.top = 0;
  }

  pick(space) {
    if (!space || !this.tab) return;
    run("tab.move_to_space", { id: this.tab.id, space: space.id });
    this.close();
  }

  // Keys while searching; false lets the key through.
  searchKey(k, e) {
    const c = typed(e);
    if (k === "escape") {
      if (this.query) this.search("");
      else this.search(null);
    } else if (k === "backspace") this.search(this.query ? this.query.slice(0, -1) : null);
    else if (k === "down") this.go(this.at + 1);
    else if (k === "up") this.go(this.at - 1);
    else if (k === "enter") this.pick(this.rows()[this.at]);
    else if (c) this.search(this.query + c);
    else return false;
    return true;
  }

  navKey(k, e) {
    const c = typed(e);
    if (k === "escape") this.close();
    else if (/^[1-9]$/.test(k)) this.pick(this.rows()[this.top + Number(k) - 1]);
    else if (k === "j" || k === "down") this.go(this.at + 1);
    else if (k === "k" || k === "up") this.go(this.at - 1);
    else if (k === "enter") this.pick(this.rows()[this.at]);
    else if (c === "/") this.search("");
    // Navigation keys are taken above, so any other character starts a search; a space doesn't, since nothing would seem to happen.
    else if (c && c !== " ") this.search(c);
    else return false;
    return true;
  }

  key(e, cx) {
    const k = e.keystroke;
    if (!(this.query === null ? this.navKey(k, e) : this.searchKey(k, e))) return;
    cx.stop_propagation();
    cx.notify();
  }

  row(t, space, i) {
    const at = this.top + i;
    return h_flex().id(`tm-${at}`).px(8).h(28).gap(10).items_center().rounded(6)
      .when(at === this.at, (el) => el.bg(t.hover))
      .cursor_pointer().on_click(() => this.pick(space))
      .child(div().w(14).text_color(t.dim).child(this.query === null ? String(i + 1) : "")) // digits type while searching
      .child(div().flex_1().truncate().text_color(t.foreground).child(space.name))
      .child(div().text_size(11).text_color(t.dim).child(count((space.tabs ?? []).length, "tab")));
  }

  list(t) {
    const rows = this.rows();
    if (!rows.length) return div().px(8).text_color(t.dim).child(this.query ? `No space matches "${this.query}"` : "No other space on this machine.");
    const shown = rows.slice(this.top, this.top + ROWS);
    const more = (n, dir) => (n > 0 ? [div().px(8).text_size(11).text_color(t.dim).child(`${dir} ${n} more`)] : []);
    return v_flex().gap(2)
      .children(more(this.top, "↑"))
      .children(shown.map((s, i) => this.row(t, s, i)))
      .children(more(rows.length - this.top - shown.length, "↓"));
  }

  field(t) {
    const searching = this.query !== null;
    return h_flex().h(28).px(10).items_center().rounded(6).border_1().bg(t.background)
      .border_color(searching ? t.accent : t.border)
      .child(this.query
        ? div().flex_1().truncate().text_color(t.foreground).child(`${this.query}▏`)
        : div().flex_1().truncate().text_color(t.dim).child(`${searching ? "▏" : ""}Type to search spaces`));
  }

  hint() {
    if (this.query !== null) return "↑↓ move · Enter moves it there · Esc clears, then stops searching";
    return "1-9 or Enter moves it there · / or type to search · Esc closes";
  }

  render() {
    const t = theme();
    const body = !this.tab ? div().text_color(t.dim).child("No tab to move.")
      : v_flex().gap(8).child(this.field(t)).child(this.list(t));
    return v_flex().id("tab-move").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .w_full().p(16).gap(10).bg(t.popover).text_color(t.foreground).text_size(12.5)
      .child(h_flex().items_center()
        .child(div().flex_1().text_size(15).font_semibold().truncate().child(`Move ${this.tab ? tabName(this.tab) : "tab"} to a space`))
        .child(div().text_size(11).text_color(t.dim).child("Esc")))
      .child(body)
      .child(div().text_size(11.5).text_color(t.dim).child(this.hint()));
  }
}
