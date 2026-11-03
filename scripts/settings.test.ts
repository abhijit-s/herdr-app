import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The Settings window's Look section without a window or a core: its steppers
// and what a click writes to settings.json.
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
const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");
let composed: Record<string, any> = {};
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, image: element, v_flex: element, h_flex: element,
  Scrollbar: element, Input: { new: element }, InputState: { new: () => ({ on() {}, focus() {} }) },
  settings: () => composed, window: () => ({ platform: "linux" }), props: () => ({}),
};
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap(m => m[1].split(",").map(n => n.trim()));
const args = names.map(name => values[name] ?? (() => undefined));
const body = source
  .replace(/^import .*;\n/gm, "")
  .replace("export function scaleStep", "function scaleStep")
  .replace("export default class Settings", "const Settings = class Settings")
  + "\nreturn { Settings, scaleStep };";
const { Settings, scaleStep } = new Function(...names, body)(...args);

function find(root: Element, id: string): Element {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined as any;
}

/** Clicks `id` in the Look section with `person` in settings.json; what gets written. */
function click(person: Record<string, any>, shown: Record<string, any>, id: string) {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] }, ...shown };
  const view = new Settings();
  view.person = person;
  for (const part of ["layoutPicker", "themePicker", "choicePicker", "fontButton", "fontList"]) view[part] = element;
  let written: Record<string, any> | undefined;
  view.change = (edit: (q: Record<string, any>) => void) => {
    written = structuredClone(person);
    edit(written);
  };
  const button = find(view.look(new Proxy({}, { get: () => "#000000" }), {}), id);
  expect(button).toBeDefined();
  button.calls.on_click[0](null, {});
  return written;
}

test("interface scale steppers step 5% from the value in use", () => {
  expect(click({}, { ui_zoom: 1, ui_text_scale: 1 }, "st-ui_zoom-plus")).toEqual({ ui_zoom: 1.05 });
  expect(click({ ui_text_scale: 1.1 }, { ui_zoom: 1, ui_text_scale: 1.1 }, "st-ui_text_scale-minus")).toEqual({ ui_text_scale: 1.05 });
  // Out of range in settings.json: Herdr uses the end, so the first click moves off it.
  expect(click({ ui_zoom: 9 }, { ui_zoom: 1.5, ui_text_scale: 1 }, "st-ui_zoom-minus")).toEqual({ ui_zoom: 1.45 });
  expect(click({ ui_zoom: 1.5 }, { ui_zoom: 1.5, ui_text_scale: 1 }, "st-ui_zoom-plus")).toEqual({ ui_zoom: 1.5 });
  // Back at 100%, the key leaves settings.json.
  expect(click({ ui_zoom: 1.05, font_size: 15 }, { ui_zoom: 1.05, ui_text_scale: 1 }, "st-ui_zoom-minus")).toEqual({ font_size: 15 });
});

test("scale steps stay in the range Rust publishes, in hundredths", () => {
  const f32 = (v: number) => Math.fround(v);
  expect(scaleStep(undefined, 1, 0.05, [f32(0.8), f32(1.3)])).toBe(1.05);
  expect(scaleStep(0.85, 0.85, -0.05, [f32(0.8), f32(1.3)])).toBe(0.8);
  expect(scaleStep(0.8, 0.8, -0.05, [f32(0.8), f32(1.3)])).toBe(0.8);
  expect(scaleStep("big", 1.2, 0.05, [0.75, 1.5])).toBe(1.25);
  expect(scaleStep(Number.NaN, 1, -0.05, [0.75, 1.5])).toBe(0.95);
});
