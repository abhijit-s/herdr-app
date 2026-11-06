import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The Settings window's Text and Look pages without a window or a core: their steppers
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
  appHome: () => "/home", readTextFile: async () => { if (disk.readFail) throw new Error(disk.readFail); return disk.raw; }, notify: (n: any) => { disk.toasts.push(n); },
  writeTextFile: async (_path: string, text: string, expected: string | null) => {
    disk.writes++;
    if (disk.fail) throw new Error(disk.fail);
    if (expected !== disk.raw) throw new Error("changed since read");
    disk.raw = text;
  },
};
// settings.json as the mod sees it through readTextFile / writeTextFile.
const disk: { raw: string | null; writes: number; fail?: string; readFail?: string; toasts: any[] } = { raw: null, writes: 0, toasts: [] };
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

/** Clicks `id` on the Text page (or `page`) with `person` in settings.json; what gets written. */
function click(person: Record<string, any>, shown: Record<string, any>, id: string, real: string[] = [], page = "text") {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] }, ...shown };
  const view = new Settings();
  view.person = person;
  for (const part of ["layoutPicker", "themePicker", "choicePicker", "fontButton", "fontList"]) if (!real.includes(part)) view[part] = element;
  let written: Record<string, any> | undefined;
  view.change = (edit: (q: Record<string, any>) => void) => {
    written = structuredClone(person);
    edit(written);
  };
  const button = find(view[page](new Proxy({}, { get: () => "#000000" }), {}), id);
  if (real.includes("shown?")) return button;
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

/** Runs `edit` through the Settings window's change() on `raw`; the toasts it posts. */
async function change(raw: string | null, edit: (q: Record<string, any>) => void, fail?: string, readFail?: string) {
  Object.assign(disk, { raw, writes: 0, fail, readFail, toasts: [] });
  const view = new Settings();
  let done: () => void;
  const finished = new Promise<void>((resolve) => (done = resolve));
  view.change(edit, { spawn: (f: (cx: any) => Promise<void>) => f({ notify: () => done() }) });
  await finished;
  return { view, toasts: disk.toasts };
}

test("a change writes settings.json, keeping what else it holds", async () => {
  const { view, toasts } = await change('{"font_size": 15}', (q) => (q.dock_bounce = true));
  expect(JSON.parse(disk.raw!)).toEqual({ font_size: 15, dock_bounce: true });
  expect(view.person).toEqual({ font_size: 15, dock_bounce: true });
  expect(toasts).toEqual([]);
});

test("a broken settings.json is never written over, and says so", async () => {
  const broken = '{"font_size": 15, "theme": "x",}';
  const { toasts } = await change(broken, (q) => (q.dock_bounce = true));
  expect(disk.raw).toBe(broken);
  expect(disk.writes).toBe(0);
  expect(toasts).toEqual([{ title: "settings.json has errors: fix it first", level: "error" }]);
});

test("a write that keeps failing is reported, not dropped", async () => {
  const { toasts } = await change("{}", (q) => (q.dock_bounce = true), "read-only file system");
  expect(disk.writes).toBe(3);
  expect(disk.raw).toBe("{}");
  expect(toasts).toEqual([{ title: "Not saved to settings.json", body: "read-only file system", level: "error" }]);
});

test("settings.json that isn't an object counts as broken", async () => {
  for (const raw of ["null", "[]", "3"]) {
    const { toasts } = await change(raw, (q) => (q.dock_bounce = true));
    expect(disk.raw).toBe(raw);
    expect(disk.writes).toBe(0);
    expect(toasts).toEqual([{ title: "settings.json has errors: fix it first", level: "error" }]);
  }
});

test("a settings.json that can't be read, or an edit that throws, says why", async () => {
  const unread = await change("{}", (q) => (q.dock_bounce = true), undefined, "permission denied");
  expect(disk.writes).toBe(0);
  expect(unread.toasts).toEqual([{ title: "Cannot read settings.json", body: "permission denied", level: "error" }]);
  const thrown = await change("{}", () => { throw new Error("no such section"); });
  expect(disk.writes).toBe(0);
  expect(thrown.toasts).toEqual([{ title: "Cannot read settings.json", body: "no such section", level: "error" }]);
});

test("terminal font weight steps by 25 within 100 to 900", () => {
  expect(click({}, {}, "st-font_weight-plus")).toEqual({ font_weight: 525 });
  expect(click({ font_weight: 425 }, { font_weight: 425 }, "st-font_weight-minus")).toEqual({ font_weight: 400 });
  expect(click({ font_weight: 900 }, { font_weight: 900 }, "st-font_weight-plus")).toEqual({ font_weight: 900 });
  expect(click({ font_weight: 100 }, { font_weight: 100 }, "st-font_weight-minus")).toEqual({ font_weight: 100 });
});

test("the thicken picker writes true or false, and Auto removes font_thicken", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"]);
  expect(pick({ font_size: 15 }, "st-font_thicken-On")).toEqual({ font_size: 15, font_thicken: true });
  expect(pick({ font_size: 15 }, "st-font_thicken-Off")).toEqual({ font_size: 15, font_thicken: false });
  expect(pick({ font_thicken: true }, "st-font_thicken-Off")).toEqual({ font_thicken: false });
  expect(pick({ font_size: 15, font_thicken: true }, "st-font_thicken-Auto")).toEqual({ font_size: 15 });
  expect(pick({ font_thicken: false }, "st-font_thicken-Auto")).toEqual({});
});

test("thicken strength shows only while thickening, and steps by 16 within 0 to 255", () => {
  expect(() => click({}, {}, "st-font_thicken_strength-minus")).toThrow();
  expect(() => click({ font_thicken: false }, {}, "st-font_thicken_strength-minus")).toThrow();
  expect(() => click({ font_thicken: "auto" }, {}, "st-font_thicken_strength-minus")).toThrow();
  const on = { font_thicken: true };
  expect(click(on, {}, "st-font_thicken_strength-minus")).toEqual({ ...on, font_thicken_strength: 240 });
  expect(click({ ...on, font_thicken_strength: 96 }, {}, "st-font_thicken_strength-plus")).toEqual({ ...on, font_thicken_strength: 112 });
  expect(click({ ...on, font_thicken_strength: 0 }, {}, "st-font_thicken_strength-minus")).toEqual({ ...on, font_thicken_strength: 0 });
  expect(click({ ...on, font_thicken_strength: 999 }, {}, "st-font_thicken_strength-minus")).toEqual({ ...on, font_thicken_strength: 240 });
  // Back at 255, the key leaves settings.json.
  expect(click({ ...on, font_thicken_strength: 240 }, {}, "st-font_thicken_strength-plus")).toEqual(on);
  expect(click({ ...on, font_thicken_strength: 250 }, {}, "st-font_thicken_strength-plus")).toEqual(on);
});

test("the thicken pickers show auto, true and false selected, and nothing for an unknown string", () => {
  // choicePicker marks the picked option with a border; the others have none.
  const selected = (person: Record<string, any>, key: string) =>
    ["Auto", "On", "Off"].filter((label) => click(person, {}, `st-${key}-${label}`, ["choicePicker", "shown?"]).calls.border_1);
  for (const key of ["font_thicken", "ui_font_thicken"]) {
    expect(selected({}, key)).toEqual(["Auto"]);
    expect(selected({ [key]: "auto" }, key)).toEqual(["Auto"]);
    expect(selected({ [key]: true }, key)).toEqual(["On"]);
    expect(selected({ [key]: false }, key)).toEqual(["Off"]);
    expect(selected({ [key]: "thin" }, key)).toEqual([]);
  }
});

test("the interface thicken picker writes ui_font_thicken apart from the terminal's", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"]);
  expect(pick({ font_size: 15 }, "st-ui_font_thicken-On")).toEqual({ font_size: 15, ui_font_thicken: true });
  expect(pick({ ui_font_thicken: true }, "st-ui_font_thicken-Off")).toEqual({ ui_font_thicken: false });
  expect(pick({ ui_font_thicken: true }, "st-ui_font_thicken-Auto")).toEqual({});
  expect(pick({ font_thicken: true }, "st-ui_font_thicken-Off")).toEqual({ font_thicken: true, ui_font_thicken: false });
  expect(pick({ ui_font_thicken: true }, "st-font_thicken-Off")).toEqual({ ui_font_thicken: true, font_thicken: false });
});

test("contrast steps by 0.1 within 0 to 1, terminal and interface apart", () => {
  expect(click({}, {}, "st-text_contrast-plus")).toEqual({ text_contrast: 0.1 });
  expect(click({ text_contrast: 0.3 }, {}, "st-text_contrast-plus")).toEqual({ text_contrast: 0.4 });
  expect(click({ text_contrast: 1 }, {}, "st-text_contrast-plus")).toEqual({ text_contrast: 1 });
  expect(click({ text_contrast: 7 }, {}, "st-text_contrast-minus")).toEqual({ text_contrast: 0.9 });
  // Back at 0, the key leaves settings.json.
  expect(click({ text_contrast: 0.1 }, {}, "st-text_contrast-minus")).toEqual({});
  expect(click({}, {}, "st-text_contrast-minus")).toEqual({});
  expect(click({ text_contrast: 0.5 }, {}, "st-ui_text_contrast-plus")).toEqual({ text_contrast: 0.5, ui_text_contrast: 0.1 });
  expect(click({ ui_text_contrast: 0.2, text_contrast: 0.5 }, {}, "st-ui_text_contrast-minus")).toEqual({ ui_text_contrast: 0.1, text_contrast: 0.5 });
});

test("faint opacity steps by 0.1 within 0 to 1 and leaves Ghostty's 0.5 out", () => {
  expect(click({}, {}, "st-faint_opacity-plus")).toEqual({ faint_opacity: 0.6 });
  expect(click({}, {}, "st-faint_opacity-minus")).toEqual({ faint_opacity: 0.4 });
  expect(click({ faint_opacity: 0.6 }, {}, "st-faint_opacity-minus")).toEqual({});
  expect(click({ faint_opacity: 1 }, {}, "st-faint_opacity-plus")).toEqual({ faint_opacity: 1 });
  expect(click({ faint_opacity: 0.1 }, {}, "st-faint_opacity-minus")).toEqual({ faint_opacity: 0 });
});

test("interface thicken strength shows only while it's on, and steps by 16 within 0 to 255", () => {
  expect(() => click({}, {}, "st-ui_font_thicken_strength-minus")).toThrow();
  expect(() => click({ font_thicken: true }, {}, "st-ui_font_thicken_strength-minus")).toThrow();
  expect(() => click({ ui_font_thicken: false }, {}, "st-ui_font_thicken_strength-minus")).toThrow();
  const on = { ui_font_thicken: true };
  expect(click(on, {}, "st-ui_font_thicken_strength-minus")).toEqual({ ...on, ui_font_thicken_strength: 240 });
  expect(click({ ...on, ui_font_thicken_strength: 96 }, {}, "st-ui_font_thicken_strength-plus")).toEqual({ ...on, ui_font_thicken_strength: 112 });
  expect(click({ ...on, ui_font_thicken_strength: 0 }, {}, "st-ui_font_thicken_strength-minus")).toEqual({ ...on, ui_font_thicken_strength: 0 });
  expect(click({ ...on, ui_font_thicken_strength: 240 }, {}, "st-ui_font_thicken_strength-plus")).toEqual(on);
  expect(click({ ...on, font_thicken_strength: 96 }, {}, "st-ui_font_thicken_strength-minus")).toEqual({ ...on, font_thicken_strength: 96, ui_font_thicken_strength: 240 });
});

test("interface font weight steps by 25 within 100 to 900, leaving 400 out of settings.json", () => {
  expect(click({}, {}, "st-ui_font_weight-plus")).toEqual({ ui_font_weight: 425 });
  expect(click({ font_size: 15 }, {}, "st-ui_font_weight-minus")).toEqual({ font_size: 15, ui_font_weight: 375 });
  expect(click({ ui_font_weight: 425 }, {}, "st-ui_font_weight-minus")).toEqual({});
  expect(click({ ui_font_weight: 900 }, {}, "st-ui_font_weight-plus")).toEqual({ ui_font_weight: 900 });
  expect(click({ ui_font_weight: 100 }, {}, "st-ui_font_weight-minus")).toEqual({ ui_font_weight: 100 });
  // Out of range in settings.json: Herdr uses the end, so the first click moves off it.
  expect(click({ ui_font_weight: 950 }, {}, "st-ui_font_weight-minus")).toEqual({ ui_font_weight: 875 });
  expect(click({ font_weight: 300 }, { font_weight: 300 }, "st-ui_font_weight-plus")).toEqual({ font_weight: 300, ui_font_weight: 425 });
  expect(click({ ui_font_weight: 600 }, {}, "st-font_weight-plus")).toEqual({ ui_font_weight: 600, font_weight: 525 });
});

test("text smoothing pickers write subpixel and leave grayscale out, terminal and interface apart", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"]);
  expect(pick({}, "st-tr-Subpixel")).toEqual({ text_rendering: "subpixel" });
  expect(pick({ text_rendering: "subpixel" }, "st-tr-Grayscale")).toEqual({});
  expect(pick({}, "st-uitr-Subpixel")).toEqual({ ui_text_rendering: "subpixel" });
  expect(pick({ ui_text_rendering: "subpixel", text_rendering: "subpixel" }, "st-uitr-Grayscale")).toEqual({ text_rendering: "subpixel" });
});

test("the new tabs arrangement writes its mode and leaves off out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "look");
  expect(pick({}, "st-da-BSP")).toEqual({ default_arrange: "bsp" });
  expect(pick({ default_arrange: "bsp" }, "st-da-Tiled")).toEqual({ default_arrange: "tiled" });
  expect(pick({ default_arrange: "rows", zoom: 1.1 }, "st-da-Off")).toEqual({ zoom: 1.1 });
});

test("dim when inactive sits on the look page, its scope writes text and leaves window out", () => {
  expect(click({}, {}, "st-unfocused_dim-plus", [], "look")).toEqual({ unfocused_dim: 0.1 });
  expect(click({ unfocused_dim: 0.3 }, {}, "st-unfocused_dim-minus", [], "look")).toEqual({ unfocused_dim: 0.2 });
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "look");
  expect(pick({ unfocused_dim: 0.3 }, "st-ds-Text")).toEqual({ unfocused_dim: 0.3, unfocused_dim_scope: "text" });
  expect(pick({ unfocused_dim: 0.3, unfocused_dim_scope: "text" }, "st-ds-Window")).toEqual({ unfocused_dim: 0.3 });
  expect(pick({}, "st-ss-Darker when active")).toEqual({ sidebar_shade: "active" });
  expect(pick({}, "st-ss-Always darker")).toEqual({ sidebar_shade: "always" });
  expect(pick({ sidebar_shade: "active" }, "st-ss-Flat")).toEqual({});
});

test("subpixel order shows only while either text is subpixel, and leaves rgb out", () => {
  const shown = (person: Record<string, any>) => click(person, {}, "st-so-BGR", ["choicePicker", "shown?"]) !== undefined;
  expect(shown({})).toBe(false);
  expect(shown({ text_rendering: "subpixel" })).toBe(true);
  expect(shown({ ui_text_rendering: "subpixel" })).toBe(true);
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"]);
  expect(pick({ text_rendering: "subpixel" }, "st-so-BGR")).toEqual({ text_rendering: "subpixel", subpixel_order: "bgr" });
  expect(pick({ text_rendering: "subpixel", subpixel_order: "bgr" }, "st-so-RGB")).toEqual({ text_rendering: "subpixel" });
});

test("resume agents after restart sits on the alerts page, writes auto or off and leaves ask out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "alerts");
  expect(pick({}, "st-ra-Auto")).toEqual({ resume_agents: "auto" });
  expect(pick({ resume_agents: "auto" }, "st-ra-Off")).toEqual({ resume_agents: "off" });
  expect(pick({ resume_agents: "off", zoom: 1.1 }, "st-ra-Ask")).toEqual({ zoom: 1.1 });
});
