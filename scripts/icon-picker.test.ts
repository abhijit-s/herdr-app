import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import * as catalogue from "../defaults/mods/icon-picker/search.js";
import { parseEmoji, parseGlyphs, rows, search } from "../defaults/mods/icon-picker/search.js";

const dir = new URL("../defaults/mods/icon-picker/", import.meta.url);
const glyphText = readFileSync(new URL("names/glyphs.json", dir), "utf8");
const emojiText = readFileSync(new URL("names/emoji.json", dir), "utf8");
const glyphs = parseGlyphs(glyphText);
const emoji = parseEmoji(emojiText);
const everything = [...glyphs, ...emoji.entries];

test("the name files hold every glyph and emoji, one row each", () => {
  expect(glyphs.length).toBeGreaterThan(10000);
  expect(new Set(glyphs.map((g) => g.text)).size).toBe(glyphs.length);
  expect(emoji.entries.length).toBeGreaterThan(1800);
  expect(new Set(emoji.entries.map((e) => e.text)).size).toBe(emoji.entries.length);
  // No skin tone variants: those are picked from their base emoji.
  expect(emoji.entries.some((e) => /[\u{1F3FB}-\u{1F3FF}]/u.test(e.text))).toBe(false);
  expect(emoji.groups).not.toContain("Component");
  // Every value is one a set_icon command takes: at most 32 bytes, no spaces or controls.
  for (const e of everything) {
    expect(new TextEncoder().encode(e.text).length).toBeLessThanOrEqual(32);
    expect(/[\s\p{Cc}]/u.test(e.text)).toBe(false);
  }
});

test("search ranks exact names, then prefixes, then the rest", () => {
  const names = (q: string, set: string | null = null) => search(everything, q, set).map((e) => e.name);
  const rocket = names("rocket");
  // Every set's rocket, and the emoji, before rocket_launch and before space_rocket.
  const exact = rocket.slice(0, 3).sort();
  expect(exact).toEqual(["fa-rocket", "md-rocket", "rocket"]);
  expect(rocket.indexOf("md-rocket_launch")).toBeGreaterThan(2);
  const prefix = rocket.findIndex((n) => n.endsWith("rocket_launch"));
  const inner = rocket.findIndex((n) => /-.+_rocket/.test(n));
  expect(inner < 0 || prefix < inner).toBe(true);
  expect(search(everything, "rocket")[2].text).toBeDefined();
  expect(search(everything, "rocket").find((e) => e.kind === "emoji")?.text).toBe("🚀");
  // Prefix and hyphenated or spaced queries find names written with underscores.
  expect(names("md-rocket")[0]).toBe("md-rocket");
  expect(names("partly cloudy").every((n) => n.includes("partly_cloudy") || n.includes("partly cloudy"))).toBe(true);
  // A keyword finds an emoji whose name doesn't hold it.
  expect(search(emoji.entries, "launch").map((e) => e.text)).toContain("🚀");
  expect(names("zzzz-nothing")).toEqual([]);
  // The empty query keeps everything, in order.
  expect(search(glyphs, "")).toBe(glyphs);
});

test("set chips keep one set: Powerline holds pl and ple", () => {
  expect(search(glyphs, "", "fa").every((e) => e.set === "fa")).toBe(true);
  expect(search(glyphs, "rocket", "md").map((e) => e.name)[0]).toBe("md-rocket");
  const powerline = search(glyphs, "", "pl").map((e) => e.name);
  expect(powerline.some((n) => n.startsWith("pl-")) && powerline.some((n) => n.startsWith("ple-"))).toBe(true);
  expect(search(emoji.entries, "", "Flags").length).toBeGreaterThan(200);
  // Every glyph prefix in the file has a chip.
  expect(glyphs.filter((g) => !catalogue.SETS.some(([c]) => c === g.set))).toEqual([]);
});

test("rows break at each set and carry a heading when asked", () => {
  const sample = [...search(glyphs, "", "pom"), ...search(glyphs, "", "iec")];
  const headed = rows(sample, 10, true);
  expect(headed.filter((r: any) => r.head).map((r: any) => r.head)).toEqual(["pom", "iec"]);
  expect(headed.flatMap((r: any) => r.cells ?? []).length).toBe(sample.length);
  expect(rows(sample, 10, false).every((r: any) => r.cells)).toBe(true);
});

// The view, without a window: elements record their calls.
class Element {
  nodes: Element[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const element = () => new Element();
let built = 0;
let visible = { start: 0, end: 6 };
const effects: unknown[][] = [];
let store: Record<string, any> = {};
let current: Record<string, any> = {};
const theme = new Proxy({}, { get: (_t, k) => String(k) });
const pane = { pane: "p1", label: "Build", icon: null, own_icon: null };
const values: Record<string, any> = {
  View: class {}, div: element, h_flex: element, v_flex: element,
  Input: { new: element }, InputState: { new: () => ({ on() {}, focus() {}, value: () => "" }) },
  VirtualListScrollHandle: { new: () => ({ scroll_to_item(i: number) { effects.push(["scroll", i]); } }) },
  Scrollbar: { vertical: element },
  v_virtual_list: (id: string, count: number, sizes: number[], key: (i: number) => string, render: (r: any) => any[]) => {
    const list = element();
    (list as any).calls.v_virtual_list = [id, count, sizes, key];
    const range = { start: visible.start, end: Math.min(visible.end, count) };
    for (const row of render(range)) { list.nodes.push(row); built++; }
    return list;
  },
  run: (...args: unknown[]) => effects.push(["run", ...args]),
  props: () => current, closeOverlay: (id: string) => effects.push(["close", id]), theme: () => theme,
  panes: () => [pane], spaces: () => [{ id: "s1", tabs: [{ id: "tab1", name: "Two", named: true, letter: "T", icon: null, own_icon: null }] }],
  shared: (k: string) => store[k], setShared: (k: string, v: unknown) => { store[k] = v; },
  appHome: () => "/app", readTextFile: async () => null,
  ...catalogue,
};
const source = readFileSync(new URL("main.js", dir), "utf8");
const Picker = new Function(...Object.keys(values), source.replace(/^import [^;]*;\n/gm, "").replace("export default class IconPicker", "return class IconPicker"))(...Object.values(values));
const cx = { notify() {}, stop_propagation() {}, focus_handle() {}, spawn() {} };
function open(p: Record<string, any>, loaded = true) {
  current = { overlay: "o1", ...p };
  effects.length = 0;
  const view = new Picker();
  view.init(current, cx);
  if (loaded) view.loaded(glyphText, emojiText);
  return view;
}
function find(root: Element, test: (e: Element) => boolean): Element[] {
  const out = test(root) ? [root] : [];
  for (const child of root?.nodes ?? []) if (child instanceof Object) out.push(...find(child, test));
  return out;
}
const ids = (root: Element, prefix: string) => find(root, (e) => String(e?.calls?.id?.[0] ?? "").startsWith(prefix)).map((e) => e.calls.id[0]);
const grid = (root: Element) => find(root, (e) => Boolean(e?.calls?.v_virtual_list))[0];

test("Suggested is the default tab: the 60 glyphs and 60 emoji, named once read", () => {
  const view = open({ pane: "p1" });
  visible = { start: 0, end: 100 };
  const tree = view.render();
  expect(ids(tree, "icon-glyph-").length).toBe(60);
  expect(ids(tree, "icon-emoji-").length).toBe(60);
  expect(view.shown({ own: null, extra: [] }).entries[0].name).toBe("fa-rocket");
  // Before the names are read, the tiles are there without them.
  const early = open({ pane: "p1" }, false).render();
  expect(ids(early, "icon-glyph-").length).toBe(60);
});

test("the grid builds only the rows on screen", () => {
  const view = open({ pane: "p1" });
  view.tab = "glyph";
  visible = { start: 0, end: 6 };
  built = 0;
  const tree = view.render();
  const list = grid(tree);
  expect(list.calls.v_virtual_list[1]).toBeGreaterThan(1000);
  expect(built).toBe(6);
  expect(ids(tree, "icon-glyph-").length).toBeLessThanOrEqual(60);
  visible = { start: 0, end: 100 };
});

test("typing searches every set; Enter picks the first, arrows move by row", () => {
  const view = open({ pane: "p1" });
  view.find("rocket", cx);
  const tree = view.render();
  const first = view.shown({ own: null, extra: [] }).entries.slice(0, 3).map((e: any) => e.name).sort();
  expect(first).toEqual(["fa-rocket", "md-rocket", "rocket"]);
  expect(find(tree, (e) => e?.calls?.tooltip?.[0] === "fa-rocket · Font Awesome").length).toBe(1);
  const keys = tree.calls.on_key_down[0];
  keys({ key: "down" }, cx);
  expect(view.at).toBe(10);
  keys({ key: "up" }, cx);
  // While there's a query, left and right move the search box's cursor.
  keys({ key: "right" }, cx);
  expect(view.at).toBe(0);
  effects.length = 0;
  keys({ key: "enter" }, cx);
  const picked = view.shown({ own: null, extra: [] }).entries[0];
  expect(effects).toEqual([["run", "pane.set_icon", { pane: "p1", icon: { kind: picked.kind, value: picked.text } }], ["close", "o1"]]);
});

test("a picked glyph or emoji goes to its pane, tab or shared map; Clear icon removes it", () => {
  const glyph = { kind: "glyph", value: String.fromCodePoint(0xf0463) };
  const flag = { kind: "emoji", value: "🏳️‍🌈" };
  open({ pane: "p1" }).pick({ kind: "pane", id: "p1" }, glyph);
  expect(effects[0]).toEqual(["run", "pane.set_icon", { pane: "p1", icon: glyph }]);
  open({ tab_id: "tab1" }).pick({ kind: "tab", id: "tab1" }, flag);
  expect(effects[0]).toEqual(["run", "tab.set_icon", { id: "tab1", icon: flag }]);
  store = { "space.icons": { s0: { kind: "letter" } } };
  const view = open({ shared: "space.icons", id: "s1", name: "herdr" });
  expect(view.render().calls.id[0]).toBe("icon-picker");
  view.pickAt();
  expect(store["space.icons"]).toEqual({ s0: { kind: "letter" }, s1: { kind: "glyph", value: String.fromCodePoint(0xf135) } });
  expect(effects).toEqual([["close", "o1"]]);
  const again = open({ shared: "space.icons", id: "s1", name: "herdr" });
  expect(find(again.render(), (e) => e?.calls?.id?.[0] === "icon-clear").length).toBe(1);
  again.pick({ kind: "shared", key: "space.icons", id: "s1" }, null);
  expect(store["space.icons"]).toEqual({ s0: { kind: "letter" } });
});

test("with no query, left and right move along the grid", () => {
  const view = open({ pane: "p1" });
  view.tab = "glyph";
  const keys = view.render().calls.on_key_down[0];
  keys({ key: "right" }, cx);
  keys({ key: "right" }, cx);
  keys({ key: "left" }, cx);
  expect(view.at).toBe(1);
});

test("a shared space shows the icon it wears now, and says so when the names can't be read", () => {
  store = { "space.icons": { s1: { kind: "emoji", value: "🐙" } } };
  const view = open({ shared: "space.icons", id: "s1", name: "herdr", now: { kind: "emoji", text: "🐙" } });
  expect(find(view.render(), (e) => e?.nodes?.includes("🐙")).length).toBeGreaterThan(0);
  const broken = open({ pane: "p1" }, false);
  broken.tab = "glyph";
  broken.failed = true;
  expect(find(broken.render(), (e) => e?.nodes?.includes("Couldn't read the icon names.")).length).toBe(1);
});

test("a shared target's own kinds show beside its letter, and open there when chosen", () => {
  store = { "space.icons": { s1: { kind: "repo" } } };
  const view = open({ shared: "space.icons", id: "s1", name: "herdr", extra: [{ kind: "repo", text: "\u{e7a8}", word: "Rust" }] });
  const tree = view.render();
  expect(ids(tree, "icon-repo")).toEqual(["icon-repo"]);
  expect(ids(tree, "icon-letter").filter((i) => i !== "icon-letter-tab")).toEqual(["icon-letter"]);
  expect(find(tree, (e) => e?.calls?.id?.[0] === "icon-repo")[0].calls.border[0]).toBe(2);
});

test("perf: parse and per-keystroke search over everything", () => {
  let t = performance.now();
  for (let i = 0; i < 5; i++) { parseGlyphs(glyphText); parseEmoji(emojiText); }
  const parse = (performance.now() - t) / 5;
  const queries = ["r", "ro", "roc", "rock", "rocke", "rocket", "a", "ar", "arr", "arro", "arrow"];
  t = performance.now();
  for (const q of queries) search(everything, q);
  const perKey = (performance.now() - t) / queries.length;
  console.log(`bun: parse ${parse.toFixed(1)} ms, search ${perKey.toFixed(2)} ms per keystroke`);
  expect(perKey).toBeLessThan(16);
});

test("keywords.json: each word once, every icon one the name files hold", () => {
  const { icons } = JSON.parse(readFileSync(new URL("names/keywords.json", dir), "utf8"));
  const glyphNames = new Set(glyphs.map((g) => g.name));
  const emojiTexts = new Set(emoji.entries.map((e) => e.text));
  const words = icons.flatMap((i: { words: string[] }) => i.words);
  expect(icons.length).toBeGreaterThan(40);
  expect(new Set(words).size).toBe(words.length);
  for (const w of words) expect(w).toMatch(/^[a-z0-9]+$/);
  for (const i of icons) {
    expect(glyphNames.has(i.glyph)).toBe(true);
    expect(emojiTexts.has(i.emoji)).toBe(true);
  }
});
