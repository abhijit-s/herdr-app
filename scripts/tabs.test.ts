import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const strip = (text: string, name: string) => text.replace(/^import .*;\n/gm, "").replace(`export default class ${name}`, `class ${name}`).replace(/^export /gm, "");
const tabsSource = strip(await readFile(new URL("../defaults/mods/tabs/main.js", import.meta.url), "utf8"), "Tabs");
const menuSource = strip(await readFile(new URL("../defaults/mods/menu/main.js", import.meta.url), "utf8"), "Menu");

type Tab = { id: string; name: string; active: boolean };
type Ev = { position: { x: number; y: number }; pressed_button?: string | null; local_position?: { x: number; y: number }; bounds?: { x: number; y: number; width: number; height: number }; click_count?: number; key?: string };

// Every element a render builds, with its id and handlers, so a test can find
// a tab by id and press, move and release on it as GPUI would.
function harness(names = ["a", "b", "c", "d"]) {
  const runs: [string, unknown][] = [];
  let elements: any[] = [];
  const element = () => {
    const el: any = { $handlers: {} as Record<string, (...args: any[]) => void>, $children: [] as any[], $calls: [] as string[], $args: {} as Record<string, any[]> };
    const proxy: any = new Proxy(el, { get: (target, method: string) => {
      if (method in target) return target[method];
      return (...args: any[]) => {
        el.$calls.push(method);
        el.$args[method] = args;
        if (method === "id") el.$id = args[0];
        else if (method === "when") return args[0] ? args[1](proxy) : proxy;
        else if (method === "child") el.$children.push(args[0]);
        else if (method === "children") el.$children.push(...args[0]);
        else if (method.startsWith("on_")) el.$handlers[args.length > 1 && typeof args[0] === "string" ? `${method}:${args[0]}` : method] = args.at(-1);
        return proxy;
      };
    } });
    elements.push(proxy);
    return proxy;
  };
  let tabs: Tab[] = names.map((id, i) => ({ id, name: id, active: i === 0 }));
  const space = () => ({ id: "s1", active: true, tabs });
  const handle = { focused: false, focus() { this.focused = true; }, is_focused() { return this.focused; } };
  const Tabs = new Function("View", "div", "svg", "image", "h_flex", "spaces", "panes", "run", "openOverlay", "theme", "window", "shared", "layout", "settings", "localStorage", "ScrollHandle", `${tabsSource}; return { Tabs, slotOver, dropIndex };`)(
    class {}, element, element, element, element,
    () => [space()], () => [],
    (name: string, args: unknown) => { runs.push([name, args]); },
    () => null, () => new Proxy({}, { get: () => "#000" }), () => null, () => null, () => null, () => ({ ui_scale: 1 }),
    { getItem: () => null, setItem() {} },
    { new: () => ({ scroll_to_item() {} }) },
  );
  const view = new Tabs.Tabs();
  const cx = { focus_handle: () => handle, notify() {}, stop_propagation() {} };
  view.init(null, cx);
  const render = () => { elements = []; view.render(); return elements; };
  render();
  const find = (id: string) => elements.find(e => e.$id === id);
  const strip = () => find("tab-strip");
  // Tabs 100 px wide, 4 px apart, from x = 20; the strip is 800 × 40 at the origin.
  const box = (id: string) => ({ x: 20 + tabs.findIndex(t => t.id === id) * 104, y: 5, width: 100, height: 29 });
  const at = (id: string, dx: number): Ev => {
    const b = box(id);
    return { position: { x: b.x + dx, y: 20 }, local_position: { x: dx, y: 15 }, bounds: b, pressed_button: held };
  };
  const stripEv = (x: number): Ev => ({ position: { x, y: 20 }, local_position: { x, y: 20 }, bounds: { x: 0, y: 0, width: 800, height: 40 }, pressed_button: held });
  // GPUI bubbles a pointer event from the innermost element out.
  // The button a move reports held: left from a press to its release.
  let held: string | null = null;
  const press = (id: string, dx = 50, click_count = 1) => { held = "left"; find(`tab-${id}`).$handlers["on_mouse_down:left"]({ ...at(id, dx), click_count }, cx); render(); };
  // The row of tabs (and the tail after them) spans x = 20 to 640.
  const rowEv = (x: number): Ev => ({ position: { x, y: 20 }, local_position: { x: x - 20, y: 20 }, bounds: { x: 20, y: 0, width: 620, height: 40 }, pressed_button: held });
  const inRow = (x: number) => find("tab-scroll").$handlers.on_mouse_move(rowEv(x), cx);
  const moveOver = (id: string, dx: number) => { const x = at(id, dx).position.x; find(`tab-${id}`).$handlers.on_mouse_move(at(id, dx), cx); inRow(x); strip().$handlers.on_mouse_move(stripEv(x), cx); render(); };
  const moveStrip = (x: number) => { if (x >= 20 && x <= 640) inRow(x); strip().$handlers.on_mouse_move(stripEv(x), cx); render(); };
  const moveTail = (x: number) => { find("tab-tail").$handlers.on_mouse_move(stripEv(x), cx); inRow(x); strip().$handlers.on_mouse_move(stripEv(x), cx); render(); };
  const release = (id?: string) => { held = null; if (id) find(`tab-${id}`).$handlers["on_mouse_up:left"](at(id, 50), cx); strip().$handlers["on_mouse_up:left"](stripEv(0), cx); render(); };
  const leave = () => { strip().$handlers.on_hover(false, cx); render(); };
  const key = (k: string) => { strip().$handlers.on_key_down({ key: k }, cx); render(); };
  const setTabs = (next: Tab[]) => { tabs = next; render(); };
  const lose = () => { held = null; };
  return { view, runs, handle, find, all: () => elements, press, moveOver, moveStrip, moveTail, release, leave, key, render, setTabs, lose, ...Tabs };
}

test("a drop slot is the half of the tab under the pointer", () => {
  const { slotOver } = harness();
  expect(slotOver(2, 10, 100)).toBe(2);
  expect(slotOver(2, 49.9, 100)).toBe(2);
  expect(slotOver(2, 50, 100)).toBe(3);
  expect(slotOver(0, 0, 100)).toBe(0);
});

test("the drop index moves the tab into its slot, or nowhere", () => {
  const { dropIndex } = harness();
  // Four tabs, dragging the second (index 1).
  expect(dropIndex(1, 0)).toBe(1);
  expect(dropIndex(1, 1)).toBeNull();
  expect(dropIndex(1, 2)).toBeNull();
  expect(dropIndex(1, 3)).toBe(3);
  expect(dropIndex(1, 4)).toBe(4);
  expect(dropIndex(0, 4)).toBe(4);
  expect(dropIndex(3, 0)).toBe(1);
  expect(dropIndex(3, null)).toBeNull();
  // Every slot puts the tab where the slot says, as tab.move inserts.
  for (let from = 0; from < 4; from++) for (let slot = 0; slot <= 4; slot++) {
    const tabs = ["a", "b", "c", "d"], index = dropIndex(from, slot);
    const rest = tabs.filter((_, i) => i !== from), want = [...rest];
    want.splice(slot > from ? slot - 1 : slot, 0, tabs[from]);
    const moved = [...tabs];
    if (index !== null) { const [t] = moved.splice(from, 1); moved.splice(Math.min(index - 1, moved.length), 0, t); }
    expect(moved).toEqual(want);
  }
});

test("a click without movement selects, and a double click renames", () => {
  const h = harness();
  h.press("b");
  h.moveOver("b", 53);
  h.release("b");
  expect(h.runs).toEqual([["tab.select", { id: "b" }]]);
  expect(h.handle.focused).toBe(false);
  h.press("c", 50, 2);
  h.release("c");
  expect(h.runs.slice(1)).toEqual([["tab.rename", { id: "c" }], ["tab.select", { id: "c" }]]);
});

test("dragging past the threshold moves the tab where the marker showed", () => {
  const h = harness();
  h.press("a", 30);
  h.moveOver("a", 40);
  expect(h.view.drag).not.toBeNull();
  expect(h.handle.focused).toBe(true);
  // Over its own place: nothing would change, so no marker.
  expect(h.find("tab-a").$calls).toContain("opacity");
  expect(h.find("tab-a").$calls).not.toContain("absolute");
  expect(h.find("tab-b").$children.some((c: any) => c.$calls.includes("absolute"))).toBe(false);
  h.moveOver("c", 70);
  expect(h.view.drag.slot).toBe(3);
  // The marker sits on the left of the tab after the slot; the ghost follows the pointer.
  const marked = h.find("tab-d").$children.at(-1);
  expect(marked.$calls).toEqual(expect.arrayContaining(["absolute", "left", "bg"]));
  // The ghost: the pointer's x less where the tab was grabbed (30 px in).
  const ghost = h.all().find((e: any) => e.$calls.includes("absolute") && e.$args.opacity?.[0] === 0.9);
  expect(ghost.$args.left[0]).toBe(20 + 2 * 104 + 70 - 30);
  h.release("c");
  expect(h.runs).toEqual([["tab.move", { id: "a", index: 3 }], ["pane.refocus", undefined]]);
  expect(h.view.drag).toBeNull();
});

test("dropping in a gap keeps the slot the pointer came from", () => {
  const h = harness();
  h.press("d");
  h.moveOver("b", 80);
  // Into the gap between b and c: only the strip hears it.
  h.moveStrip(20 + 104 + 101);
  expect(h.view.drag.slot).toBe(2);
  h.release();
  expect(h.runs[0]).toEqual(["tab.move", { id: "d", index: 3 }]);
});

test("either end of the strip drops at that end", () => {
  const h = harness();
  h.press("c");
  h.moveOver("b", 20);
  h.moveOver("a", 60);
  // The strip's padding, left of the first tab.
  h.moveStrip(5);
  expect(h.view.drag.slot).toBe(0);
  h.release();
  expect(h.runs[0]).toEqual(["tab.move", { id: "c", index: 1 }]);

  // A flick from the middle straight into the padding, over no other tab.
  const f = harness();
  f.press("c");
  f.moveOver("c", 90);
  f.moveStrip(5);
  expect(f.view.drag.slot).toBe(0);

  const g = harness();
  g.press("a");
  g.moveOver("b", 50);
  // The empty space after the last tab.
  g.moveTail(600);
  expect(g.view.drag.slot).toBe(4);
  // The buttons on the right, past the row.
  g.moveOver("d", 10);
  g.moveStrip(780);
  expect(g.view.drag.slot).toBe(4);
  g.release();
  expect(g.runs[0]).toEqual(["tab.move", { id: "a", index: 4 }]);
});

test("Escape or leaving the strip cancels a drag", () => {
  const h = harness();
  h.press("a");
  h.moveOver("c", 60);
  h.key("escape");
  expect(h.view.drag).toBeNull();
  h.release("c");
  // Its release selects nothing either.
  expect(h.runs).toEqual([["pane.refocus", undefined]]);

  const g = harness();
  g.press("a");
  g.moveOver("c", 60);
  g.leave();
  expect(g.view.drag).toBeNull();
  expect(g.view.press).toBeNull();
  expect(g.runs).toEqual([["pane.refocus", undefined]]);
  // Other keys go nowhere; a key without a drag does nothing.
  g.key("escape");
  expect(g.runs.length).toBe(1);
});

test("a drag whose release went missing ends at the next move without the button", () => {
  const h = harness();
  h.press("a");
  h.moveOver("c", 60);
  h.lose();
  h.moveOver("d", 60);
  expect(h.view.drag).toBeNull();
  expect(h.view.press).toBeNull();
  expect(h.runs).toEqual([["pane.refocus", undefined]]);
  // A plain move over the strip starts nothing, and the next click only selects.
  h.moveOver("b", 10);
  expect(h.view.drag).toBeNull();
  h.press("b");
  h.release("b");
  expect(h.runs.slice(1)).toEqual([["tab.select", { id: "b" }]]);

  // A press over a drag still going (its release never came) ends that drag first.
  const g = harness();
  g.press("a");
  g.moveOver("c", 60);
  g.press("d");
  expect(g.view.drag).toBeNull();
  g.release("d");
  expect(g.runs).toEqual([["pane.refocus", undefined], ["tab.select", { id: "d" }]]);
});

test("a tab closed during its drag is not moved", () => {
  const h = harness();
  h.press("b");
  h.moveOver("d", 80);
  h.setTabs([{ id: "a", name: "a", active: true }, { id: "c", name: "c", active: false }, { id: "d", name: "d", active: false }]);
  h.release();
  expect(h.runs.filter(([n]) => n === "tab.move")).toEqual([]);
});

test("the tab menu offers moves except at the ends", () => {
  const tabs = ["a", "b", "c"].map(id => ({ id, name: id }));
  const items = new Function("View", "div", "svg", "v_flex", "h_flex", "run", "props", "closeOverlay", "openOverlay", "theme", "keys", "terminals", "spaces", "panes", "exec", `${menuSource}; return items;`)(
    class {}, ...Array(4).fill(() => null), () => null, () => null, () => null, () => null, () => ({}), () => [], () => [],
    () => [{ id: "s1", tabs }], () => [], async () => ({ code: 1 }),
  );
  const labels = (id: string) => items({ tab: "tab", target: { id, space: "s1" } }, null).map(([label]: [string]) => label);
  expect(labels("a")).toContain("Move right");
  expect(labels("a")).not.toContain("Move left");
  expect(labels("b")).toEqual(expect.arrayContaining(["Move left", "Move right"]));
  expect(labels("c")).toContain("Move left");
  expect(labels("c")).not.toContain("Move right");
  const one = items({ tab: "tab", target: { id: "a", space: "s1" } }, null);
  expect(one.find(([, command]: [string, string]) => command === "tab.move_right")).toEqual(["Move right", "tab.move_right", { id: "a" }]);
});
