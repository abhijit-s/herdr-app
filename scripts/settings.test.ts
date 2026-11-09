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
// The window's props: app.settings' section.
let propsNow: Record<string, any> = {};
// The window: its platform, and whether it's the active one.
let windowNow: Record<string, any> = { platform: "linux" };
// What toastLooks() gives the Settings window's toast previews.
let looksNow: Record<string, any> = {};
// What themes() lists, and shared.json apart from the layout's name.
let themeList: any[] = [];
const sharedDisk: Record<string, any> = {};
// What `hc machine ls` lists.
let machineDisk: any[] = [];
// An input field: its text, whether it was focused, and the range last selected.
const fakeInput = () => {
  const input = { text: "", focused: false, selected: null as any, on() {}, focus() { input.focused = true; }, value: () => input.text, set_value(v: string) { input.text = v; }, set_selected_range(r: any) { input.selected = r; } };
  return input;
};
// The app's layoutSpacing() for a tree: the mod's own spacingFieldsOf, set once the mod loads.
let fieldsOf: (tree: any) => any = () => null;
const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");
let composed: Record<string, any> = {};
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, image: element, v_flex: element, h_flex: element,
  Scrollbar: { vertical: element }, Input: { new: element }, InputState: { new: () => fakeInput() },
  // The theme grid's list: what it was given, and where its handle was asked to scroll.
  v_virtual_list: (...args: any[]) => { const list = element(); list.calls.v_virtual_list = args; return list; },
  VirtualListScrollHandle: { new: () => { const handle = { scrolls: [] as any[], scroll_to_item(i: number, how: string) { handle.scrolls.push([i, how]); } }; return handle; } },
  toastLooks: () => looksNow,
  themes: () => themeList, layouts: () => ["win98"], setShared: (key: string, value: any) => { sharedDisk[key] = value; },
  settings: () => ({ theme_dark: "ayu-mirage", theme_light: "ayu-light", ...composed }), window: () => windowNow, props: () => propsNow,
  // Pages search reads besides Text and Look: a build from source that checks nothing.
  updates: () => ({ app: { version: "0.1.0", updates: false }, core: { version: "0.1.17" }, checks: false }), telemetry: () => null,
  hc: async (args: string[]) => (args.join(" ") === "machine ls --json" ? { code: 0, stdout: JSON.stringify({ machines: machineDisk }) } : { code: 1, stderr: "no" }),
  // Herdr's preview of a pane_header_info template: here, the template as one segment (a test may swap it).
  headerInfoPreview: (template: string) => previewOf(template),
  // Herdr's preview of an agents_card_info template on the sample agent: here, each segment as written (a test may swap it).
  cardInfoPreview: (template: string | null) => cardPreviewOf(template),
  appHome: () => "/home", notify: (n: any) => { disk.toasts.push(n); },
  shared: (key: string) => (key === "layout" ? layoutDisk.name : sharedDisk[key]), run: (name: string, args: any) => { layoutDisk.runs.push([name, args]); },
  layout: () => { layoutDisk.layoutReads++; return layoutDisk.shown; },
  layoutSpacing: () => fieldsOf(layoutDisk.shown),
  readTextFile: async (path: string) => {
    if (path.startsWith("/home/layouts/")) {
      if (layoutDisk.readFail) throw new Error(layoutDisk.readFail);
      return path === `/home/layouts/${layoutDisk.name}.json` ? layoutDisk.raw : null;
    }
    if (disk.readFail) throw new Error(disk.readFail);
    return disk.raw;
  },
  writeTextFile: async (path: string, text: string, expected: string | null) => {
    if (path.startsWith("/home/layouts/")) {
      layoutDisk.writes++;
      if (path !== `/home/layouts/${layoutDisk.name}.json` || expected !== layoutDisk.raw) throw new Error("changed since read");
      layoutDisk.raw = text;
      return;
    }
    disk.writes++;
    if (disk.fail) throw new Error(disk.fail);
    if (expected !== disk.raw) throw new Error("changed since read");
    disk.raw = text;
  },
};
// What cardInfoPreview gives: the default resolved as Herdr does, else the template's segments as written.
let cardPreviewOf = (template: string | null): any => {
  const parts = (template ?? "@{remote} · {space}").split(/[·|]/).map((p) => p.trim()).filter(Boolean)
    .map((p) => p.replace("{remote}", "vps").replace("{space}", "herdr-app"));
  return parts.length ? { text: parts.join(" · "), segments: parts.map((text, i) => ({ sep: i ? " · " : "", text, ask: false })) } : null;
};
// What headerInfoPreview gives for a template.
let previewOf = (template: string): any => ({ text: template, segments: [{ sep: "", text: template, ask: false }] });
// settings.json as the mod sees it through readTextFile / writeTextFile.
const disk: { raw: string | null; writes: number; fail?: string; readFail?: string; toasts: any[] } = { raw: null, writes: 0, toasts: [] };
// Timers the Settings window starts, and a click's context that records them.
type FakeTimer = { ms: number; f: (cx: any) => void; every: boolean; cancelled: boolean; cancel(): void; is_done(): boolean };
const timers: FakeTimer[] = [];
const fakeTimer = (every: boolean) => (ms: number, f: (cx: any) => void) => {
  const t: FakeTimer = { ms, f, every, cancelled: false, cancel() { t.cancelled = true; }, is_done: () => t.cancelled };
  timers.push(t);
  return t;
};
const clickCx = () => ({ notify() {}, timer: { after: fakeTimer(false), every: fakeTimer(true) } });
const live = () => timers.filter((t) => !t.cancelled);
// The shown layout's file (layouts/<name>.json), apart from settings.json.
const layoutDisk: { name: string; raw: string | null; shown: any; writes: number; runs: any[]; layoutReads: number; readFail?: string } = { name: "mine", raw: null, shown: null, writes: 0, runs: [], layoutReads: 0 };
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap(m => m[1].split(",").map(n => n.trim()));
const args = names.map(name => values[name] ?? (() => undefined));
const body = source
  .replace(/^import .*;\n/gm, "")
  .replace(/^export function /gm, "function ")
  .replace("export default class Settings", "const Settings = class Settings")
  + "\nreturn { Settings, scaleStep, spacingOf, spacingFieldsOf, sameSpacing, settlePending, searchWords, wordSpan, matchSetting, markRuns, pageFor, over, contrast, tonal, forkTag };";
const { Settings, scaleStep, spacingOf, spacingFieldsOf, sameSpacing, settlePending, searchWords, wordSpan, matchSetting, markRuns, pageFor, over, contrast, tonal, forkTag } = new Function(...names, body)(...args);
fieldsOf = spacingFieldsOf;
const spacingOfTree = (tree: any) => spacingOf(spacingFieldsOf(tree));

function find(root: Element, id: string): Element {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined as any;
}

/** Clicks `id` on the Text group's pages (or `page`) with `person` in settings.json; what gets written. */
function click(person: Record<string, any>, shown: Record<string, any>, id: string, real: string[] = [], page: string | string[] = ["zoom", "fonts", "grid", "rendering"]) {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] }, ...shown };
  const view = new Settings();
  view.person = person;
  for (const part of ["layoutPicker", "spacing", "choicePicker", "fontButton", "fontList"]) if (!real.includes(part)) view[part] = element;
  let written: Record<string, any> | undefined;
  view.change = (edit: (q: Record<string, any>) => void) => {
    written = structuredClone(person);
    edit(written);
  };
  const theme = new Proxy({}, { get: () => "#000000" });
  const button = [page].flat().map((p) => find(view.page(p, theme, {}), id)).find(Boolean);
  if (real.includes("shown?")) return button;
  expect(button).toBeDefined();
  button.calls.on_click[0](null, {});
  return written;
}

test("dock badge: on unless false, and a click toggles the key in settings.json", () => {
  const toggle = (person: Record<string, any>) => click(person, {}, "st-dock_badge", ["choicePicker"], "alerts");
  expect(toggle({})).toEqual({ dock_badge: false });
  expect(toggle({ dock_badge: false, dock_bounce: true })).toEqual({ dock_bounce: true });
});

test("notification sound, marks and icons write settings.json's keys", () => {
  const pick = (person: Record<string, any>, label: string) => click(person, {}, `st-ns-${label}`, ["choicePicker"], "alerts");
  expect(pick({}, "System")).toEqual({ notification_sounds: "default" });
  expect(pick({ notification_sounds: "default" }, "Silent")).toEqual({});
  expect(pick({}, "By state")).toEqual({ notification_sounds: { done: "Glass", blocked: "Purr", error: "Basso" } });
  // A per-state choice of the person's own stays as it is.
  expect(pick({ notification_sounds: { done: "Hero" } }, "By state")).toEqual({ notification_sounds: { done: "Hero" } });
  expect(pick({ notification_sounds: "Ping" }, "Silent")).toEqual({});
  const toggle = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "alerts");
  expect(toggle({}, "st-notification_marks")).toEqual({ notification_marks: true });
  expect(toggle({ notification_marks: true }, "st-notification_marks")).toEqual({});
  expect(toggle({}, "st-notification_icons")).toEqual({ notification_icons: false });
  expect(toggle({ notification_icons: false }, "st-notification_icons")).toEqual({});
});

test("interface scale steppers step 5% from the value in use", () => {
  expect(click({}, { ui_zoom: 1, ui_text_scale: 1 }, "st-ui_zoom-plus")).toEqual({ ui_zoom: 1.05 });
  expect(click({ ui_text_scale: 1.1 }, { ui_zoom: 1, ui_text_scale: 1.1 }, "st-ui_text_scale-minus")).toEqual({ ui_text_scale: 1.05 });
  // Out of range in settings.json: Herdr uses the end, so the first click moves off it.
  expect(click({ ui_zoom: 9 }, { ui_zoom: 1.5, ui_text_scale: 1 }, "st-ui_zoom-minus")).toEqual({ ui_zoom: 1.45 });
  expect(click({ ui_zoom: 1.5 }, { ui_zoom: 1.5, ui_text_scale: 1 }, "st-ui_zoom-plus")).toEqual({ ui_zoom: 1.5 });
  // Back at 100%, the key leaves settings.json.
  expect(click({ ui_zoom: 1.05, font_size: 15 }, { ui_zoom: 1.05, ui_text_scale: 1 }, "st-ui_zoom-minus")).toEqual({ font_size: 15 });
});

test("pane zoom keys: on unless false, and a click toggles the key in settings.json", () => {
  // On by default: the click writes false.
  expect(click({}, {}, "st-pane_zoom_keys")).toEqual({ pane_zoom_keys: false });
  expect(click({ zoom: 1.1 }, { pane_zoom_keys: true }, "st-pane_zoom_keys")).toEqual({ zoom: 1.1, pane_zoom_keys: false });
  // Off: the click turns it back on by leaving the key out.
  expect(click({ pane_zoom_keys: false, zoom: 1.1 }, { pane_zoom_keys: false }, "st-pane_zoom_keys")).toEqual({ zoom: 1.1 });
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
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "panes");
  expect(pick({}, "st-da-BSP")).toEqual({ default_arrange: "bsp" });
  expect(pick({ default_arrange: "bsp" }, "st-da-Tiled")).toEqual({ default_arrange: "tiled" });
  expect(pick({ default_arrange: "rows", zoom: 1.1 }, "st-da-Off")).toEqual({ zoom: 1.1 });
});

test("focus border width writes 1 to 3 px and leaves the layout's width out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "panes");
  expect(pick({}, "st-fw-2 px")).toEqual({ focus_border_width: 2 });
  expect(pick({ focus_border_width: 2 }, "st-fw-3 px")).toEqual({ focus_border_width: 3 });
  expect(pick({ focus_border_width: 3, zoom: 1.1 }, "st-fw-Layout")).toEqual({ zoom: 1.1 });
});

test("focus border picks a theme color into colors.focus and from theme takes it out", () => {
  const theme = new Proxy({ ansi1: "#E06C75", ansi4: "#61afef", terminal_foreground: "#abb2bf" } as Record<string, string>, { get: (o, k: string) => o[k] ?? (k.startsWith("ansi") ? undefined : "#000000") });
  const swatch = (person: Record<string, any>, id: string) => {
    composed = {};
    const view = new Settings();
    view.person = person;
    for (const part of ["layoutPicker", "spacing", "choicePicker", "fontButton", "fontList", "iconPicker"]) view[part] = element;
    let written: Record<string, any> | undefined;
    view.change = (edit: (q: Record<string, any>) => void) => { written = structuredClone(person); edit(written); };
    const button = find(view.page("panes", theme, {}), id);
    if (!button) return undefined;
    button.calls.on_click[0](null, {});
    return written;
  };
  expect(swatch({}, "st-fc-#e06c75")).toEqual({ colors: { focus: "#e06c75" } });
  expect(swatch({ colors: { accent: "#123456" } }, "st-fc-#61afef")).toEqual({ colors: { accent: "#123456", focus: "#61afef" } });
  expect(swatch({ colors: { focus: "#61afef" }, zoom: 1.1 }, "st-fc-theme")).toEqual({ zoom: 1.1 });
  expect(swatch({ colors: { focus: "#61afef", accent: "#123456" } }, "st-fc-theme")).toEqual({ colors: { accent: "#123456" } });
  // A color written by hand shows as its own swatch.
  expect(swatch({ colors: { focus: "#FF8800" } }, "st-fc-#ff8800")).toEqual({ colors: { focus: "#ff8800" } });
  expect(swatch({}, "st-fc-#ff8800")).toBeUndefined();
});

test("name panes automatically sits on the Panes page, writes true and leaves off out", () => {
  expect(click({}, {}, "st-pane_auto_names", [], "panes")).toEqual({ pane_auto_names: true });
  expect(click({ pane_auto_names: true, zoom: 1.1 }, {}, "st-pane_auto_names", [], "panes")).toEqual({ zoom: 1.1 });
  const on = (person: Record<string, any>) => click(person, {}, "st-pane_auto_names", ["shown?"], "panes").calls.justify_end !== undefined;
  expect(on({ pane_auto_names: true })).toBe(true);
  expect(on({})).toBe(false);
  expect(on({ pane_auto_names: false })).toBe(false);
  const shown = texts(search("pane_auto_names").root).join("|");
  expect(shown).toContain("Appearance › Panes|PANES|Name");
  expect(shown).toContain("pane|_|auto|_|names");
});

test("the working spinner picker previews each choice, writes its name and leaves the ring out", () => {
  const presets = { working_spinner_presets: [{ name: "braille", frames: ["⠋", "⠙"], interval_ms: 80 }, { name: "line", frames: ["|", "/"], interval_ms: 100 }] };
  expect(click({}, presets, "st-ws-braille", [], "activity")).toEqual({ working_spinner: "braille" });
  expect(click({ working_spinner: "braille", zoom: 1.1 }, presets, "st-ws-line", [], "activity")).toEqual({ working_spinner: "line", zoom: 1.1 });
  expect(click({ working_spinner: "line", zoom: 1.1 }, presets, "st-ws-ring", [], "activity")).toEqual({ zoom: 1.1 });
  // What Herdr resolved: frames for a name it knows (or your own), null for the ring or a name it doesn't.
  const drawn = (v: any) => (v && v !== "ring" && v !== "spiral" ? { frames: ["x"], interval_ms: 80 } : null);
  const chip = (person: Record<string, any>, name: string) => click(person, { ...presets, working_spinner: drawn(person.working_spinner) }, `st-ws-${name}`, ["shown?"], "activity");
  const preview = (name: string) => chip({}, name).nodes[0].calls;
  expect(preview("braille").frames).toEqual([["⠋", "⠙"], 80]);
  expect(preview("ring").spin).toEqual([900]);
  const chosen = (person: Record<string, any>, name: string) => chip(person, name).calls.bg !== undefined;
  expect(chosen({}, "ring")).toBe(true);
  expect(chosen({ working_spinner: "line" }, "line")).toBe(true);
  expect(chosen({ working_spinner: "spiral" }, "ring")).toBe(true);
  expect(chosen({ working_spinner: "line" }, "ring")).toBe(false);
  expect(["ring", "braille", "line"].some((n) => chosen({ working_spinner: { frames: ["a"], interval_ms: 80 } }, n))).toBe(false);
});

test("dim when inactive sits on the Window page, its scope writes text and leaves window out", () => {
  expect(click({}, {}, "st-unfocused_dim-plus", [], "window")).toEqual({ unfocused_dim: 0.1 });
  expect(click({ unfocused_dim: 0.3 }, {}, "st-unfocused_dim-minus", [], "window")).toEqual({ unfocused_dim: 0.2 });
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "window");
  expect(pick({ unfocused_dim: 0.3 }, "st-ds-Text")).toEqual({ unfocused_dim: 0.3, unfocused_dim_scope: "text" });
  expect(pick({ unfocused_dim: 0.3, unfocused_dim_scope: "text" }, "st-ds-Window")).toEqual({ unfocused_dim: 0.3 });
  expect(pick({}, "st-ss-Darker when active")).toEqual({ sidebar_shade: "active" });
  expect(pick({}, "st-ss-Always darker")).toEqual({ sidebar_shade: "always" });
  expect(pick({ sidebar_shade: "active" }, "st-ss-Flat")).toEqual({});
  expect(pick({}, "st-pc-Classic")).toEqual({ panel_colors: "classic" });
  expect(pick({ panel_colors: "classic" }, "st-pc-Tinted")).toEqual({});
  const shadeShown = (person: Record<string, any>) => click(person, {}, "st-ss-Flat", ["choicePicker", "shown?"], "window") !== undefined;
  expect(shadeShown({})).toBe(true);
  expect(shadeShown({ panel_colors: "classic" })).toBe(false);
  const shadeShownOver = (inUse: string) => click({}, { panels_in_use: inUse }, "st-ss-Flat", ["choicePicker", "shown?"], "window") !== undefined;
  expect(shadeShownOver("tinted")).toBe(true);
  expect(shadeShownOver("classic")).toBe(false);
});

test("notification position sits on the Window page: six places, bottom right left out", () => {
  const pick = (person: Record<string, any>, value: string) => click(person, {}, `st-tp-${value}`, [], "window");
  for (const value of ["top-left", "top-center", "top-right", "bottom-left", "bottom-center"]) {
    expect(pick({}, value)).toEqual({ toast_position: value });
  }
  expect(pick({ toast_position: "top-left" }, "bottom-right")).toEqual({});
  // The offset, settings.json's own, stays as it is.
  expect(pick({ toast_offset: { y: 24 } }, "top-center")).toEqual({ toast_offset: { y: 24 }, toast_position: "top-center" });
});

test("notification time, style, corners and level mark sit beside the position, each default left out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "window");
  expect(pick({}, "st-td-Short")).toEqual({ toast_duration: "short" });
  expect(pick({}, "st-td-Long")).toEqual({ toast_duration: "long" });
  expect(pick({}, "st-td-Until clicked")).toEqual({ toast_duration: "until-dismissed" });
  expect(pick({ toast_duration: "long" }, "st-td-Normal")).toEqual({});
  expect(pick({}, "st-tr-Square")).toEqual({ toast_corners: "square" });
  expect(pick({}, "st-tr-Slight")).toEqual({ toast_corners: "slight" });
  expect(pick({ toast_corners: "square" }, "st-tr-Rounded")).toEqual({});
  expect(pick({}, "st-tm-Dot")).toEqual({ toast_mark: "dot" });
  expect(pick({}, "st-tm-None")).toEqual({ toast_mark: "none" });
  expect(pick({ toast_mark: "none" }, "st-tm-Stripe")).toEqual({});
  const style = (person: Record<string, any>, value: string) => click(person, {}, `st-ts-${value}`, [], "window");
  for (const value of ["subtle", "high-contrast", "accent"]) expect(style({}, value)).toEqual({ toast_style: value });
  expect(style({ toast_style: "accent", toast_mark: "dot" }, "standard")).toEqual({ toast_mark: "dot" });
});

test("each style's preview is a small toast in that style's colours, with the chosen corners and mark", () => {
  const styles = {
    standard: { toast: "#202020", toast_border: "#505050", toast_foreground: "#eeeeee", toast_info: "#3b82f6" },
    subtle: { toast: "#181818", toast_border: "#303030", toast_foreground: "#dddddd", toast_info: "#2563eb" },
  };
  looksNow = { styles, radius: { square: 0, slight: 4, rounded: 8 } };
  const cell = (person: Record<string, any>, value: string) => click(person, {}, `st-ts-${value}`, ["shown?"], "window");
  const toast = (person: Record<string, any>, value: string) => cell(person, value).nodes[0];
  expect(toast({}, "subtle").calls.bg).toEqual(["#181818"]);
  expect(toast({}, "subtle").calls.border_color).toEqual(["#303030"]);
  expect(find(cell({}, "subtle"), "st-ts-subtle-stripe").calls.bg).toEqual(["#2563eb"]);
  expect(toast({}, "standard").calls.rounded).toEqual([4]);
  expect(toast({ toast_corners: "square" }, "standard").calls.rounded).toEqual([0]);
  expect(toast({ toast_corners: "slight" }, "standard").calls.rounded).toEqual([2]);
  const marks = (person: Record<string, any>) => ["stripe", "dot"].filter((m) => find(cell(person, "standard"), `st-ts-standard-${m}`));
  expect(marks({})).toEqual(["stripe", "dot"]);
  expect(marks({ toast_mark: "dot" })).toEqual(["dot"]);
  expect(marks({ toast_mark: "none" })).toEqual([]);
  // A style Herdr didn't send draws in the theme's own toast colours.
  expect(toast({}, "accent").calls.bg).toEqual(["#000000"]);
  // The radii are Herdr's own.
  looksNow = { styles, radius: { square: 1, slight: 3, rounded: 10 } };
  expect(toast({ toast_corners: "square" }, "standard").calls.rounded).toEqual([0.5]);
  expect(toast({}, "standard").calls.rounded).toEqual([5]);
  // The chosen design's own corners and mark, as Herdr says it draws them.
  looksNow = { styles, radius: { square: 0, slight: 4, rounded: 8 }, takes: { terminal: { corners: false, mark: false, radius: 0, draws: "none" }, pill: { corners: false, mark: true, radius: 999, draws: "dot" } } };
  expect(toast({ toast_design: "terminal" }, "standard").calls.rounded).toEqual([0]);
  expect(marks({ toast_design: "terminal" })).toEqual([]);
  expect(marks({ toast_design: "pill" })).toEqual(["dot"]);
  looksNow = {};
});

test("notification design: six designs, card left out; corners and mark only where the design takes them", () => {
  const design = (person: Record<string, any>, value: string) => click(person, {}, `st-tg-${value}`, [], "window");
  for (const value of ["pill", "terminal", "outline", "icon-card", "banner"]) expect(design({}, value)).toEqual({ toast_design: value });
  expect(design({ toast_design: "banner", toast_style: "subtle" }, "card")).toEqual({ toast_style: "subtle" });
  const shown = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker", "shown?"], "window") !== undefined;
  const rows = (toast_design?: string) => [shown({ toast_design }, "st-tr-Square"), shown({ toast_design }, "st-tm-Dot")];
  // Without Herdr's word, both rows show.
  looksNow = {};
  expect(rows("terminal")).toEqual([true, true]);
  // Herdr says which designs take corners and marks (toastLooks().takes, from ToastDesign::takes).
  const takes = (corners: boolean, mark: boolean) => ({ corners, mark });
  looksNow = { takes: { card: takes(true, true), pill: takes(false, true), terminal: takes(false, false), outline: takes(true, false), "icon-card": takes(true, false), banner: takes(false, true) } };
  expect(rows()).toEqual([true, true]);
  expect(rows("card")).toEqual([true, true]);
  expect(rows("pill")).toEqual([false, true]);
  expect(rows("terminal")).toEqual([false, false]);
  expect(rows("outline")).toEqual([true, false]);
  expect(rows("icon-card")).toEqual([true, false]);
  expect(rows("banner")).toEqual([false, true]);
  looksNow = {};
});

test("each design's preview is a small toast in that design's colours, from toastLooks()", () => {
  const card = { toast: "#202020", toast_border: "#505050", toast_foreground: "#eeeeee", toast_info: "#3b82f6" };
  looksNow = {
    designs: { card, pill: card, terminal: { ...card, toast_info: "#5599ff" }, outline: { ...card, toast_ghost: "#202020b3" }, "icon-card": card, banner: card },
    radius: { square: 0, slight: 4, rounded: 8 },
    font_family: "Iosevka",
  };
  const cell = (person: Record<string, any>, value: string) => click(person, {}, `st-tg-${value}`, ["shown?"], "window");
  const toast = (value: string, person: Record<string, any> = {}) => find(cell(person, value), `st-tg-${value}-toast`);
  expect(toast("card").calls.bg).toEqual(["#202020"]);
  expect(toast("card").calls.rounded).toEqual([4]);
  expect(toast("card", { toast_corners: "square" }).calls.rounded).toEqual([0]);
  expect(toast("pill").calls.rounded).toEqual([7]);
  expect(toast("terminal").calls.font_family).toEqual(["Iosevka"]);
  expect(find(cell({}, "terminal"), "st-tg-terminal-tag").calls.text_color).toEqual(["#5599ff"]);
  expect(toast("terminal").calls.rounded).toBeUndefined();
  expect([toast("outline").calls.bg, toast("outline").calls.border_color]).toEqual([["#202020b3"], ["#3b82f6"]]);
  expect(find(cell({}, "icon-card"), "st-tg-icon-card-icon").calls.bg).toEqual(["#3b82f6"]);
  expect(toast("banner").calls.w_full).toEqual([]);
  // A design Herdr didn't send draws in the theme's own toast colours.
  looksNow = {};
  expect(toast("pill").calls.bg).toEqual(["#000000"]);
});

test("notification countdown toggles toast_progress, off left out", () => {
  const toggle = (person: Record<string, any>) => click(person, {}, "st-toast_progress", [], "window");
  expect(toggle({})).toEqual({ toast_progress: true });
  expect(toggle({ toast_progress: true, toast_style: "accent" })).toEqual({ toast_style: "accent" });
});

test("search finds the toast settings by their keys and words", () => {
  for (const [q, title] of [["toast_duration", "Notification time"], ["toast style", "Notification style"], ["corners", "Notification corners"], ["toast_mark", "Notification level mark"], ["toast_design", "Notification design"], ["banner", "Notification design"], ["toast_progress", "Notification countdown"], ["countdown", "Notification countdown"]]) {
    expect(texts(search(q).root).join("")).toContain(title);
  }
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

test("resume agents after restart sits on the Activity page, writes auto or off and leaves ask out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "activity");
  expect(pick({}, "st-ra-Auto")).toEqual({ resume_agents: "auto" });
  expect(pick({ resume_agents: "auto" }, "st-ra-Off")).toEqual({ resume_agents: "off" });
  expect(pick({ resume_agents: "off", zoom: 1.1 }, "st-ra-Ask")).toEqual({ zoom: 1.1 });
});

test("sub-agents sits on the Activity page, writes folded or off and leaves tree out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "activity");
  expect(pick({}, "st-sa-Folded")).toEqual({ agents_subagents: "folded" });
  expect(pick({ agents_subagents: "folded" }, "st-sa-Off")).toEqual({ agents_subagents: "off" });
  expect(pick({ agents_subagents: "off", zoom: 1.1 }, "st-sa-Tree")).toEqual({ zoom: 1.1 });
});

test("sub-agent click sits right after sub-agents on the Activity page, writes transcript and leaves claude out", () => {
  const pick = (person: Record<string, any>, id: string) => click(person, {}, id, ["choicePicker"], "activity");
  expect(pick({}, "st-sc-Transcript")).toEqual({ agents_subagent_click: "transcript" });
  expect(pick({ agents_subagent_click: "transcript", zoom: 1.1 }, "st-sc-Claude's view")).toEqual({ zoom: 1.1 });
  expect(pick({ agents_subagent_click: "transcript" }, "st-sc-Transcript")).toEqual({ agents_subagent_click: "transcript" });
  const view = new Settings();
  view.person = {};
  const drawn = texts(view.page("activity", new Proxy({}, { get: () => "#000000" }), {}));
  const at = drawn.indexOf("Sub-agents");
  expect(at).toBeGreaterThanOrEqual(0);
  expect(drawn.indexOf("Sub-agent click")).toBeGreaterThan(at);
  // Only the Sub-agents row's own hint and choices lie between the two titles.
  expect(drawn.slice(at + 2, drawn.indexOf("Sub-agent click"))).toEqual(["Tree", "Folded", "Off"]);
  const after = drawn.indexOf("Sub-agent click");
  expect(drawn.slice(after + 2, after + 4)).toEqual(["Claude's view", "Transcript"]);
});

// A layout like agent-deck: the tab block in a row beside a panel.
const deck = () => ({
  id: "root", type: "row", terminal_padding: [14, 12],
  children: [
    { id: "side", type: "column", children: [] },
    { id: "work-body", type: "row", padding: 4, gap: 8, children: [
      { id: "main", type: "block", content: { tab: "active", pane: { radius: 10, gap: 6, lone: true } } },
      { id: "panel", type: "column", children: [] },
    ] },
  ],
});

test("spacing reads the tab block's pane gap, its container's padding and gap, and the root's terminal padding", () => {
  expect(spacingOfTree(deck())).toEqual({ pane_gap: 6, area_padding: [4, 4], area_gap: 8, terminal_padding: [14, 12] });
  // Unset, Herdr's defaults; no pane frame or no row or column around the tab block: nothing to set there.
  expect(spacingOfTree({ id: "root", type: "block", content: { context: "active" } })).toEqual({ pane_gap: null, area_padding: null, area_gap: null, terminal_padding: [10, 6] });
  expect(spacingOfTree({ id: "root", type: "column", padding: [6, 2], children: [{ type: "block", content: { tab: "active", pane: {} } }] })).toEqual({ pane_gap: 0, area_padding: [6, 2], area_gap: 0, terminal_padding: [10, 6] });
  expect(spacingOfTree({ id: "root", type: "row", children: [{ type: "block", content: { mod: "x" } }] })).toBeNull();
});

/** The Look page's Spacing group, `shown` being layoutSpacing() and `file` the layout's file; clicks `id`. */
async function spacingClick(shown: any, file: any, id?: string) {
  Object.assign(layoutDisk, { name: "mine", raw: file === null ? null : JSON.stringify(file), shown, writes: 0, runs: [] });
  const view = new Settings();
  await view.loadLayoutFile();
  const group = view.spacing(new Proxy({}, { get: () => "#000000" }), {});
  const button = id ? find(group, id) : undefined;
  button?.calls.on_click[0](null, clickCx());
  return { view, group, button, runs: layoutDisk.runs };
}

test("spacing steppers run layout.spacing, never write the layout file, and stay within 0 to 40", async () => {
  expect((await spacingClick(deck(), deck(), "st-sp-pane_gap-plus")).runs).toEqual([["layout.spacing", { pane_gap: 7 }]]);
  expect((await spacingClick(deck(), deck(), "st-sp-area_padding-minus")).runs).toEqual([["layout.spacing", { area_padding: 3 }]]);
  expect((await spacingClick(deck(), deck(), "st-sp-area_gap-plus")).runs).toEqual([["layout.spacing", { area_gap: 9 }]]);
  expect((await spacingClick(deck(), deck(), "st-sp-terminal_padding-y-minus")).runs).toEqual([["layout.spacing", { terminal_padding: [14, 11] }]]);
  const uneven = deck();
  uneven.children[1].padding = [4, 6] as any;
  expect((await spacingClick(uneven, uneven, "st-sp-area_padding-plus")).runs).toEqual([["layout.spacing", { area_padding: [5, 7] }]]);
  const edge = deck();
  edge.children[1].gap = 40;
  edge.children[1].children[0].content.pane!.gap = 0;
  expect((await spacingClick(edge, edge, "st-sp-area_gap-plus")).runs).toEqual([["layout.spacing", { area_gap: 40 }]]);
  expect((await spacingClick(edge, edge, "st-sp-pane_gap-minus")).runs).toEqual([["layout.spacing", { pane_gap: 0 }]]);
  expect(layoutDisk.writes).toBe(0);
});

test("reset shows only where a value differs from the layout file, and clears that click", async () => {
  const clicked = deck();
  clicked.children[1].gap = 12;
  clicked.terminal_padding = [8, 4];
  const { group, runs } = await spacingClick(clicked, deck(), "st-sp-area_gap-reset");
  expect(runs).toEqual([["layout.spacing", { reset: ["area_gap"] }]]);
  expect(find(group, "st-sp-terminal_padding-reset")).toBeDefined();
  expect(find(group, "st-sp-pane_gap-reset")).toBeUndefined();
  expect(find(group, "st-sp-area_padding-reset")).toBeUndefined();
});

test("a layout file that can't be read or parsed hides the steppers and says why", async () => {
  for (const raw of ['{"type": "row",}', "[]", ""]) {
    Object.assign(layoutDisk, { name: "mine", raw, shown: deck(), runs: [] });
    const view = new Settings();
    await view.loadLayoutFile();
    const group = view.spacing(new Proxy({}, { get: () => "#000000" }), {});
    for (const id of ["st-sp-pane_gap-plus", "st-sp-terminal_padding-x-plus", "st-sp-area_gap-reset"]) expect(find(group, id)).toBeUndefined();
    expect(texts(group).some((t) => t.includes("layouts/mine.json can't be read or has errors"))).toBe(true);
  }
});

test("a layout with nothing to space hides those controls and says why", async () => {
  const alone = { id: "root", type: "block", content: { tab: "active" } };
  const { group } = await spacingClick(alone, alone);
  for (const id of ["st-sp-pane_gap-plus", "st-sp-area_padding-plus", "st-sp-area_gap-plus"]) expect(find(group, id)).toBeUndefined();
  expect(find(group, "st-sp-terminal_padding-x-plus")).toBeDefined();
  const none = await spacingClick({ id: "root", type: "row", children: [] }, null);
  expect(find(none.group, "st-sp-terminal_padding-x-plus")).toBeUndefined();
});

/** Every string drawn under `root`. */
function texts(root: any): string[] {
  return (root?.nodes ?? []).flatMap((n: any) => (typeof n === "string" ? [n] : texts(n)));
}

test("spacing follows the tab block on screen: not a hidden one, not a stack's other child", () => {
  const tree = {
    id: "root", type: "row", children: [
      { id: "h", type: "block", hidden: true, content: { tab: "active", pane: { gap: 1 } } },
      { id: "st", type: "stack", active: "b", children: [
        { id: "a", type: "column", gap: 2, children: [{ id: "ta", type: "block", content: { tab: "active", pane: { gap: 3 } } }] },
        { id: "b", type: "row", gap: 4, children: [{ id: "tb", type: "block", content: { tab: "active", pane: { gap: 5 } } }] },
      ] },
    ],
  };
  expect(spacingOfTree(tree)).toMatchObject({ pane_gap: 5, area_gap: 4 });
  (tree.children[1] as any).active = undefined;
  expect(spacingOfTree(tree)).toMatchObject({ pane_gap: 3, area_gap: 2 });
});

test("spacing shows f32 values rounded, and a value equal to the file's offers no reset", async () => {
  const shown = deck();
  shown.children[1].children[0].content.pane!.gap = 2.299999952316284;
  const file = deck();
  file.children[1].children[0].content.pane!.gap = 2.3;
  const { group } = await spacingClick(shown, file);
  expect(texts(group)).toContain("2.3");
  expect(texts(group).some((t) => t.includes("2.2999"))).toBe(false);
  expect(find(group, "st-sp-pane_gap-reset")).toBeUndefined();
});

test("a click equal to Herdr's default still offers a reset when the file leaves the field out", async () => {
  const file = deck();
  delete (file as any).terminal_padding;
  const shown = deck();
  shown.terminal_padding = [10, 6];
  const { group } = await spacingClick(shown, file);
  expect(find(group, "st-sp-terminal_padding-reset")).toBeDefined();
});

test("quick clicks each step once, before layoutSpacing() catches up", async () => {
  const { view } = await spacingClick(deck(), deck());
  const t = new Proxy({}, { get: () => "#000000" });
  for (let i = 0; i < 2; i++) find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  expect(layoutDisk.runs).toEqual([["layout.spacing", { pane_gap: 7 }], ["layout.spacing", { pane_gap: 8 }]]);
  // Once layoutSpacing() shows it, the pending step is gone and layoutSpacing() leads again.
  layoutDisk.shown = deck();
  layoutDisk.shown.children[1].children[0].content.pane.gap = 8;
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  expect(layoutDisk.runs.at(-1)).toEqual(["layout.spacing", { pane_gap: 9 }]);
});

test("reset drops a pending step, so the stepper shows layoutSpacing() again", async () => {
  const clicked = deck();
  clicked.children[1].children[0].content.pane!.gap = 7;
  const { view } = await spacingClick(clicked, deck());
  const t = new Proxy({}, { get: () => "#000000" });
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  find(view.spacing(t, {}), "st-sp-pane_gap-reset").calls.on_click[0](null, clickCx());
  expect(view.spacingPending).toEqual({});
  // layoutSpacing() now has the file's 6 again: the next step counts from it.
  layoutDisk.shown = deck();
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  expect(layoutDisk.runs).toEqual([["layout.spacing", { pane_gap: 8 }], ["layout.spacing", { reset: ["pane_gap"] }], ["layout.spacing", { pane_gap: 7 }]]);
});

test("a step layoutSpacing() never shows (a refused call) expires, and steps don't carry to another layout", async () => {
  const { view } = await spacingClick(deck(), deck());
  const t = new Proxy({}, { get: () => "#000000" });
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  expect(texts(view.spacing(t, {}))).toContain("7");
  // What the one-shot settle timer (1050 ms after the newest step) does: nothing to drop yet, then the step expires.
  expect(settlePending(view.spacingPending, spacingOfTree(deck()), Date.now())).toBe(false);
  expect(settlePending(view.spacingPending, spacingOfTree(deck()), Date.now() + 1001)).toBe(true);
  expect(view.spacingPending).toEqual({});
  expect(texts(view.spacing(t, {}))).toContain("6");
  // A step on one layout is gone once another shows.
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  layoutDisk.name = "other";
  view.layoutFile = { name: "other", tree: deck() };
  view.spacing(t, {});
  expect(view.spacingPending).toEqual({});
});

test("spacing treats a null field as left out, and shows padding as x, y like terminal padding", async () => {
  expect(sameSpacing(null, undefined)).toBe(true);
  expect(sameSpacing(null, 0)).toBe(false);
  const file = deck();
  (file.children[1] as any).gap = null;
  const shown = deck();
  delete (shown.children[1] as any).gap;
  shown.children[1].padding = [4, 6] as any;
  file.children[1].padding = [4, 6] as any;
  const { group } = await spacingClick(shown, file);
  expect(find(group, "st-sp-area_gap-reset")).toBeUndefined();
  expect(texts(group)).toContain("6, 4");
});

test("a missing layout file keeps the steppers (Herdr takes layout.spacing then), without a Reset", async () => {
  const clicked = deck();
  clicked.children[1].gap = 12;
  const { group, runs } = await spacingClick(clicked, null, "st-sp-pane_gap-plus");
  expect(runs).toEqual([["layout.spacing", { pane_gap: 7 }]]);
  expect(find(group, "st-sp-area_gap-reset")).toBeUndefined();
});

test("the Settings window runs no spacing timer until a step is pending, and stops it once none is", async () => {
  timers.length = 0;
  const view = new Settings();
  view.init({}, { ...clickCx(), focus_handle: () => ({ focus() {} }), spawn() {} });
  // The window's own timers (update check, file and machine refresh) and nothing faster.
  expect(timers.map((t) => [t.every, t.ms])).toEqual([[true, 300], [true, 3000]]);
  timers.length = 0;
  Object.assign(layoutDisk, { name: "mine", raw: JSON.stringify(deck()), shown: deck(), runs: [] });
  await view.loadLayoutFile();
  const t = new Proxy({}, { get: () => "#000000" });
  view.spacing(t, {});
  expect(timers).toEqual([]);
  // A step starts one one-shot timer; another replaces it.
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  expect(live().map((x) => [x.every, x.ms])).toEqual([[false, 1050]]);
  // Reset leaves nothing pending: no timer.
  layoutDisk.shown.children[1].children[0].content.pane.gap = 7;
  find(view.spacing(t, {}), "st-sp-pane_gap-reset").calls.on_click[0](null, clickCx());
  expect(live()).toEqual([]);
  // A step layoutSpacing() never shows: when the timer fires after it expired, it goes and nothing re-arms.
  find(view.spacing(t, {}), "st-sp-pane_gap-plus").calls.on_click[0](null, clickCx());
  const [pending] = live();
  view.spacingPending.pane_gap.at -= 2000;
  let notified = 0;
  pending.f({ ...clickCx(), notify() { notified++; } });
  expect(view.spacingPending).toEqual({});
  expect(notified).toBe(1);
  expect(live().filter((x) => x !== pending)).toEqual([]);
});

test("the mod's spacing fields have the app's shape, and the page never reads the whole layout", async () => {
  expect(spacingFieldsOf(deck())).toEqual({ pane: true, area: true, pane_gap: 6, area_padding: 4, area_gap: 8, terminal_padding: [14, 12] });
  expect(spacingFieldsOf({ id: "root", type: "block", content: { tab: "active", pane: { gap: null } } })).toEqual({ pane: true, area: false });
  expect(spacingFieldsOf({ id: "root", type: "row", children: [] })).toBeNull();
  const { view } = await spacingClick(deck(), deck(), "st-sp-pane_gap-plus");
  view.spacing(new Proxy({}, { get: () => "#000000" }), {});
  expect(layoutDisk.layoutReads).toBe(0);
});

test("re-reading an unchanged layout file asks for no redraw", async () => {
  Object.assign(layoutDisk, { name: "mine", raw: JSON.stringify(deck()), readFail: undefined });
  const view = new Settings();
  expect(await view.loadLayoutFile()).toBe(true);
  expect(await view.loadLayoutFile()).toBe(false);
  layoutDisk.raw = JSON.stringify({ ...deck(), terminal_padding: [1, 1] });
  expect(await view.loadLayoutFile()).toBe(true);
  expect(view.layoutFile.tree.terminal_padding).toEqual([1, 1]);
  // The same failure twice is no change either.
  layoutDisk.readFail = "permission denied";
  expect(await view.loadLayoutFile()).toBe(true);
  expect(await view.loadLayoutFile()).toBe(false);
  expect(view.layoutFile.tree).toBeNull();
  layoutDisk.readFail = undefined;
});

// Search ----------------------------------------------------------------------

/** Every text an element tree draws, in order. */
function texts(root: any): string[] {
  if (typeof root === "string") return [root];
  return (root?.nodes ?? []).flatMap(texts);
}
const t = new Proxy({}, { get: () => "#000000" });
const searchCx = { spawn() {}, notify() {}, stop_propagation() {} };

/** A Settings window searching for `query`: what its results draw, and the view. */
function search(query: string, person: Record<string, any> = {}) {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] } };
  const view = new Settings();
  view.init({}, { ...searchCx, focus_handle: () => ({ focus() {} }), timer: { every() {}, after() {} } });
  view.person = person;
  for (const part of ["layoutPicker", "spacing", "fontButton", "fontList", "iconPicker"]) view[part] = element;
  view.setQuery(query);
  return { view, root: view.results(t, searchCx) };
}

test("search words are lower case, with settings.json's _ - . as spaces", () => {
  expect(searchWords("  Cell_Rounding  zoom-step ")).toEqual(["cell", "rounding", "zoom", "step"]);
  expect(searchWords("   ")).toEqual([]);
});

test("a word matches as a prefix or substring anywhere, and with one typo only in titles and keys", () => {
  expect(wordSpan("round", "Cell rounding", true)).toEqual([5, 10]);
  expect(wordSpan("ound", "Cell rounding", true)).toEqual([6, 10]);
  expect(wordSpan("roundng", "Cell rounding", true)).toEqual([5, 13]);
  expect(wordSpan("ligatrues", "Ligatures", true)).toEqual([0, 9]);
  expect(wordSpan("zooom", "Zoom steps", true)).toEqual([0, 4]);
  // Short words and descriptions only match as typed.
  expect(wordSpan("zom", "Zoom steps", true)).toBeNull();
  expect(wordSpan("zoom", "Zom", true)).toEqual([0, 3]);
  // A typo is forgiven against a whole word that starts the same.
  expect(wordSpan("sound", "Cell rounding", true)).toBeNull();
  expect(wordSpan("sound", "Round", true)).toBeNull();
  expect(wordSpan("roun", "Cell rounding", true)).toEqual([5, 9]);
  expect(wordSpan("roundng", "Cell rounding", false)).toBeNull();
  expect(wordSpan("rounding", "cell_rounding", true)).toEqual([5, 13]);
});

test("every word must match the title, a key, the description or the heading", () => {
  const entry = { title: "Cell rounding", about: "Nearest keeps rows within half a pixel.", keys: ["cell_rounding"], heading: "Terminal text" };
  expect(matchSetting(searchWords("cell rounding"), entry)).toEqual({ title: [[0, 4], [5, 13]], about: [], keys: [[[0, 4], [5, 13]]] });
  expect(matchSetting(searchWords("pixel"), entry)?.about).toEqual([[33, 38]]);
  expect(matchSetting(searchWords("terminal"), entry)).not.toBeNull();
  expect(matchSetting(searchWords("cell zoom"), entry)).toBeNull();
  expect(matchSetting([], entry)).toBeNull();
});

test("marked text is cut into runs, overlaps merged", () => {
  expect(markRuns("Cell rounding", [[5, 9], [7, 13]])).toEqual([["Cell ", false], ["rounding", true]]);
  expect(markRuns("abc", [])).toEqual([["abc", false]]);
});

test("search shows only matching settings, under their page and heading", () => {
  const { root } = search("rounding");
  const shown = texts(root).join("|");
  expect(find(root, "st-found-grid")).toBeDefined();
  expect(find(root, "st-found-window")).toBeUndefined();
  expect(shown).toContain("Text › Terminal grid");
  expect(shown).toContain("rounding");
  expect(shown).toContain("TERMINAL GRID");
  expect(shown).not.toContain("Ligatures");
  expect(shown).not.toContain("SIZE");
  // The row's settings.json key shows, marked where it matched.
  expect(shown).toContain("cell_|rounding");
});

test("search finds a setting by its settings.json key, and forgives a typo", () => {
  for (const q of ["cell_rounding", "roundng"]) {
    const shown = texts(search(q).root).join("|");
    expect(shown).toContain("rounding");
    expect(shown).not.toContain("Cell width");
  }
  const icon = search("icon").root;
  expect(find(icon, "st-found-window")).toBeDefined();
  expect(texts(icon).join("|")).not.toContain("Light or dark");
  // Notes between settings aren't results.
  expect(texts(icon).join("|")).not.toContain("For the layout you're in");
});

test("a search with no match says so", () => {
  const { root } = search("qqqzzz");
  expect(find(root, "st-none")).toBeDefined();
  expect(texts(root).join("|")).toContain("No settings match");
});

test("the index is built once per window and again only after a setting changes", () => {
  const { view } = search("zoom");
  let builds = 0;
  const build = view.buildIndex.bind(view);
  view.buildIndex = (...a: any[]) => { builds++; return build(...a); };
  for (const q of ["zoo", "zoom st", "font", "rounding"]) {
    view.setQuery(q);
    view.results(t, searchCx);
  }
  expect(builds).toBe(0);
  // Drawing again with the same words matches nothing again.
  const matched = view.matched;
  view.results(t, searchCx);
  expect(view.matched).toBe(matched);
  view.person = { cell_rounding: "nearest" }; // settings.json written
  view.results(t, searchCx);
  expect(builds).toBe(1);
});

test("with nothing typed, pages draw as before and nothing is indexed or matched", () => {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] } };
  const view = new Settings();
  view.init({}, { ...searchCx, focus_handle: () => ({ focus() {} }), timer: { every() {}, after() {} } });
  view.person = {};
  for (const part of ["fontButton", "fontList"]) view[part] = element;
  view.match = () => { throw new Error("matched with nothing typed"); };
  const shown = ["fonts", "grid"].map((p) => texts(view.page(p, t, searchCx)).join("|")).join("|");
  expect(shown).toContain("Ligatures");
  expect(shown).toContain("Cell rounding");
  expect(view.index).toBeNull();
});

test("cmd-f focuses the field, typing with nothing focused searches, esc clears then leaves", () => {
  const { view } = search("");
  let closed = 0;
  view.close = () => closed++;
  const key = (key: string, key_char?: string, modifiers: Record<string, boolean> = {}) => view.onKey({ key, key_char, modifiers }, searchCx);
  key("f", "f", { control: true }); // the harness's window is Linux: ctrl-f
  expect(view.searchInput.focused).toBe(true);
  expect(view.query).toBe("");
  view.searchInput.focused = false;
  key("z", "z");
  expect(view.query).toBe("z");
  expect(view.searchInput.text).toBe("z");
  expect(view.searchInput.focused).toBe(true);
  key("o", "o"); // the field isn't focused in this harness: the window adds it
  expect(view.query).toBe("zo");
  // Shortcuts and a lone space don't start a search.
  view.setQuery("");
  key("s", "s", { control: true });
  key("space", " ");
  expect(view.query).toBe("");
  // Esc clears the words, then leaves the field, and only then closes the window.
  view.setQuery("zoom");
  view.searchFocused = true;
  key("escape");
  expect(view.query).toBe("");
  expect(closed).toBe(0);
  key("escape");
  expect(closed).toBe(0);
  view.searchFocused = false;
  key("escape");
  expect(closed).toBe(1);
});

/** A search on a window opened at `section`, its spawned work run when `settle` is awaited. */
function searchFrom(section: string) {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0, ui_scale_ranges: { ui_zoom: [0.75, 1.5], ui_text_scale: [0.8, 1.3] } };
  const pending: Promise<void>[] = [];
  const cx = { ...searchCx, spawn: (f: (cx: any) => Promise<void>) => pending.push(f(cx)), focus_handle: () => ({ focus() {} }), timer: { every() {}, after() {} } };
  const view = new Settings();
  view.init({}, cx);
  view.section = section;
  view.person = {};
  for (const part of ["layoutPicker", "fontButton", "fontList", "iconPicker"]) view[part] = element;
  const shown = (query: string) => { view.setQuery(query); return texts(view.results(t, cx)).join("|"); };
  const settle = async () => { while (pending.length) await pending.shift(); };
  return { view, shown, settle };
}

test("search finds machines and spacing that a page not yet opened reads in", async () => {
  machineDisk = [{ name: "buildbox", target: "me@buildbox", state: "connected", version: "0.1.17" }];
  Object.assign(layoutDisk, { name: "mine", raw: JSON.stringify(deck()), shown: deck(), readFail: undefined });
  const { shown, settle } = searchFrom("updates");
  shown("buildbox");
  shown("pane gap");
  await settle();
  expect(shown("buildbox")).toContain("me@buildbox");
  expect(shown("pane gap")).toContain("layout.spacing.|pane|_|gap");
});

test("the spacing rows are found once the layout file is read, with nothing else changing", async () => {
  machineDisk = [];
  Object.assign(layoutDisk, { name: "mine", raw: JSON.stringify(deck()), shown: deck(), readFail: undefined });
  const { view, shown, settle } = searchFrom("updates");
  await view.loadMachines(); // the machine list is in before the search starts
  shown("pane gap");
  await settle();
  expect(shown("pane gap")).toContain("layout.spacing.|pane|_|gap");
});

test("a description that changed since indexing is matched as it is now", () => {
  const { view, shown } = searchFrom("window");
  expect(shown("keeps classic")).not.toContain("Panel colours");
  composed.panels_in_use = "classic";
  // Its page has no other match, so only a new index can show it.
  expect(shown("keeps classic")).toContain("Panel colours");
  // The same state draws from the same index.
  const index = view.index;
  shown("keeps");
  expect(view.index).toBe(index);
});

test("typing into the focused field is left to the field", () => {
  const { view } = search("zo");
  let stopped = 0;
  view.searchFocused = true;
  view.onKey({ key: "o", key_char: "o", modifiers: {} }, { ...searchCx, stop_propagation: () => stopped++ });
  expect(view.query).toBe("zo");
  expect(stopped).toBe(0);
});

test("esc leaves the field for the window, and closes a list opened in the results", () => {
  const { view } = search("font");
  let focused = 0;
  view.handle = { focus: () => focused++ };
  view.picking = "font";
  view.onKey({ key: "escape", modifiers: {} }, searchCx);
  expect(view.query).toBe("");
  expect(view.picking).toBeNull();
  expect(focused).toBe(1);
  view.searchFocused = true;
  view.onKey({ key: "escape", modifiers: {} }, searchCx);
  expect(focused).toBe(2);
});

test("alt starts no search off macOS, and marks keep their place after a letter whose lower case is longer", () => {
  const { view } = search("");
  view.onKey({ key: "x", key_char: "x", modifiers: { alt: true } }, searchCx);
  expect(view.query).toBe("");
  expect(wordSpan("box", "İbox", false)).toEqual([1, 4]);
});

// Groups, pages and the Themes page ------------------------------------------

/** A Settings window opened at `section` (as app.settings names it), on a dark theme. */
function opened(section?: string, palette: Record<string, string> = dark) {
  composed = { zoom: 1, font_size: 14, font_weight: 500, line_height: 1.15, letter_spacing: 0 };
  propsNow = { section };
  const view = new Settings();
  view.init({}, { ...searchCx, focus_handle: () => ({ focus() {} }), timer: { every() {}, after() {} } });
  propsNow = {};
  view.person = {};
  return { view, t: palette, nav: () => view.sidebar(palette, false) };
}
const dark: Record<string, string> = { popover: "#2a2f3a", background: "#1f2430", foreground: "#cccac2", accent: "#6dcbfa", border: "#393d46", muted: "#aeb1b7", dim: "#9ca0a6" };
const light: Record<string, string> = { popover: "#fafafa", background: "#ffffff", foreground: "#24292f", accent: "#f2a33a", border: "#e1e4e8", muted: "#57606a", dim: "#6e7781" };
const card = (name: string, isLight = false) => ({ name, light: isLight, colors: { background: isLight ? "#ffffff" : "#000000" } });

test("app.settings' section opens a page, its older names still work, and anything else opens Themes", () => {
  expect(pageFor("look")).toBe("themes");
  expect(pageFor("text")).toBe("zoom");
  expect(pageFor("updates")).toBe("updates");
  expect(pageFor("rendering")).toBe("rendering");
  expect(pageFor("nope")).toBe("themes");
  expect(pageFor(undefined)).toBe("themes");
  // Keyboard is macOS only: elsewhere it isn't a page.
  expect(pageFor("keyboard")).toBe("themes");
  expect(opened("text").view.section).toBe("zoom");
});

test("the sidebar opens the groups left open and the shown page's, and remembers each open and close", () => {
  for (const k of Object.keys(sharedDisk)) delete sharedDisk[k];
  sharedDisk["settings.groups.open"] = ["system"];
  const { view, nav } = opened("look");
  expect(find(nav(), "st-s-themes")).toBeDefined();
  expect(find(nav(), "st-s-updates")).toBeDefined();
  expect(find(nav(), "st-s-zoom")).toBeUndefined();
  find(nav(), "st-g-text").calls.on_click[0](null, searchCx);
  expect(sharedDisk["settings.groups.open"]).toEqual(["appearance", "text", "system"]);
  expect(find(nav(), "st-s-zoom")).toBeDefined();
  find(nav(), "st-g-appearance").calls.on_click[0](null, searchCx);
  expect(sharedDisk["settings.groups.open"]).toEqual(["text", "system"]);
  expect(find(nav(), "st-s-themes")).toBeUndefined();
  // Going to a page opens its group.
  view.go("alerts", searchCx);
  expect(sharedDisk["settings.groups.open"]).toEqual(["text", "agents", "system"]);
  expect(opened("updates").view.open).toEqual(new Set(["text", "agents", "system"]));
});

test("a search opens every group with a match and counts each page's, leaving what's remembered", () => {
  for (const k of Object.keys(sharedDisk)) delete sharedDisk[k];
  const { view, t, nav } = opened("look");
  view.setQuery("rounding");
  view.results(t, searchCx);
  const text = find(nav(), "st-g-text");
  expect(texts(text)).toContain("1");
  expect(texts(find(nav(), "st-s-grid"))).toEqual(["Terminal grid", "1"]);
  expect(texts(find(nav(), "st-s-zoom"))).toEqual(["Zoom"]);
  expect(find(nav(), "st-s-machines")).toBeUndefined();
  // Two matches on one page: a page's count is its matches.
  view.setQuery("focus border");
  view.results(t, searchCx);
  expect(texts(find(nav(), "st-s-panes"))).toEqual(["Panes", "2"]);
  expect(sharedDisk["settings.groups.open"]).toBeUndefined();
  // A click on a group a search opened leaves what's kept alone.
  find(nav(), "st-g-text").calls.on_click[0](null, searchCx);
  expect(sharedDisk["settings.groups.open"]).toBeUndefined();
  view.setQuery("");
  expect(find(nav(), "st-s-grid")).toBeUndefined();
});

test("arrows walk the pages in order, opening their group, and close and open the shown page's group", () => {
  for (const k of Object.keys(sharedDisk)) delete sharedDisk[k];
  const { view } = opened("panes");
  const key = (k: string, modifiers = {}) => view.onKey({ key: k, modifiers }, searchCx);
  key("down");
  expect(view.section).toBe("zoom");
  expect(view.open.has("text")).toBe(true);
  key("up");
  key("up");
  expect(view.section).toBe("window");
  key("left");
  expect(view.open.has("appearance")).toBe(false);
  key("right");
  expect(view.open.has("appearance")).toBe(true);
  // The ends stay put, a modifier leaves the key alone, and so does a field with the keyboard.
  view.go("themes", searchCx);
  key("up");
  expect(view.section).toBe("themes");
  key("down", { shift: true });
  expect(view.section).toBe("themes");
  view.themeInputFocused = true;
  key("down");
  expect(view.section).toBe("themes");
});

test("the Themes page lists the shown slot's themes, by source, filtered by chip and name", () => {
  themeList = [card("ayu-mirage"), card("rex-basalt-shore"), card("rex-cinder-peak"), card("dracula"), card("ayu-light", true), card("rex-alpine-milk", true), card("win98", true)];
  const { view, t } = opened("look");
  const rows = () => view.themeRows(view.themeGroups(themeList)).map((r: any) => r.head ?? r.cards.map((x: any) => x.name).join(","));
  expect(rows()).toEqual(["standard", "ayu-mirage,dracula", "rex", "rex-basalt-shore,rex-cinder-peak"]);
  const page = view.themesPage(t);
  expect(texts(find(page, "st-tc-all"))).toEqual(["All 4"]);
  expect(texts(find(page, "st-tc-rex"))).toEqual(["Rex 2"]);
  find(page, "st-tc-rex").calls.on_click[0](null, searchCx);
  expect(rows()).toEqual(["rex-basalt-shore,rex-cinder-peak"]);
  find(page, "st-tt-light").calls.on_click[0](null, searchCx);
  expect(rows()).toEqual(["rex-alpine-milk"]);
  view.themeSource = null;
  expect(rows()).toEqual(["standard", "ayu-light", "rex", "rex-alpine-milk"]);
  view.themeSlot = "dark";
  view.themeFilter = "Cinder";
  expect(rows()).toEqual(["rex-cinder-peak"]);
  view.themeFilter = "nothing";
  expect(find(view.themesPage(t), "st-themes-none")).toBeDefined();
  // A closed source keeps its heading only.
  view.themeFilter = "";
  view.themesClosed.add("standard");
  expect(rows()).toEqual(["standard", "rex", "rex-basalt-shore,rex-cinder-peak"]);
  // A theme named like a layout belongs to that layout.
  view.themeSlot = "light";
  expect(rows()).not.toContain("win98");
});

test("the theme grid scrolls to the picked theme when it opens and when what it lists changes, not on every draw", () => {
  themeList = Array.from({ length: 23 }, (_, i) => card(`theme-${String(i).padStart(2, "0")}`)).concat([card("rex-a"), card("rex-b")]);
  composed = { theme_dark: "theme-17" };
  const { view, t } = opened("themes");
  composed = { theme_dark: "theme-17" };
  view.themesPage(t);
  // Row 0 is Standard's heading; theme-17 is in its fourth row of five.
  expect(view.themeScroll.scrolls).toEqual([[4, "center"]]);
  view.themesPage(t);
  expect(view.themeScroll.scrolls.length).toBe(1);
  view.themeSource = "standard";
  view.themesPage(t);
  expect(view.themeScroll.scrolls.at(-1)).toEqual([3, "center"]);
  // The page builds no card: the list draws the rows on screen, from their heights.
  const args = (function walk(e: any): any { if (e?.calls?.v_virtual_list) return e.calls.v_virtual_list; for (const n of e?.nodes ?? []) { const a = walk(n); if (a) return a; } })(view.themesPage(t));
  expect(find(view.themesPage(t), "st-t-theme-17")).toBeUndefined();
  expect(args[1]).toBe(5);
  expect(args[2]).toEqual([84, 84, 84, 84, 84]);
  expect(args[3](2)).toBe("row:theme-10");
  expect(args[3](0)).toBe("row:theme-00");
  const drawn = args[4]({ start: 3, end: 4 }, searchCx);
  expect(drawn.length).toBe(1);
  expect(find(drawn[0], "st-t-theme-17")).toBeDefined();
  // Coming back to the page shows it again.
  // A listing without the picked theme starts at the top.
  view.themeFilter = "theme-0";
  view.themesPage(t);
  expect(view.themeScroll.scrolls.at(-1)).toEqual([0, "top"]);
  view.themeFilter = "";
  view.themesPage(t);
  const scrolls = view.themeScroll.scrolls.length;
  view.go("zoom", searchCx);
  view.go("themes", searchCx);
  view.themesPage(t);
  expect(view.themeScroll.scrolls.length).toBe(scrolls + 1);
  expect(view.themeScroll.scrolls.at(-1)).toEqual([3, "center"]);
});

test("the selected page, tab, chip and card are a tonal pill; hover is a neutral one", () => {
  themeList = [card("ayu-mirage"), card("dracula")];
  composed = { theme_dark: "ayu-mirage" };
  const { view, t, nav } = opened("themes");
  composed = { theme_dark: "ayu-mirage" };
  const sel = tonal(t, true);
  const hover = (el: any) => { const st = element(); el.calls.hover[0](st); return st.calls.bg?.[0]; };
  const row = find(nav(), "st-s-themes");
  expect(row.calls.bg).toEqual([sel.fill]);
  expect(row.calls.rounded).toEqual([8]);
  expect(row.calls.hover).toBeUndefined();
  const label = row.nodes[0];
  expect(label.calls.text_color).toEqual([sel.text]);
  expect(label.calls.font_semibold).toBeDefined();
  const other = find(nav(), "st-s-layout");
  expect(other.calls.bg).toBeUndefined();
  expect(hover(other)).toBe(over(t.foreground, t.popover, 0.04));
  expect(hover(other)).not.toBe(sel.fill);
  const page = view.themesPage(t);
  expect(find(page, "st-tt-dark").calls.bg).toEqual([sel.fill]);
  expect(find(page, "st-tt-dark").calls.border_color).toEqual([sel.ring]);
  expect(find(page, "st-tt-light").calls.bg).toEqual([t.background]);
  expect(find(page, "st-tc-all").calls.bg).toEqual([sel.fill]);
  expect(find(page, "st-tc-standard").calls.bg).toBeUndefined();
  const picked = view.themeCard(t, themeList[0], "ayu-mirage", sel);
  expect(picked.calls.bg).toEqual([sel.fill]);
  expect(picked.calls.border_color).toEqual([sel.ring]);
  expect(texts(picked)).toContain("ayu mirage");
  const plain = view.themeCard(t, themeList[1], "ayu-mirage", sel);
  expect(plain.calls.border_color).toEqual(["#00000000"]);
  expect(hover(plain)).not.toBe(sel.fill);
});

test("selected text reads at 4.5:1 on its tonal fill on light and dark themes, and an inactive window draws it grey", () => {
  const dim = { ...dark, accent: "#2a3f8f" };
  for (const palette of [dark, light, dim, { ...light, accent: "#ffd400" }]) {
    const { fill, text } = tonal(palette, true);
    expect(fill).toBe(over(palette.accent, palette.popover, 0.16));
    expect(contrast(text, fill)).toBeGreaterThanOrEqual(4.5);
  }
  // An accent that already reads keeps its color.
  expect(tonal(dark, true).text).toBe("#6dcbfa");
  // Without an accent, the ring.
  const ringed = { ...dark, accent: undefined as any, ring: "#ff8800" };
  expect(tonal(ringed, true).fill).toBe(over("#ff8800", dark.popover, 0.16));
  const away = tonal(dark, false);
  expect(away.text).toBe(dark.foreground);
  expect(away.fill).toBe(over(dark.foreground, dark.popover, 0.1));
  expect(contrast("#000000", "#ffffff")).toBeCloseTo(21, 5);
  // Drawn so while the window isn't the active one: page, tab, chip and card.
  themeList = [card("ayu-mirage")];
  windowNow = { platform: "linux", active: false };
  try {
    composed = { theme_dark: "ayu-mirage" };
    const { view, t, nav } = opened("themes");
    composed = { theme_dark: "ayu-mirage" };
    const page = view.themesPage(t);
    const grid = (function walk(e: any): any { if (e?.calls?.v_virtual_list) return e.calls.v_virtual_list; for (const n of e?.nodes ?? []) { const a = walk(n); if (a) return a; } })(page);
    const picked = find(grid[4]({ start: 0, end: 1 }, searchCx)[0], "st-t-ayu-mirage");
    for (const el of [find(nav(), "st-s-themes"), find(page, "st-tt-dark"), find(page, "st-tc-all"), picked]) expect(el.calls.bg).toEqual([tonal(t, false).fill]);
  } finally {
    windowNow = { platform: "linux" };
  }
});

test("keys typed into the theme filter stay there, and esc leaves the filter, not the window", () => {
  const { view } = opened("themes");
  let focused = 0, closed = 0;
  view.handle = { focus: () => focused++ };
  view.close = () => closed++;
  view.themeInputFocused = true;
  view.onKey({ key: "a", key_char: "a", modifiers: {} }, searchCx);
  view.onKey({ key: "down", modifiers: {} }, searchCx);
  expect([view.query, view.section]).toEqual(["", "themes"]);
  view.onKey({ key: "escape", modifiers: {} }, searchCx);
  expect([focused, closed]).toEqual([1, 0]);
});

test("a long theme name is cut inside its card, with the whole name on hover and the ✓ kept", () => {
  const { view, t } = opened("themes");
  const sel = tonal(t, true);
  const picked = view.themeCard(t, card("catppuccin-macchiato"), "catppuccin-macchiato", sel);
  const row = picked.nodes[1];
  expect([row.calls.w_full, row.calls.min_w]).toEqual([[], [0]]);
  const name = row.nodes[0];
  expect(name.calls.truncate).toBeDefined();
  expect(name.calls.tooltip).toEqual(["catppuccin macchiato"]);
  expect(row.nodes[1].calls.flex_none).toBeDefined();
  // A Rex theme drops its prefix.
  expect(texts(view.themeCard(t, card("rex-basalt-shore"), "x", sel))).toContain("basalt shore");
});

test("a fork build shows its commit beside the version", () => {
  expect(forkTag("fork.0990622")).toBe(" (fork 0990622)");
  expect(forkTag("fork.0990622.dirty")).toBe(" (fork 0990622.dirty)");
  expect(forkTag(null)).toBe("");
  expect(forkTag(undefined)).toBe("");
});

test("header info: Use suggested writes the template, a blank field leaves it out, the preview follows the field", () => {
  const suggested = "{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed} · {subagents}";
  expect(click({ zoom: 1.1 }, {}, "st-hi-suggested", [], "activity")).toEqual({ zoom: 1.1, pane_header_info: suggested });
  expect(click({ pane_header_info: suggested }, {}, "st-hi-suggested", ["shown?"], "activity")).toBeUndefined();
  const field = (person: Record<string, any>) => {
    composed = {};
    const view = new Settings();
    view.person = person;
    let written: Record<string, any> | undefined;
    view.change = (edit: (q: Record<string, any>) => void) => { written = structuredClone(person); edit(written); };
    const preview = find(view.page("activity", new Proxy({}, { get: () => "#000000" }), {}), "st-hi-preview");
    return { view, preview, written: () => written };
  };
  const on = field({ pane_header_info: "{state} {elapsed}" });
  expect(texts(on.preview)).toEqual(["{state} {elapsed}"]);
  on.view.setHeaderInfo("  ", clickCx());
  expect(on.written()).toEqual({});
  const off = field({});
  expect(texts(off.preview).join("")).toContain("Off");
  off.view.setHeaderInfo("", clickCx());
  expect(off.written()).toBeUndefined();
});

test("header info: a save keeps what the field holds; opening fills it from settings.json", async () => {
  Object.assign(disk, { raw: '{"pane_header_info": "{state}"}', writes: 0, fail: undefined, readFail: undefined, toasts: [] });
  const view = new Settings();
  view.person = {};
  const input = view.headerInputState();
  await view.load(true);
  expect(input.value()).toBe("{state}");
  input.set_value("{branch}");
  let done: () => void;
  const finished = new Promise<void>((resolve) => (done = resolve));
  view.setHeaderInfo("{branch}", { spawn: (f: (cx: any) => Promise<void>) => f({ notify: () => done() }), notify() {} });
  await finished;
  expect(JSON.parse(disk.raw!)).toEqual({ pane_header_info: "{branch}" });
  expect(input.value()).toBe("{branch}");
});

test("header info: the preview draws separators as the pane header does, the mark with 4 px either side", () => {
  const one = previewOf;
  previewOf = (template: string) => ({ text: template, segments: [{ sep: "", text: "approve", ask: true }, { sep: " · ", text: "main", ask: false }, { sep: "|", text: "idle", ask: false }] });
  const view = new Settings();
  view.person = { pane_header_info: "{ask} · {branch}|{state}" };
  composed = {};
  view.headerInputState();
  const row = (template: string) => {
    view.headerInput.set_value(template);
    return find(view.page("activity", new Proxy({}, { get: (_t, k) => `#${String(k)}` }), {}), "st-hi-preview");
  };
  const seps = (n: any) => (n?.nodes ?? []).filter((c: any) => c?.calls?.px);
  const shown = row("{ask} · {branch}|{state}");
  expect(texts(shown)).toEqual(["approve", "\u00b7", "main", "|", "idle"]);
  expect(seps(shown).map((c: any) => [c.calls.px, c.calls.text_color, texts(c)])).toEqual([[[4], ["#dim"], ["\u00b7"]], [[4], ["#dim"], ["|"]]]);
  previewOf = one;
});

test("card info: the field holds the default until set, Use suggested and the default leave it out, blank is kept as off", () => {
  const field = (person: Record<string, any>) => {
    composed = {};
    const view = new Settings();
    view.person = person;
    let written: Record<string, any> | undefined;
    view.change = (edit: (q: Record<string, any>) => void) => { written = structuredClone(person); edit(written); };
    const page = view.page("activity", new Proxy({}, { get: (_t, k) => `#${String(k)}` }), {});
    return { view, page, preview: find(page, "st-ci-preview"), written: () => written };
  };
  const unset = field({});
  expect(unset.view.cardInput.value()).toBe("@{remote} · {space}");
  expect(find(unset.page, "st-ci-suggested")).toBeUndefined();
  // Each segment a chip, as the agents panel draws them.
  expect(texts(unset.preview)).toEqual(["@vps", "herdr-app"]);
  expect(unset.preview.nodes.map((c: any) => [c.calls.px, c.calls.rounded, c.calls.bg])).toEqual([[[5], [4], ["#chrome"]], [[5], [4], ["#chrome"]]]);
  unset.view.setCardInfo("@{remote} · {space}", clickCx());
  expect(unset.written()).toBeUndefined();
  unset.view.setCardInfo("{machine} · {space}", clickCx());
  expect(unset.written()).toEqual({ agents_card_info: "{machine} · {space}" });
  const blank = field({ zoom: 1.1 });
  blank.view.setCardInfo("", clickCx());
  expect(blank.written()).toEqual({ zoom: 1.1, agents_card_info: "" });
  const set = field({ agents_card_info: "{space} · {branch}", zoom: 1.1 });
  expect(set.view.cardInput.value()).toBe("{space} · {branch}");
  set.view.setCardInfo("@{remote} · {space}", clickCx());
  expect(set.written()).toEqual({ zoom: 1.1 });
  expect(click({ agents_card_info: "{space}", zoom: 1.1 }, {}, "st-ci-suggested", [], "activity")).toEqual({ zoom: 1.1 });
  const off = field({ agents_card_info: "" });
  expect(off.view.cardInput.value()).toBe("");
  expect(texts(off.preview).join("")).toContain("Off");
  const none = field({ agents_card_info: " · " });
  expect(texts(none.preview)).toEqual(["Nothing to show"]);
});

test("card info: its row follows Header info; opening fills the field from settings.json", async () => {
  composed = {};
  const view = new Settings();
  view.person = {};
  const drawn = texts(view.page("activity", new Proxy({}, { get: () => "#000000" }), {}));
  expect(drawn.indexOf("Card info")).toBeGreaterThan(drawn.indexOf("Header info"));
  expect(drawn.indexOf("Card info")).toBeLessThan(drawn.indexOf("Resume agents after restart"));
  Object.assign(disk, { raw: '{"agents_card_info": ""}', writes: 0, fail: undefined, readFail: undefined, toasts: [] });
  await view.load(true);
  expect(view.cardInput.value()).toBe("");
  Object.assign(disk, { raw: "{}" });
  await view.load(true);
  expect(view.cardInput.value()).toBe("@{remote} · {space}");
});
