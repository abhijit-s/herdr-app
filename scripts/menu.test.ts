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
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  run: (...args: any[]) => ran.push(args), props: () => menuProps, closeOverlay: () => {}, openOverlay: () => {},
  theme: () => new Proxy({}, { get: () => "#000000" }), keys: () => [], exec: async () => ({ code: 1 }),
  terminals: () => [{ address: "t2", labels: {} }], spaces: () => [], panes: () => [{ pane: "p1", terminal: "t1" }, { pane: "p2", terminal: "t2" }],
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
  { mod: "mover", title: "Move to…", command: "mod.open", args: { name: "mover", props: { mode: "tab" } }, when: "pane", icon: null, fill: ["props"] },
  { mod: "mover", title: "Split under", command: "pane.split", args: { direction: "down" }, when: null, icon: null, fill: ["pane"] },
  { mod: "grab", title: "Focus it", command: "pane.focus", args: {}, when: null, icon: null, fill: ["pane", "terminal"] },
  { mod: "grab", title: "Zoom p1", command: "pane.zoom", args: { pane: "p1" }, when: null, icon: null, fill: [] },
];

// The clicked pane is p2, not the focused p1: every item acts on it.
test("a mod's menu item runs its command on the right-clicked terminal", () => {
  menuProps = { tab: "pane", target, overlay: "o1" };
  expect(click("Move to…")).toEqual([["mod.open", { name: "mover", props: { mode: "tab", target } }]]);
  expect(click("Split under")).toEqual([["pane.split", { direction: "down", pane: "p2" }]]);
  expect(click("Focus it")).toEqual([["pane.focus", { pane: "p2", terminal: "t2" }]]);
  expect(click("Zoom p1")).toEqual([["pane.zoom", { pane: "p1" }]]);
  expect(items[0].args.props).toEqual({ mode: "tab" });
});
