import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The toast mod without a window: what it draws for each `toast_mark` and `toast_design`.
class Element {
  nodes: Element[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const element = () => new Element();
let propsNow: Record<string, any> = {};
// Each colour named after its theme key, unless a test sets a real theme.
const named = new Proxy({}, { get: (_t, key: string) => `#${key}` });
let themeNow: Record<string, any> = named;
const values: Record<string, any> = {
  View: class {}, div: element, image: element, svg: element, v_flex: element, h_flex: element,
  props: () => propsNow, closeOverlay() {}, run() {},
  theme: () => themeNow,

};
const source = await readFile(new URL("../defaults/mods/toast/main.js", import.meta.url), "utf8");
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap((m) => m[1].split(",").map((n) => n.trim()));
const body = source.replace(/^import .*;\n/gm, "").replace("export default class Toast", "const Toast = class Toast") + "\nreturn Toast;";
const Toast = new Function(...names, body)(...names.map((n) => values[n]));

function find(root: Element, id: string): Element | undefined {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}

// The mark comes in the props, as Herdr puts the person's toast_mark there.
const draw = (mark: Record<string, any>, props: Record<string, any> = { title: "Saved", level: "error" }) => {
  propsNow = { ...props, ...mark };
  return new Toast().render({});
};

test("stripe, the default: a stripe in the level's colour, its width from the padding, and the dot", () => {
  for (const s of [{}, { toast_mark: "stripe" }]) {
    const root = draw(s);
    expect(root.calls.border_l).toEqual([3]);
    expect(root.calls.border_color).toEqual(["#toast_error"]);
    expect(root.calls.pl).toEqual([15]);
    expect(find(root, "toast-dot")?.calls.bg).toEqual(["#toast_error"]);
  }
});

test("dot: the dot alone, no stripe, the padding whole", () => {
  const root = draw({ toast_mark: "dot" });
  expect(root.calls.border_l).toBeUndefined();
  expect(root.calls.pl).toEqual([18]);
  expect(find(root, "toast-dot")?.calls.bg).toEqual(["#toast_error"]);
});

test("none: neither stripe nor dot, but an icon still shows", () => {
  const root = draw({ toast_mark: "none" });
  expect(root.calls.border_l).toBeUndefined();
  expect(find(root, "toast-dot")).toBeUndefined();
  const icon = draw({ toast_mark: "none" }, { title: "Build", icon: "/i.png" });
  expect(icon.nodes[0].calls.w).toEqual([18]);
});

test("an icon takes the dot's place under every mark", () => {
  for (const toast_mark of ["stripe", "dot"]) expect(find(draw({ toast_mark }, { title: "Build", icon: "/i.png" }), "toast-dot")).toBeUndefined();
});

const designs = ["card", "pill", "terminal", "outline", "icon-card", "banner"];
const all = (root: Element): Element[] => [root, ...(root?.nodes ?? []).flatMap((n) => (n instanceof Object && "calls" in n ? all(n) : []))];

// master's card, as its toast mod drew it before designs (tests/fixtures/master-toast-card.json,
// rendered through these same stand-ins): every mark, level, body and icon.
test("a toast without a design draws master's card, call for call", async () => {
  const tree = (e: any): any => (e instanceof Element ? { calls: Object.fromEntries(Object.entries(e.calls).map(([k, v]) => [k, v.map((a) => (typeof a === "function" ? "fn" : a))])), nodes: e.nodes.map(tree) } : e);
  const master = JSON.parse(await readFile(new URL("../tests/fixtures/master-toast-card.json", import.meta.url), "utf8"));
  expect(master.length).toBe(80);
  for (const { props, tree: want } of master) {
    for (const design of [{}, { toast_design: "card" }]) {
      propsNow = { ...props, ...design };
      expect(tree(new Toast().render({}))).toEqual(want);
    }
  }
});

test("pill: one line, the dot for a stripe, the body only as its tooltip", () => {
  // Herdr sends the mark the pill takes: a dot for the person's stripe (ToastDesign::mark).
  const root = draw({ toast_design: "pill", toast_mark: "dot" }, { title: "Copied", body: "to the clipboard", level: "success" });
  expect(root.calls.border_l).toBeUndefined();
  expect(find(root, "toast-dot")?.calls.bg).toEqual(["#toast_success"]);
  expect(root.calls.tooltip).toEqual(["to the clipboard"]);
  expect(all(root).filter((e) => e.calls.text_color?.[0] === "#toast_muted")).toEqual([]);
  expect(find(draw({ toast_design: "pill", toast_mark: "none" }), "toast-dot")).toBeUndefined();
});

test("terminal: the terminal's font, the level bracketed in its colour, no stripe or dot whatever the mark", () => {
  for (const [level, tag] of [["warning", "[WARN]"], ["success", "[DONE]"], ["error", "[ERR]"], ["info", "[INFO]"], [undefined, "[INFO]"]]) {
    const root = draw({ toast_design: "terminal", toast_font: "Iosevka" }, { title: "Build", level });
    expect(root.calls.font_family).toEqual(["Iosevka"]);
    const t = find(root, "toast-tag")!;
    expect(t.nodes).toEqual([tag]);
    expect(t.calls.text_color).toEqual([`#toast_${level ?? "info"}`]);
    expect(root.calls.border_l).toBeUndefined();
    expect(find(root, "toast-dot")).toBeUndefined();
  }
});

test("outline: see-through, its title in the level's colour, no stripe or dot", () => {
  const root = draw({ toast_design: "outline", toast_mark: "none" }, { title: "Failed", level: "error" });
  expect(root.calls.bg).toBeUndefined();
  expect(root.calls.border_l).toBeUndefined();
  expect(find(root, "toast-dot")).toBeUndefined();
  expect(all(root).some((e) => e.calls.text_color?.[0] === "#toast_error" && e.nodes[0] === "Failed")).toBe(true);
  // No hover fill: it would lighten what shows through the backing the text is fitted to.
  expect(root.calls.hover).toBeUndefined();
  expect(draw({}, { title: "Saved" }).calls.hover).toBeDefined();
});

test("icon card: a big level icon, and Show and Dismiss only for a toast with an action", () => {
  const plain = draw({ toast_design: "icon-card" }, { title: "Saved", level: "warning" });
  const icon = find(plain, "toast-icon")!;
  expect([icon.calls.size, icon.calls.bg]).toEqual([[36], ["#toast_warning"]]);
  expect(icon.nodes[0].calls.text_color).toEqual(["#toast"]);
  expect(find(plain, "toast-actions")).toBeUndefined();
  const agent = draw({ toast_design: "icon-card" }, { title: "claude finished", level: "info", action: { name: "agent.focus", args: { terminal: "t1" } } });
  expect(find(agent, "toast-show")?.nodes).toEqual(["Show"]);
  expect(find(agent, "toast-dismiss")?.nodes).toEqual(["Dismiss"]);
  // Its own icon takes the level icon's place.
  expect(find(draw({ toast_design: "icon-card" }, { title: "Build", icon: "/i.png" }), "toast-icon")).toBeUndefined();
});

test("banner: title, place, how many more, Show for an action, Dismiss always", () => {
  const root = draw({ toast_design: "banner", toast_more: 2 }, { title: "claude finished", body: "work / api", level: "info", action: { name: "agent.focus" } });
  expect(find(root, "toast-more")?.nodes).toEqual(["+2 more"]);
  // A long title truncates rather than pushing the buttons out.
  expect(all(root).find((e) => e.nodes[0] === "claude finished")?.calls.min_w).toEqual([0]);
  expect(find(root, "toast-show")).toBeDefined();
  expect(find(root, "toast-dismiss")).toBeDefined();
  expect(root.calls.border_l).toEqual([3]);
  const alone = draw({ toast_design: "banner" }, { title: "Saved" });
  expect([find(alone, "toast-more"), find(alone, "toast-show")]).toEqual([undefined, undefined]);
  expect(find(alone, "toast-dismiss")).toBeDefined();
});

// Every colour each design draws is one of the theme's: under two themes far
// apart, each drawn colour is one that theme holds, so a theme switch restyles them all.
test("every design draws only its theme's colours, under a dark and a light theme", () => {
  const keys = ["toast", "toast_border", "toast_foreground", "toast_muted", "toast_info", "toast_success", "toast_warning", "toast_error", "hover"];
  const dark = Object.fromEntries(keys.map((k, i) => [k, `#1${i}2${i}3${i}`]));
  const light = Object.fromEntries(keys.map((k, i) => [k, `#e${i}d${i}c${i}`]));
  const colours = (root: Element) => all(root).flatMap((e) => ["bg", "text_color", "border_color"].flatMap((k) => e.calls[k] ?? []));
  for (const theme of [dark, light]) {
    themeNow = theme;
    const palette = new Set(Object.values(theme));
    for (const toast_design of designs) {
      for (const level of ["info", "success", "warning", "error"]) {
        for (const toast_mark of ["stripe", "dot", "none"]) {
          const drawn = colours(draw({ toast_design, toast_mark, toast_more: 1 }, { title: "t", body: "b", level, action: { name: "agent.focus" } }));
          expect(drawn.length).toBeGreaterThan(0);
          expect(drawn.filter((c) => !palette.has(c))).toEqual([]);
        }
      }
    }
  }
  themeNow = named;
});
