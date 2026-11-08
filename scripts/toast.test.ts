import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The toast mod without a window: what it draws for each `toast_mark`.
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
const values: Record<string, any> = {
  View: class {}, div: element, image: element, v_flex: element, h_flex: element,
  props: () => propsNow, closeOverlay() {}, run() {},
  theme: () => new Proxy({}, { get: (_t, key: string) => `#${key}` }),
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
