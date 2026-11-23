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
let dark = true;
const home = "/home/app";
let disk: Record<string, string> = {};
const shipped = [["graphite", false], ["ink", false], ["moss", false], ["paper", true], ["atom-one-dark", false], ["mine-light", true], ["win98", true]];
const values = {
  View: class {}, div: element, svg: (path: string) => { icons.push(path); return element(); }, v_flex: element, h_flex: element, Input: { new: element }, theme: () => ({}), keys: () => bound, win: () => ({ platform }),
  commands: () => [
    { name: "tab.new", args: [{ name: "name", required: false }], about: "New tab." },
    { name: "mod.open", args: [{ name: "name", required: true }, { name: "props", required: false }], about: "Open a mod." },
    { name: "app.theme", args: [{ name: "name", required: true }], about: "Use a theme." },
    { name: "mod.clone", args: [{ name: "name", required: true }], about: "Copy a mod." },
    { name: "tab.rename", args: [{ name: "id", required: false }, { name: "name", required: true }], about: "Rename a tab." },
    { name: "layout.resize", args: [{ name: "id", required: true }, { name: "px", required: false }], about: "Resize." },
  ],
  layouts: () => ["main", "win98"], modActions: () => actions, run: (name: string, args: any) => { ran.push([name, args]); }, props: () => ({ overlay: "palette" }), closeOverlay: () => {}, shared: () => "main",
  themes: () => shipped.map(([name, light]) => ({ name, light })), settings: () => ({ dark }),
  localStorage: { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => { store.set(k, v); } },
  appHome: () => home,
  readdir: async (path: string) => {
    if (path !== `${home}/mods`) throw new Error("no such folder");
    return [...new Set(Object.keys(disk).map((k) => k.split("/")[4]))];
  },
  exists: (path: string) => {
    if (path === `${home}/mods/blocked/main.js`) throw new Error("permission denied");
    return Promise.resolve(path in disk);
  },
  readFile: async (path: string) => {
    if (!(path in disk)) throw new Error("no such file");
    return disk[path];
  },
};
const body = source
  .replace(/^import .*;\n/gm, "")
  .replace("export default class Palette", "class Palette")
  .replace("export async function modPalettes", "async function modPalettes");
const { Palette, modPalettes, keyOf } = new Function(...Object.keys(values), `${body}\nreturn { Palette, modPalettes, keyOf };`)(...Object.values(values));
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

test("palette offers only the themes that can show in the current appearance", () => {
  const offered = () => {
    const view = new Palette();
    view.query = ""; view.recent = [];
    return view.rows().map((r: any) => r.e).filter((e: any) => e.name === "app.theme").map((e: any) => e.args.name);
  };
  dark = true;
  expect(offered()).toEqual(["graphite", "ink", "moss", "atom-one-dark"]);
  dark = false;
  // win98 is the Herdr 98 layout's own theme, so it isn't offered here.
  expect(offered()).toEqual(["paper", "mine-light"]);
  dark = true;
});

// Entries from the mods' mod.json files, as the palette lists them.
async function listed(spec: Record<string, unknown>, query = "") {
  disk = {};
  for (const [name, manifest] of Object.entries(spec)) {
    disk[`${home}/mods/${name}/mod.json`] = typeof manifest === "string" ? manifest : JSON.stringify(manifest);
    disk[`${home}/mods/${name}/main.js`] = "";
  }
  const view = new Palette();
  view.query = query; view.recent = [];
  const before = new Set(view.rows().map((r: any) => r.e.id));
  view.mods = await modPalettes(home);
  return view.rows().map((r: any) => r.e).filter((e: any) => !before.has(e.id));
}
const tui = { title: "TUI sessions", command: "mod.open", args: { name: "tui-sessions" }, words: "tui agents" };

test("a mod's palette entry is listed under Mod and found by its words", async () => {
  const spec = { "tui-sessions": { place: "none", palette: [tui] } };
  const [e, ...rest] = await listed(spec);
  expect(rest).toEqual([]);
  expect(e).toMatchObject({ name: "mod.open", args: { name: "tui-sessions" }, group: "Mod", title: "TUI sessions" });
  expect(e.words).toContain("tui agents");
  expect((await listed(spec, "agents")).map((x: any) => x.title)).toEqual(["TUI sessions"]);
});

test("a mod's palette entry defaults args and words, and may pick a group", async () => {
  const found = await listed({
    a: { palette: [{ title: "New tab here", command: "tab.new", args: { name: "work" }, group: "Tab" }] },
    b: { palette: [{ title: "Other", command: "b.toggle" }] },
    c: { palette: [{ title: "Scaled", command: "layout.resize", args: { id: "sidebar" } }] },
  });
  expect(found.find((e: any) => e.title === "New tab here")).toMatchObject({ args: { name: "work" }, group: "Tab" });
  expect(found.find((e: any) => e.title === "Other")).toMatchObject({ name: "b.toggle", group: "Mod" });
  expect(found.find((e: any) => e.title === "Scaled")).toMatchObject({ args: { id: "sidebar" } });
});

test("entries that can't run are skipped", async () => {
  const found = await listed({
    ok: { palette: [{ title: "Fine", command: "mod.open", args: { name: "ok" } }] },
    bad: {
      palette: [
        { command: "tab.new" },
        { title: "  ", command: "tab.new" },
        { title: "No command" },
        { title: "Unknown command", command: "nothing.here" },
        { title: "Unknown arg", command: "tab.new", args: { nope: 1 } },
        { title: "Missing arg", command: "mod.open" },
        { title: "Missing id", command: "layout.resize" },
        { title: "Args not an object", command: "tab.new", args: ["x"] },
        { title: "Missing mod folder", command: "mod.open", args: { name: "gone" } },
        { title: "Null required arg", command: "layout.resize", args: { id: null } },
        { title: "Blank required arg", command: "layout.resize", args: { id: "  " } },
        { title: "Non-string target", command: "layout.resize", args: { id: 42 } },
        { title: "An empty mod command", command: "bad." },
        { title: "Another mod's action", command: "ok.toggle", mod: "ok" },
        { title: "Mod command with args", command: "bad.toggle", args: { a: 1 } },
        { title: "Mod not installed", command: "gone.toggle" },
        "not an entry",
        null,
      ],
    },
    broken: "{ not json",
    plain: { place: "right" },
  });
  expect(found.map((e: any) => e.title)).toEqual(["Fine"]);
});

test("a rename entry may leave the name for the window to ask", async () => {
  const found = await listed({ a: { palette: [{ title: "Rename this tab", command: "tab.rename", args: { id: "t1" } }] } });
  expect(found.map((e: any) => e.title)).toEqual(["Rename this tab"]);
});

test("a mod's entry that repeats a listed one isn't listed twice", async () => {
  const found = await listed({ a: { palette: [
    { title: "Mine", command: "tab.new" },
    { title: "Next to Mine", command: "tab.new" },
    { title: "Go", command: "mod.open", args: { name: "picker" } },
  ] } });
  expect(found).toEqual([]);
});

test("palette loader skips hidden folders and survives a folder it can't read", async () => {
  disk = {
    [`${home}/mods/.git/mod.json`]: JSON.stringify({ palette: [{ title: "Hidden", command: "tab.new" }] }),
    [`${home}/mods/a/mod.json`]: JSON.stringify({ palette: [{ title: "Shown", command: "tab.new" }] }),
  };
  const linkOnly = { palette: [
    { ...tui, args: { name: "a" } },
    { ...tui, args: { name: "blocked" } },
    { title: "Still listed", command: "tab.new" },
  ] };
  disk[`${home}/mods/b/mod.json`] = JSON.stringify(linkOnly);
  disk[`${home}/mods/blocked/main.js`] = "";
  expect((await modPalettes(home)).map((e: any) => e.title)).toEqual(["Shown", "Still listed"]);
  expect(await modPalettes("/elsewhere")).toEqual([]);
});

test("the palette deduplicates equivalent arguments and shows the binding for the whole action", async () => {
  const [first, ...rest] = await listed({
    a: { palette: [
      { title: "Show left", command: "layout.resize", args: { id: "left", px: 250 }, group: "Recent" },
      { title: "Same action", command: "layout.resize", args: { px: 250, id: "left" } },
    ] },
  });
  expect(rest).toEqual([]);
  expect(first).toMatchObject({ title: "Show left", group: "Mod" });
  expect(keyOf(first, [
    { section: "global", keys: "f6", action: "layout.resize:left" },
    { section: "global", keys: "f7", action: '{"args":{"px":250,"id":"left"},"command":"layout.resize"}' },
  ])).toBe("f7");
});
