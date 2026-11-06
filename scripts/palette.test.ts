import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped palette's handler with the pixel deltas from GPUI.
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
const source = await readFile(new URL("../defaults/frame/palette/main.js", import.meta.url), "utf8");
const element = () => new Element();
let platform = "linux";
const icons: string[] = [];
let actions: any[] = [];
let bound: any[] = [];
const ran: [string, any][] = [];
const store = new Map<string, string>();
const values = {
  View: class {}, div: element, svg: (path: string) => { icons.push(path); return element(); }, v_flex: element, h_flex: element, Input: { new: element }, theme: () => ({}), keys: () => bound, win: () => ({ platform }),
  commands: () => [{ name: "tab.new", args: [], about: "" }, { name: "mod.open", args: [{ name: "name", required: true }, { name: "props", required: false }], about: "" }],
  layouts: () => [], modActions: () => actions, run: (name: string, args: any) => { ran.push([name, args]); }, props: () => ({ overlay: "palette" }), closeOverlay: () => {}, shared: () => null,
  localStorage: { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => { store.set(k, v); } },
};
const Palette = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Palette", "return class Palette"))(...Object.values(values));
function find(root: Element, id: string): Element | undefined {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}

test("palette follows small pixel movements and converted wheel deltas", () => {
  for (const delta of [-1, -3.9, -4, -48]) {
    const view = new Palette();
    view.query = "command"; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => Array.from({ length: 50 }, (_, i) => ({ e: { name: "test", title: `Command ${i}`, args: {} } }));
    const list = find(view.render(), "palette-rows");
    expect(list).toBeDefined();
    list!.calls.on_scroll_wheel[0]({ delta: { y: delta } }, { notify() {} });
    expect(view.scroll).toBe(-delta);
  }
});

test("palette input shows the command key only on macos", () => {
  for (const [os, icon] of [["macos", "frame/palette/command.svg"], ["windows", "mods/workspace-chrome/search.svg"], ["linux", "mods/workspace-chrome/search.svg"]]) {
    platform = os; icons.length = 0;
    const view = new Palette();
    view.query = ""; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => [];
    view.render();
    expect(icons).toEqual([icon]);
  }
});

test("palette scrolls past group titles with small movements", () => {
  for (const delta of [-3, -16, -25]) {
    const view = new Palette();
    view.query = ""; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => Array.from({ length: 30 }, (_, i) => ({ head: `Group ${Math.floor(i / 6)}`, e: { name: "test", title: `Command ${i}`, args: {} } }));
    for (let step = 0; step < 400 && (step === 0 || view.scroll < view.maxScroll); step++) {
      const list = find(view.render(), "palette-rows");
      const before = view.scroll;
      list!.calls.on_scroll_wheel[0]({ delta: { y: delta } }, { notify() {} });
      view.render();
      expect(view.scroll).toBe(Math.min(before - delta, view.maxScroll));
    }
    expect(view.scroll).toBe(view.maxScroll);
  }
});

const broadcast = [
  { mod: "broadcast", title: "Send to all terminals", command: "mod.open", args: { name: "broadcast", props: { target: "shells" } }, keywords: "smart" },
  { mod: "broadcast", title: "Send to all agent sessions", command: "mod.open", args: { name: "broadcast", props: { target: "agents" } }, keywords: "smart" },
];
const palette = (query: string) => {
  const view = new Palette();
  view.query = query; view.recent = []; view.at = 0; view.scroll = 0;
  return view;
};

test("palette lists a mod's actions under the mod's name and runs them with their args", () => {
  actions = broadcast;
  const grouped = palette("").rows().filter((r: any) => r.head === "broadcast");
  expect(grouped.map((r: any) => r.e.title)).toEqual(["Send to all terminals", "Send to all agent sessions"]);
  expect(palette("tab").rows().some((r: any) => r.e.group === "broadcast")).toBe(false);
  const view = palette("agent sessions");
  const [first] = view.rows();
  expect(first.e.title).toBe("Send to all agent sessions");
  ran.length = 0;
  view.go(first.e);
  expect(ran).toEqual([["mod.open", { name: "broadcast", props: { target: "agents" } }]]);
  expect(view.recent[0]).toBe(first.e.id);
  expect(palette("smart").rows().map((r: any) => r.e.title)).toEqual(expect.arrayContaining(["Send to all terminals", "Send to all agent sessions"]));
});

test("a mod's actions go when the mod does, and none change the list", () => {
  actions = [];
  const before = palette("").rows().map((r: any) => r.e.id);
  actions = broadcast;
  const withActions = palette("").rows().map((r: any) => r.e.id);
  expect(withActions.length).toBe(before.length + 2);
  actions = [];
  expect(palette("").rows().map((r: any) => r.e.id)).toEqual(before);
});

test("a mod action's id is its own, and a key for the command's first argument isn't its key", () => {
  actions = [{ mod: "acts", title: "New tab here", command: "tab.new", args: {}, keywords: "" }, { mod: "acts", title: "Another new tab", command: "tab.new", args: {}, keywords: "" }];
  const rows = palette("").rows();
  const ids = rows.filter((r: any) => r.e.name === "tab.new").map((r: any) => r.e.id);
  expect(new Set(ids).size).toBe(ids.length);
  actions = broadcast;
  const shown = (query: string) => {
    const texts: string[] = [];
    const walk = (el: any) => { for (const n of el?.nodes ?? []) { if (typeof n === "string") texts.push(n); walk(n); } };
    walk(palette(query).render());
    return texts;
  };
  bound = [{ section: "global", keys: "ctrl-shift-y", action: "mod.open:broadcast" }, { section: "global", keys: "ctrl-shift-u", action: "tab.new" }];
  try {
    expect(shown("terminals")).not.toContain("y");
    expect(shown("new tab")).toContain("u");
  } finally {
    bound = [];
  }
});

test("a mod action runs its own args, even for commands the palette special-cases", () => {
  actions = [
    { mod: "acts", title: "Clone zen", command: "layout.clone", args: { name: "zen" }, keywords: "" },
    { mod: "acts", title: "Clone files", command: "mod.clone", args: { name: "files", as: "files-mine" }, keywords: "" },
  ];
  for (const [query, args] of [["clone zen", { name: "zen" }], ["clone files", { name: "files", as: "files-mine" }]] as const) {
    const view = palette(query);
    ran.length = 0;
    view.go(view.rows()[0].e);
    expect(ran).toEqual([[view.rows()[0].e.name, args]]);
  }
});
