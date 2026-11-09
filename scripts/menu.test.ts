import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The shipped terminal menu's mod items, without a window: what a click runs.
class Element {
  nodes: any[] = [];
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
const ran: any[] = [];
let menuProps: any;
let items: any[] = [];
let spaceList: any[] = [];
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  run: (...args: any[]) => ran.push(args), props: () => menuProps, closeOverlay: () => {}, openOverlay: () => {},
  theme: () => new Proxy({}, { get: () => "#000000" }), keys: () => [], exec: async () => ({ code: 1 }),
  terminals: () => [{ address: "t2", labels: {} }], spaces: () => spaceList, panes: () => [{ pane: "p1", terminal: "t1", focused: true }, { pane: "p2", terminal: "t2", focused: false }],
  focused: () => ({ pane: "p1", terminal: "t1" }),
  menuItems: () => items,
};
const source = await readFile(new URL("../defaults/mods/menu/main.js", import.meta.url), "utf8");
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap((m) => m[1].split(",").map((n) => n.trim()));
const Menu = new Function(...names, source.replace(/^import .*;\n/gm, "").replace("export default class Menu", "return class Menu"))(...names.map((n) => values[n]));

function find(root: any, id: string): any {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}
function click(title: string) {
  const view = new Menu();
  view.init(menuProps, { focus_handle: () => ({ focus() {} }), spawn() {} });
  const tree = view.render({});
  const rows = Array.from({ length: 40 }, (_, i) => find(tree, `mi-${i}`)).filter(Boolean);
  const row = rows.find((r) => JSON.stringify(r.nodes.map((n: any) => n.nodes)).includes(JSON.stringify(title)));
  expect(row).toBeDefined();
  ran.length = 0;
  row.calls.on_click[0]({}, {});
  return ran;
}

const target = { pane: "p2", terminal: "t2", selected: false, cwd: "/home/me/src", last_output: false };
items = [
  { mod: "mover", title: "Move to…", command: "mod.open", args: { name: "mover", props: { mode: "tab", target: "stale" } }, when: "pane", icon: null, fill: ["props"] },
  { mod: "mover", title: "Split under", command: "pane.split", args: { direction: "down" }, when: null, icon: null, fill: ["pane"] },
  { mod: "grab", title: "Focus it", command: "pane.focus", args: {}, when: null, icon: null, fill: ["pane", "terminal"] },
  { mod: "grab", title: "Zoom p1", command: "pane.zoom", args: { pane: "p1" }, when: null, icon: null, fill: [] },
];

// The clicked pane is p2, not the focused p1: every item acts on it, and the
// click's target replaces an entry's own props.target.
test("a mod's menu item runs its command on the right-clicked terminal", () => {
  menuProps = { tab: "pane", target, overlay: "o1" };
  expect(click("Move to…")).toEqual([["mod.open", { name: "mover", props: { mode: "tab", target } }]]);
  expect(click("Split under")).toEqual([["pane.split", { direction: "down", pane: "p2" }]]);
  expect(click("Focus it")).toEqual([["pane.focus", { pane: "p2", terminal: "t2" }]]);
  expect(click("Zoom p1")).toEqual([["pane.zoom", { pane: "p1" }]]);
  expect(items[0].args.props).toEqual({ mode: "tab", target: "stale" }); // the published entry is left as it was
});

// More items than the popup's height: they scroll, Close pane stays pinned
// below them, and the keyboard's selection is scrolled into view.
test("a long pane menu scrolls its items and keeps Close pane in reach", () => {
  menuProps = { tab: "pane", target: { ...target, selected: true, last_output: true }, height: 420, overlay: "o1" };
  items = Array.from({ length: 6 }, (_, i) => ({ mod: `m${i % 2}`, title: `Item ${i}`, command: "pane.zoom", args: {}, when: null, icon: null, fill: ["pane"] }));
  const cx = { focus_handle: () => ({ focus() {} }), spawn() {}, notify() {} };
  const view = new Menu();
  view.init(menuProps, cx);
  const rows = (tree: any) => Array.from({ length: 40 }, (_, i) => [i, find(tree, `mi-${i}`)]).filter(([, r]) => r);
  let tree = view.render({});
  const body = find(tree, "menu-body");
  expect(body).toBeDefined();
  const all = rows(tree);
  const last = all.length - 1;
  expect(JSON.stringify(all[last][1].nodes.map((n: any) => n.nodes))).toContain("Close pane");
  expect(find(body, `mi-${last}`)).toBeUndefined(); // pinned outside the scrolled part
  expect(find(body, `mi-${last - 1}`)).toBeDefined();
  // 420 less the padding and the pinned Close pane with its rule.
  const room = body.calls.h[0];
  expect(room).toBe(420 - 10 - 40);
  const inner = () => find(tree, "menu-body").nodes[0];
  expect(inner().calls.top[0]).toBe(-0);
  // Down to the last scrolled item: it comes into view at the bottom.
  const press = (key: string) => { tree.calls.on_key_down[0]({ key }, cx); tree = view.render({}); };
  for (let i = 0; i < last - 1; i++) press("down");
  expect(view.at).toBe(last - 1);
  const scrolled = -inner().calls.top[0];
  expect(scrolled).toBeGreaterThan(0);
  expect(view.scroll).toBe(scrolled);
  press("down"); // onto the pinned Close pane: no scroll needed
  expect(view.at).toBe(last);
  expect(-inner().calls.top[0]).toBe(scrolled);
  press("down"); // wraps to the top, scrolled back
  expect(view.at).toBe(0);
  expect(inner().calls.top[0]).toBe(-0);
  // The wheel scrolls within bounds: the last scrolled item was brought to the very bottom.
  find(tree, "menu-body").calls.on_scroll_wheel[0]({ delta: { y: -10000 } }, cx);
  tree = view.render({});
  expect(view.scroll).toBe(scrolled);
  find(tree, "menu-body").calls.on_scroll_wheel[0]({ delta: { y: 10000 } }, cx);
  tree = view.render({});
  expect(view.scroll).toBe(0);
});

test("a menu that fits doesn't scroll, and one without a height never does", () => {
  for (const height of [420, undefined]) {
    menuProps = { tab: "pane", target, overlay: "o1", ...(height ? { height } : {}) };
    items = height ? [] : Array.from({ length: 20 }, (_, i) => ({ mod: "m", title: `Item ${i}`, command: "pane.zoom", args: {}, when: null, icon: null, fill: ["pane"] }));
    const view = new Menu();
    view.init(menuProps, { focus_handle: () => ({ focus() {} }), spawn() {} });
    expect(find(view.render({}), "menu-body")).toBeUndefined();
  }
});

// Nothing to pin (the app menu ends with themes): every item scrolls, in all the room.
test("an overflowing menu with no Close at the end scrolls all of it", () => {
  menuProps = { tab: "global", target: {}, height: 100, overlay: "o1" };
  items = [];
  const cx = { focus_handle: () => ({ focus() {} }), spawn() {}, notify() {} };
  const view = new Menu();
  view.init(menuProps, cx);
  let tree = view.render({});
  const body = find(tree, "menu-body");
  expect(body.calls.h[0]).toBe(100 - 10);
  const count = Array.from({ length: 40 }, (_, i) => find(tree, `mi-${i}`)).filter(Boolean).length;
  expect(count).toBe(6);
  expect(Array.from({ length: count }, (_, i) => find(body, `mi-${i}`)).every(Boolean)).toBe(true);
  // Up from the first item wraps to the last, scrolled to the bottom of the list.
  tree.calls.on_key_down[0]({ key: "up" }, cx);
  tree = view.render({});
  expect(view.at).toBe(count - 1);
  // Six rows and two rules (before Keybinds and the themes), less the room.
  expect(view.scroll).toBe(6 * 31 + 2 * 9 - 90);
  expect(find(tree, "menu-body").nodes[0].calls.top[0]).toBe(-view.scroll);
});

test("up from the first item wraps to the pinned Close pane without scrolling", () => {
  menuProps = { tab: "pane", target: { ...target, selected: true, last_output: true }, height: 420, overlay: "o1" };
  items = Array.from({ length: 6 }, (_, i) => ({ mod: `m${i % 2}`, title: `Item ${i}`, command: "pane.zoom", args: {}, when: null, icon: null, fill: ["pane"] }));
  const cx = { focus_handle: () => ({ focus() {} }), spawn() {}, notify() {} };
  const view = new Menu();
  view.init(menuProps, cx);
  let tree = view.render({});
  const last = Array.from({ length: 40 }, (_, i) => find(tree, `mi-${i}`)).filter(Boolean).length - 1;
  tree.calls.on_key_down[0]({ key: "up" }, cx);
  tree = view.render({});
  expect(view.at).toBe(last);
  expect(find(find(tree, "menu-body"), `mi-${last}`)).toBeUndefined();
  expect(view.scroll).toBe(0);
});

// Move to space… opens the tab-move picker on the right-clicked tab, and only
// when that tab's machine has another space.
test("a tab's Move to space… opens the space picker for that tab", () => {
  const tab = (id: string) => ({ id, name: id, index: 1, active: true });
  spaceList = [
    { id: "s1", machine: null, tabs: [tab("a"), tab("b")] },
    { id: "mac/s2", machine: "mac", tabs: [tab("mac/c")] },
  ];
  menuProps = { tab: "tab", target: { id: "b", space: "s1" }, overlay: "o1" };
  const titles = () => {
    const view = new Menu();
    view.init(menuProps, { focus_handle: () => ({ focus() {} }), spawn() {} });
    const tree = view.render({});
    return Array.from({ length: 40 }, (_, i) => find(tree, `mi-${i}`)).filter(Boolean).map((r) => JSON.stringify(r.nodes.map((n: any) => n.nodes)));
  };
  expect(titles().some((t) => t.includes("Move to space"))).toBe(false);
  spaceList.push({ id: "s3", machine: null, tabs: [tab("d")] });
  expect(click("Move to space…")).toEqual([["mod.open", { name: "tab-move", props: { id: "b" } }]]);
  menuProps = { tab: "tab", target: { id: "mac/c", space: "mac/s2" }, overlay: "o1" };
  expect(titles().some((t) => t.includes("Move to space"))).toBe(false);
  spaceList = [];
});
