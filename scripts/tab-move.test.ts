import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

// The shipped tab-move picker, without a window: which spaces it offers, its keys and what a pick runs.
const source = (await readFile(new URL("../defaults/mods/tab-move/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class TabMove", "class TabMove").replace(/^export /gm, "");

const tab = (id: string, name: string, index: number, active = index === 1) => ({ id, name, index, active });
const SPACES = [
  { id: "s1", name: "herdr", machine: null, active: true, tabs: [tab("t1", "build", 1, false), tab("t2", "", 2, true)] },
  { id: "s2", name: "docs", machine: null, active: false, tabs: [tab("t3", "write", 1)] },
  { id: "mac/s3", name: "mac work", machine: "mac", active: false, tabs: [tab("mac/t4", "", 1)] },
  { id: "s4", name: "operations", machine: null, active: false, tabs: [tab("t5", "", 1), tab("t6", "", 2)] },
  { id: "mac/s5", name: "mac docs", machine: "mac", active: false, tabs: [tab("mac/t7", "", 1)] },
];

function harness(props: any, spaces: any[] = SPACES) {
  const runs: [string, unknown][] = [];
  const closed: string[] = [];
  let elements: any[] = [];
  const element = () => {
    const el: any = { $id: null, $handlers: {}, $children: [] as any[] };
    const proxy: any = new Proxy(el, { get: (target, method: string) => {
      if (method in target) return target[method];
      return (...args: any[]) => {
        if (method === "id") el.$id = args[0];
        else if (method === "when") return args[0] ? args[1](proxy) : proxy;
        else if (method === "child") el.$children.push(args[0]);
        else if (method === "children") el.$children.push(...args[0]);
        else if (method.startsWith("on_")) el.$handlers[method] = args.at(-1);
        return proxy;
      };
    } });
    elements.push(proxy);
    return proxy;
  };
  const mod = new Function("View", "div", "v_flex", "h_flex", "spaces", "props", "run", "closeOverlay", "theme",
    `${source}; return { TabMove, chosenTab, targets, searchRows, matchRank, typed, ROWS };`)(
    class {}, element, element, element,
    () => spaces, () => ({ overlay: "o1", ...props }),
    (name: string, args: unknown) => { runs.push([name, args]); },
    (id: string) => { closed.push(id); },
    () => new Proxy({}, { get: () => "#000" }),
  );
  const view = new mod.TabMove();
  const cx = { focus_handle: () => ({ focus() {} }), timer: { after() {} }, notify() {}, stop_propagation() {} };
  view.init(props, cx);
  const texts = (el: any): string[] => typeof el === "string" ? [el] : (el?.$children ?? []).flatMap(texts);
  const render = () => { elements = []; return texts(view.render()); };
  const rows = () => { render(); return elements.filter((e) => e.$id?.startsWith("tm-")).map((e) => texts(e)[1]); };
  // A key as GPUI reports it: printable ones carry key_char.
  const press = (keystroke: string, modifiers = {}) => view.key({ keystroke, key_char: keystroke.length === 1 ? keystroke : null, modifiers }, cx);
  const type = (text: string) => [...text].forEach((c) => press(c));
  return { mod, view, runs, closed, render, rows, press, type, elements: () => elements };
}

test("it moves the named tab, else the shown one, and offers only the other spaces of its machine", () => {
  const named = harness({ id: "t1" });
  expect(named.view.tab.id).toBe("t1");
  expect(named.rows()).toEqual(["docs", "operations"]);
  const shown = harness({});
  expect(shown.view.tab.id).toBe("t2");
  const remote = harness({ id: "mac/t4" });
  expect(remote.rows()).toEqual(["mac docs"]);
  // Nothing to move, or nowhere to go: it says so and a key picks nothing.
  const gone = harness({ id: "nope" });
  expect(gone.render()).toContain("No tab to move.");
  const alone = harness({ id: "t1" }, [SPACES[0], SPACES[2]]);
  expect(alone.render()).toContain("No other space on this machine.");
  alone.press("enter");
  alone.press("1");
  expect(alone.runs).toEqual([]);
});

test("1-9, Enter and a click run tab.move_to_space once and close", () => {
  const h = harness({ id: "t1" });
  h.press("2");
  expect(h.runs).toEqual([["tab.move_to_space", { id: "t1", space: "s4" }]]);
  expect(h.closed).toEqual(["o1"]);
  const k = harness({ id: "t1" });
  k.press("j");
  k.press("down"); // already at the last
  k.press("k");
  k.press("j");
  k.press("enter");
  expect(k.runs).toEqual([["tab.move_to_space", { id: "t1", space: "s4" }]]);
  const c = harness({ id: "t1" });
  c.render();
  c.elements().find((e) => e.$id === "tm-0").$handlers.on_click({}, {});
  expect(c.runs).toEqual([["tab.move_to_space", { id: "t1", space: "s2" }]]);
  // A number past the rows picks nothing; Esc closes without moving.
  const e = harness({ id: "t1" });
  e.press("9");
  e.press("escape");
  expect(e.runs).toEqual([]);
  expect(e.closed).toEqual(["o1"]);
});

test("typing searches by name; while searching every character types and Esc steps back", () => {
  const h = harness({ id: "t1" });
  h.type("op");
  expect(h.view.query).toBe("op");
  expect(h.rows()).toEqual(["operations"]);
  // j, k and digits type while searching.
  h.press("backspace");
  h.press("backspace");
  expect(h.view.query).toBe("");
  h.type("d1");
  expect(h.rows()).toEqual([]);
  expect(h.render()).toContain('No space matches "d1"');
  h.press("escape"); // clears the query
  expect(h.view.query).toBe("");
  h.press("escape"); // ends the search, not the picker
  expect(h.view.query).toBe(null);
  expect(h.closed).toEqual([]);
  h.press("/");
  expect(h.view.query).toBe("");
  h.press("backspace"); // on an empty query: back to the list
  expect(h.view.query).toBe(null);
  h.type("o");
  expect(h.rows()).toEqual(["operations", "docs"]); // a match at the start before one inside
  h.press("down");
  h.press("enter");
  expect(h.runs).toEqual([["tab.move_to_space", { id: "t1", space: "s2" }]]);
  // A space doesn't start a search; a chord doesn't type.
  const s = harness({ id: "t1" });
  s.press(" ");
  s.press("d", { platform: true });
  expect(s.view.query).toBe(null);
});

test("a long list scrolls to keep the cursor among the nine shown", () => {
  const many = [SPACES[0], ...Array.from({ length: 12 }, (_, i) => ({ id: `x${i}`, name: `space ${i}`, machine: null, active: false, tabs: [] }))];
  const h = harness({ id: "t1" }, many);
  expect(h.rows().length).toBe(9);
  expect(h.render()).toContain("↓ 3 more");
  for (let i = 0; i < 10; i++) h.press("j");
  expect(h.rows()[0]).toBe("space 2");
  expect(h.render()).toContain("↑ 2 more");
  h.press("1");
  expect(h.runs).toEqual([["tab.move_to_space", { id: "t1", space: "x2" }]]);
});
