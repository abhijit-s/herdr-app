// App-owned settings mod. Writes changes immediately; no Save step.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, InputState, Input, Popup, v_virtual_list, VirtualListScrollHandle } from "gpui-base";
import { hc, run, theme, themes, fonts, updates, telemetry, settings, layoutSpacing, layouts, shared, setShared, appHome, readTextFile, writeTextFile, playSound, props, closeOverlay, notify, window, headerInfoPreview, cardInfoPreview, toastLooks } from "herdr";

const I = (name) => `frame/settings/${name}.svg`;

/**
 * The next `ui_zoom` / `ui_text_scale` after a stepper click: from what Herdr
 * uses (settings.json's value as Herdr clamps it, else `composed`), `delta`
 * on, within `[lo, hi]` (settings().ui_scale_ranges), in hundredths.
 */
export function scaleStep(asked, composed, delta, [lo, hi]) {
  const within = (v) => Math.max(lo, Math.min(hi, v));
  const from = typeof asked === "number" && Number.isFinite(asked) ? within(asked) : composed;
  return Math.round(within(from + delta) * 100) / 100;
}

// Spacing around the shown tab's panes (layout.spacing): Herdr's values
// when the layout leaves them out. Padding is [y, x], as layouts write it;
// terminal_padding is [x, y].
const SPACING_DEFAULTS = { pane_gap: 0, area_padding: [0, 0], area_gap: 0, terminal_padding: [10, 6] };
const SPACING_RANGE = [0, 40];

// The tab block on screen, as Herdr finds it (the first visible one: hidden
// nodes and a stack's other children don't count), and the node holding it.
function tabBlock(node, parent = null) {
  if (!node || typeof node !== "object" || node.hidden) return null;
  const c = node.content;
  if (node.type === "block" && c && typeof c === "object" && ("tab" in c || "context" in c)) return { block: node, parent };
  const children = (Array.isArray(node.children) ? node.children : []).filter((child) => child && !child.hidden);
  const shown = node.type === "stack" ? [children.find((child) => child.id === node.active) ?? children[0]].filter(Boolean) : children;
  for (const child of shown) {
    const found = tabBlock(child, node);
    if (found) return found;
  }
  return null;
}

// The spacing fields a layout tree writes, in layoutSpacing()'s shape (the
// app's `layout_sync::spacing_fields`): null when no block shows a tab;
// otherwise `pane` (a pane frame) and `area` (a row or column around the tab
// block), and each field as written, left out when unset. For the layout
// file, which the app doesn't publish.
function spacingFieldsOf(tree) {
  const found = tabBlock(tree);
  if (!found) return null;
  const pane = found.block.content.pane;
  const area = ["row", "column"].includes(found.parent?.type) ? found.parent : null;
  const out = { pane: Boolean(pane && typeof pane === "object"), area: Boolean(area) };
  const fields = { pane_gap: out.pane ? pane.gap : undefined, area_padding: area?.padding, area_gap: area?.gap, terminal_padding: tree.terminal_padding };
  for (const [key, value] of Object.entries(fields)) if (value !== undefined && value !== null) out[key] = value;
  return out;
}

// Spacing values arrive as f32 widened to f64 (2.3 as 2.299999952...).
function sameSpacing(a, b) {
  const pair = (v) => (typeof v === "number" ? [v, v] : v);
  // `"gap": null` in a file is the field left out.
  [a, b] = [a ?? undefined, b ?? undefined];
  if (a === undefined || b === undefined) return a === b;
  const [x, y] = [pair(a), pair(b)];
  return Array.isArray(x) && Array.isArray(y) && x.length === y.length && x.every((v, i) => Math.abs(v - y[i]) < 0.005);
}

// Each spacing value shown, from spacing fields, or null for one the layout
// has no place for (no pane frame, no row or column around the tab block);
// null when no block shows a tab.
function spacingOf(fields) {
  if (!fields) return null;
  const num = (v, d) => (typeof v === "number" && Number.isFinite(v) ? v : d);
  const pair = (v, d) => (typeof v === "number" ? [v, v] : Array.isArray(v) && v.length === 2 ? v.map((x) => num(x, 0)) : d);
  return {
    pane_gap: fields.pane ? num(fields.pane_gap, SPACING_DEFAULTS.pane_gap) : null,
    area_padding: fields.area ? pair(fields.area_padding, SPACING_DEFAULTS.area_padding) : null,
    area_gap: fields.area ? num(fields.area_gap, SPACING_DEFAULTS.area_gap) : null,
    terminal_padding: pair(fields.terminal_padding, SPACING_DEFAULTS.terminal_padding),
  };
}

// How long a step waits to show in layoutSpacing() before the stepper lets it go
// (a refused layout.spacing never shows); `settleLater` times it.
const SPACING_PENDING_MS = 1000;

// Drops pending steps layoutSpacing() now shows, expired ones, and all of them when
// the shown layout changed; true when any went.
function settlePending(pending, shown, now) {
  let dropped = false;
  for (const [key, { value, at }] of Object.entries(pending)) {
    if (sameSpacing(value, shown?.[key]) || now - at > SPACING_PENDING_MS) {
      delete pending[key];
      dropped = true;
    }
  }
  return dropped;
}

function stepSpacing(v, d) {
  const [lo, hi] = SPACING_RANGE;
  return Math.max(lo, Math.min(hi, Math.round(v + d)));
}

// The sidebar: groups that open and close, each with its pages. A page's id is
// what app.settings' section names.
const GROUPS = [
  { id: "appearance", title: "Appearance", icon: "palette", pages: [["themes", "Themes"], ["layout", "Layout"], ["window", "Window"], ["panes", "Panes"], ["cursor", "Cursor"]] },
  { id: "text", title: "Text", icon: "type", pages: [["zoom", "Zoom"], ["fonts", "Fonts"], ["grid", "Terminal grid"], ["rendering", "Rendering"]] },
  { id: "input", title: "Input", icon: "keyboard", pages: [...(window()?.platform === "macos" ? [["keyboard", "Keyboard"]] : []), ["mouse", "Mouse"]] },
  { id: "agents", title: "Agents", icon: "bell", pages: [["activity", "Activity"], ["alerts", "Alerts"]] },
  { id: "system", title: "System", icon: "server", pages: [["machines", "Machines"], ["updates", "Updates"], ["privacy", "Privacy"]] },
];
const PAGES = GROUPS.flatMap((g) => g.pages.map(([id, title]) => ({ id, title, group: g.id })));
// Sections named before the pages were grouped.
const ALIASES = { look: "themes", text: "zoom" };
// Where the open groups are kept (shared.json), so the sidebar opens as it was left.
const OPEN_GROUPS = "settings.groups.open";

/** The page app.settings' `section` names: a page, an older name for one, else Themes. */
export function pageFor(section) {
  const id = ALIASES[section] ?? section;
  return PAGES.some((p) => p.id === id) ? id : "themes";
}

const groupOf = (page) => PAGES.find((p) => p.id === page).group;
// A fork build reports `fork.<sha>` beside its version: shown as ` (fork <sha>)`.
const forkTag = (build) => (build ? ` (fork ${build.slice("fork.".length)})` : "");

// Themes made by `herdr-app rex-themes` are named rex-<name>; the rest are Standard.
const REX_PREFIX = "rex-";
const THEME_SOURCES = [
  ["standard", "Standard"],
  ["rex", "Rex"],
];
const themeSourceOf = (x) => (x.name.startsWith(REX_PREFIX) ? "rex" : "standard");
// Look tags, read off a theme's own colours in OKLCH (perceptual lightness,
// chroma, hue). Temperature is the background's: below NEUTRAL_CHROMA it is
// grey whatever its hue (dark greys carry a trace of tint that isn't seen);
// above, reds, oranges, yellows and browns (hue WARM_FROM° round to WARM_TO°)
// are warm and greens, teals, blues and purples cool.
const THEME_TEMPERATURES = [
  ["warm", "Warm"],
  ["neutral", "Neutral"],
  ["cool", "Cool"],
];
const NEUTRAL_CHROMA = 0.005;
const WARM_FROM = 330;
const WARM_TO = 115;
// Contrast is the OKLCH lightness gap between text and background, so a
// light theme reads like a dark one: under SOFT_GAP it's soft (greyish text
// such as #cccac2 on #1f2430, a gap of 0.58), from HIGH_GAP high. Text
// within NEAR_EXTREME of white is high whatever the background (#f7f7ee on a
// lifted #2c2f3c is still glaring); near-black text on a light theme already
// clears HIGH_GAP. The cut-offs split the author's 173 themes 63 : 77 : 33.
const THEME_CONTRASTS = [
  ["soft", "Soft"],
  ["standard", "Standard contrast"],
  ["high", "High contrast"],
];
const SOFT_GAP = 0.59;
const HIGH_GAP = 0.68;
const NEAR_EXTREME = 0.05;
// The theme grid: cards to a row and their size, so the list knows each row's height.
const THEME_COLUMNS = 5;
const THEME_CARD_W = 112;
const THEME_ROW_H = 84;
const THEME_HEAD_H = 30;
// The grid's own scroll area, under the tabs and chips, within the dialog.
const THEME_GRID_H = 330;

const LOOKS = { main: "Herdr", terminals: "Terminals", win98: "Herdr 98" };
const APPEARANCES = [
  [null, "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];
// zoom_step: what one press of the zoom keys moves.
const ZOOM_STEPS = [
  ["tenth", "10%"],
  ["point", "Whole points"],
];
// app_icon: the built-in icons, previewed from frame/settings/app-icons (scripts/app-icons.py; theme's,
// in Graphite, from src/app_icon.rs's tests).
const APP_ICONS = [
  ["classic", "Classic"],
  ["theme", "Theme"],
  ["midnight", "Midnight"],
  ["mirage", "Mirage"],
  ["glass", "Glass"],
  ["phosphor", "Phosphor"],
  ["graphite", "Graphite"],
];
// cell_rounding: how the grid snaps cells and rows to device pixels.
const CELL_ROUNDINGS = [
  ["outward", "Outward"],
  ["nearest", "Nearest"],
];
// Trial: subpixel smoothing (macOS, 1x displays; grayscale elsewhere).
const TEXT_RENDERINGS = [
  ["grayscale", "Grayscale"],
  ["subpixel", "Subpixel"],
];
// font_thicken and ui_font_thicken: macOS smoothing by text color, Ghostty's
// font-thicken, or smoothing off. Auto, the default, stays out of settings.json.
const THICKENS = [
  ["auto", "Auto"],
  [true, "On"],
  [false, "Off"],
];
const SUBPIXEL_ORDERS = [
  ["rgb", "RGB"],
  ["bgr", "BGR"],
];
// pane.arrange's modes for settings.json's default_arrange; Off leaves it out.
const ARRANGES = [
  [null, "Off"],
  ["tiled", "Tiled"],
  ["bsp", "BSP"],
  ["master", "Master"],
  ["columns", "Columns"],
  ["rows", "Rows"],
];
// What unfocused_dim fades; Window is the default, left out of settings.json.
const DIM_SCOPES = [
  ["window", "Window"],
  ["text", "Text"],
];
// How panels derive colors a theme leaves unset; Tinted is the default, left out of settings.json.
const PANEL_COLORS = [
  ["tinted", "Tinted"],
  ["classic", "Classic"],
];
// When the sidebar takes a shade darker; Flat is the default, left out of settings.json.
const SIDEBAR_SHADES = [
  ["flat", "Flat"],
  ["active", "Darker when active"],
  ["always", "Always darker"],
];
// Where toasts show (toast_position), a window's top row then its bottom row, as Rex
// names them; Bottom right is the default, left out of settings.json.
const TOAST_POSITIONS = [
  ["top-left", "Top left"],
  ["top-center", "Top center"],
  ["top-right", "Top right"],
  ["bottom-left", "Bottom left"],
  ["bottom-center", "Bottom center"],
  ["bottom-right", "Bottom right"],
];
const TOAST_POSITION_DEFAULT = "bottom-right";
// How long toasts stay up (toast_duration); Normal is the default, left out of settings.json.
const TOAST_DURATIONS = [
  ["short", "Short"],
  ["normal", "Normal"],
  ["long", "Long"],
  ["until-dismissed", "Until clicked"],
];
// How toasts look (toast_style), each drawn as a small toast in the current theme
// (toastLooks().styles); Standard is the default, left out of settings.json.
const TOAST_STYLES = [
  ["standard", "Standard"],
  ["subtle", "Subtle"],
  ["high-contrast", "High contrast"],
  ["accent", "Accent"],
];
// A toast's corners (toast_corners), their radius from toastLooks().radius; Rounded is the default.
const TOAST_CORNERS = [
  ["square", "Square"],
  ["slight", "Slight"],
  ["rounded", "Rounded"],
];
// A toast's shape (toast_design), each drawn as a small toast in the current theme
// (toastLooks().designs); Card is the default, left out of settings.json.
const TOAST_DESIGNS = [
  ["card", "Card"],
  ["pill", "Pill"],
  ["terminal", "Terminal"],
  ["outline", "Outline"],
  ["icon-card", "Icon card"],
  ["banner", "Banner"],
];
// Whether a design takes toast_corners and toast_mark (toastLooks().takes, from
// settings.rs's ToastDesign): a pill's ends are always round, a terminal's and a
// banner's corners square, and the terminal's [WARN] and the outline's border are
// their marks. Both, when Herdr doesn't say.
const toastTakes = (design) => toastLooks()?.takes?.[design ?? "card"] ?? { corners: true, mark: true };
// The level mark down a toast's left (toast_mark); Stripe is the default.
const TOAST_MARKS = [
  ["stripe", "Stripe"],
  ["dot", "Dot"],
  ["none", "None"],
];
// A choice whose default isn't listed first: picking `def` leaves `key` out of settings.json.
const writeOrDrop = (key, def) => (p, value) => {
  if (value === def) delete p[key];
  else p[key] = value;
};
// The active pane's border width (focus_border_width); Layout, the layout's own, is left out of settings.json.
const FOCUS_WIDTHS = [
  [null, "Layout"],
  [1, "1 px"],
  [2, "2 px"],
  [3, "3 px"],
];
// When a cursor trail is drawn (cursor_trail); Off is the default, left out of settings.json.
const CURSOR_TRAILS = [
  ["off", "Off"],
  ["panes", "Between panes"],
  ["all", "Panes and jumps"],
];
// How the trail looks (cursor_trail_style); Comet is the default, left out of settings.json.
const TRAIL_STYLES = [
  ["comet", "Comet"],
  ["tail", "Tail"],
  ["sweep", "Sweep"],
  ["warp", "Warp"],
];
// The pulse as the focus lands (cursor_arrival); Off is the default, left out of settings.json.
const ARRIVALS = [
  ["off", "Off"],
  ["ripple", "Ripple"],
  ["boom", "Boom"],
  ["ripple_rect", "Ripple box"],
  ["boom_rect", "Boom box"],
];
// cursor_trail_settle_ms and cursor_trail_min_cells: the app's SETTLE_MS and MIN_CELLS ranges and defaults (settings.rs).
const SETTLE_MS = [0, 500];
const SETTLE_MS_DEFAULT = 10;
const MIN_CELLS = [0, 50];
const MIN_CELLS_DEFAULT = 2;
// cursor_trail_duration_ms: the app's TRAIL_MS range and TRAIL_MS_DEFAULT (settings.rs).
const TRAIL_MS = [50, 1000];
const TRAIL_MS_DEFAULT = 120;
// Theme colors offered for the active pane's border (colors.focus).
const FOCUS_SWATCHES = ["ansi1", "ansi2", "ansi3", "ansi4", "ansi5", "ansi6", "terminal_foreground"];
// pane_header_info's suggestion: what the agent asks, the branch, the state and for how long, its live sub-agents.
// A header info separator as the pane header mods draw it (their separator()): the mark
// alone with SEP_PAD (4) px either side, which is how fitHeaderInfo measures it.
function headerSeparator(t, sep) {
  return div().flex_none().px(4).text_color(t.dim).child(sep.trim() || sep);
}

const SUGGESTED_HEADER_INFO = "{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed} · {subagents}";
// agents_card_info while settings.json leaves it out (the app's DEFAULT_CARD_INFO): another machine's name, the space.
const DEFAULT_CARD_INFO = "@{remote} · {space}";

// How the agents list shows sub-agents (agents_subagents); Tree is the default, left out of settings.json.
const AGENTS_SUBAGENTS = [
  ["tree", "Tree"],
  ["folded", "Folded"],
  ["off", "Off"],
];
// What a click on a sub-agent row opens (agents_subagent_click); Claude's view is the default, left out of settings.json.
const AGENTS_SUBAGENT_CLICK = [
  ["claude", "Claude's view"],
  ["transcript", "Transcript"],
];
const RESUME_AGENTS = [
  ["ask", "Ask"],
  ["auto", "Auto"],
  ["off", "Off"],
];
const SYSTEM_NOTIFICATIONS = [
  ["background", "When away"],
  ["always", "Always"],
  ["off", "Off"],
];

// notification_sounds: silent, the system's sound, or one per state (Glass, Purr, Basso).
const NOTIFICATION_SOUNDS = [
  ["none", "Silent"],
  ["default", "System"],
  ["state", "By state"],
];
const SOUNDS_BY_STATE = { done: "Glass", blocked: "Purr", error: "Basso" };
const soundsKind = (v) =>
  v === undefined || v === null || v === false || v === "none" ? "none" : v === "default" || v === true ? "default" : typeof v === "object" ? "state" : "named";

// What the app uses when settings.json names no font (settings.rs).
const DEFAULT_FONT = "JetBrains Mono";
const DEFAULT_UI_FONT = "Geist";
const DEFAULT_UI_FONT_WEIGHT = 400;

// Search -----------------------------------------------------------------

// Lower case, with settings.json's _ - . read as spaces, so "cell_rounding"
// finds "Cell rounding". Keeps the length, so spans index the original text: a
// letter whose lower case is longer ("İ") stays as it is.
const fold = (s) => s.replace(/[\s\S]/g, (c) => { const l = c.toLowerCase(); return l.length === 1 ? l : c; }).replace(/[_\-.]/g, " ");

/** The typed words to look for; none while nothing is typed. */
export function searchWords(query) {
  return fold(query).split(/\s+/).filter(Boolean);
}

// Whether one typo (a letter added, dropped or changed, or two swapped) makes
// `a` into `b`.
function oneTypo(a, b) {
  if (Math.abs(a.length - b.length) > 1 || a === b) return a === b;
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  const rest = (x, y) => a.slice(x) === b.slice(y);
  return rest(i + 1, i + 1) || rest(i + 1, i) || rest(i, i + 1) || (a[i] === b[i + 1] && a[i + 1] === b[i] && rest(i + 2, i + 2));
}

/**
 * Where `word` (from searchWords) falls in `text`: [start, end], or null. Anywhere
 * as typed (a prefix or a substring); with `typos`, words of four letters or more
 * also find a whole word of the text one typo away that starts with the same
 * letter ("roundng" finds "rounding", "sound" doesn't find "round").
 */
export function wordSpan(word, text, typos) {
  const t = fold(text);
  const at = t.indexOf(word);
  if (at >= 0) return [at, at + word.length];
  if (!typos || word.length < 4) return null;
  for (const m of t.matchAll(/[a-z0-9]+/g)) {
    if (m[0][0] === word[0] && oneTypo(word, m[0])) return [m.index, m.index + m[0].length];
  }
  return null;
}

/**
 * How a setting matches `words`: every word must fall in its title, its keys
 * (settings.json names), its description or its section's heading. The spans
 * to mark per field, or null. Titles and keys forgive a typo; the longer
 * descriptions only match as typed, where a typo would find too much.
 */
export function matchSetting(words, entry) {
  if (!words.length) return null;
  const spans = { title: [], about: [], keys: entry.keys.map(() => []) };
  for (const w of words) {
    let hit = false;
    const mark = (list, span) => { if (span) { list.push(span); hit = true; } };
    mark(spans.title, wordSpan(w, entry.title, true));
    entry.keys.forEach((k, i) => mark(spans.keys[i], wordSpan(w, k, true)));
    mark(spans.about, entry.about ? wordSpan(w, entry.about, false) : null);
    if (!hit && !(entry.heading && wordSpan(w, entry.heading, false))) return null;
  }
  return spans;
}

/** `text` cut into [piece, marked] runs by `spans`, overlaps merged. */
export function markRuns(text, spans) {
  const lit = new Array(text.length).fill(false);
  for (const [a, b] of spans) for (let i = a; i < b && i < text.length; i++) lit[i] = true;
  const runs = [];
  for (let i = 0; i < text.length; i++) {
    const last = runs[runs.length - 1];
    if (last && last[1] === lit[i]) last[0] += text[i];
    else runs.push([text[i], lit[i]]);
  }
  return runs;
}

// Selection ---------------------------------------------------------------

const SELECTED_ALPHA = 0.16;
const SELECTED_RING_ALPHA = 0.5;
const INACTIVE_ALPHA = 0.1;
const INACTIVE_RING_ALPHA = 0.25;
const HOVER_ALPHA = 0.04;
const MIN_CONTRAST = 4.5;

const rgbOf = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const hexOf = (rgb) => `#${rgb.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;

/** `fg` laid `alpha` of the way over `bg`, opaque (#rrggbb; any alpha of their own dropped). */
export function over(fg, bg, alpha) {
  const [a, b] = [rgbOf(fg), rgbOf(bg)];
  return hexOf(a.map((v, i) => b[i] + (v - b[i]) * alpha));
}

/** WCAG contrast ratio between two colors. */
export function contrast(a, b) {
  const lum = (hex) => {
    const [r, g, bl] = rgbOf(hex).map((v) => (v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** A color in OKLCH: [lightness 0..1, chroma, hue in degrees]. */
export function oklch(hex) {
  const [r, g, b] = rgbOf(hex).map((v) => (v /= 255) <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const [L, A, B] = [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s, 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
  return [L, Math.hypot(A, B), ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360];
}

/** "warm", "neutral" or "cool": how a theme's background reads. */
export function themeTemperature(background) {
  const [, chroma, hue] = oklch(background);
  if (chroma < NEUTRAL_CHROMA) return "neutral";
  return hue >= WARM_FROM || hue < WARM_TO ? "warm" : "cool";
}

/** "soft", "standard" or "high": how hard a theme's text stands off its background. */
export function themeContrast(foreground, background) {
  const [fg, bg] = [oklch(foreground)[0], oklch(background)[0]];
  const gap = Math.abs(fg - bg);
  if (gap >= HIGH_GAP || fg >= 1 - NEAR_EXTREME) return "high";
  return gap < SOFT_GAP ? "soft" : "standard";
}

const HEX_COLOR = /^#[0-9a-f]{6}([0-9a-f]{2})?$/i;
const lookMemo = new Map();
/**
 * A theme's look tags from its card colours: { temperature, contrast }, each
 * null when the colour it reads is missing. Kept per colour pair, since the
 * Themes page asks again on every draw.
 */
export function themeLook(colors) {
  const [bg, fg] = [colors?.background, colors?.terminal_foreground];
  const key = `${bg}|${fg}`;
  let look = lookMemo.get(key);
  if (!look) {
    const ok = (c) => typeof c === "string" && HEX_COLOR.test(c);
    look = { temperature: ok(bg) ? themeTemperature(bg) : null, contrast: ok(bg) && ok(fg) ? themeContrast(fg, bg) : null };
    lookMemo.set(key, Object.freeze(look));
  }
  return look;
}

/**
 * How a selected row, tab, chip or card draws: `fill` the accent (or ring) at
 * a sixth over the dialog, `text` the accent moved toward the foreground until
 * it reads at 4.5:1 on that fill, `ring` its border. In a window that isn't
 * active, a neutral grey with foreground text instead.
 */
export function tonal(t, active) {
  const bg = t.popover ?? t.background;
  if (!active) return { fill: over(t.foreground, bg, INACTIVE_ALPHA), text: t.foreground, ring: over(t.foreground, bg, INACTIVE_RING_ALPHA) };
  const accent = t.accent ?? t.ring;
  const fill = over(accent, bg, SELECTED_ALPHA);
  let text = hexOf(rgbOf(accent));
  for (let k = 0.1; contrast(text, fill) < MIN_CONTRAST && k <= 1.001; k += 0.1) text = over(t.foreground, accent, k);
  return { fill, text, ring: over(accent, bg, SELECTED_RING_ALPHA) };
}

/** A pointer over something not selected: the foreground at a whisper, never the accent. */
const hoverFill = (t) => over(t.foreground, t.popover ?? t.background, HOVER_ALPHA);

export default class Settings extends View {
  init(_props, cx) {
    this.section = pageFor(props()?.section);
    const open = shared(OPEN_GROUPS);
    this.open = new Set([...(Array.isArray(open) ? open : []), groupOf(this.section)]);
    // The Themes page: the slot being picked for, the source shown, a filter
    // on names, and what the grid last scrolled to the current theme for.
    const s = settings();
    this.themeSlot = s.theme === s.theme_light && s.theme !== s.theme_dark ? "light" : "dark";
    this.themeSource = null;
    // Look chips: one temperature and one contrast at most, null for any.
    this.themeLooks = { temperature: null, contrast: null };
    this.themeFilter = "";
    this.themesClosed = new Set();
    this.themeScroll = VirtualListScrollHandle.new();
    this.themeScrolledFor = null;
    this.themeInput = InputState.new({ placeholder: "Filter themes" });
    this.themeInput.on("change", (_e, cx) => { this.themeFilter = this.themeInput.value(); cx.notify(); });
    this.themeInput.on("focus", (_e, cx) => { this.themeInputFocused = true; cx.notify(); });
    this.themeInput.on("blur", (_e, cx) => { this.themeInputFocused = false; cx.notify(); });
    this.picking = null; // "font" | "ui_font" while its list is open
    this.filter = "";
    this.fontInput = InputState.new({ placeholder: "Type to search fonts" });
    this.fontInput.on("change", (_e, cx) => { this.filter = this.fontInput.value(); cx.notify(); });
    this.renameInput = InputState.new();
    this.renameInput.on("focus", (_e, cx) => { this.renameFocused = true; cx.notify(); });
    this.renameInput.on("blur", (_e, cx) => { this.renameFocused = false; cx.notify(); });
    this.renameInput.on("change", (_e, cx) => { if (this.machineEdit?.action === "rename") this.machineEdit.name = this.renameInput.value(); cx.notify(); });
    this.renameInput.on("submit", (_e, cx) => {
      const edit = this.machineEdit;
      if (edit?.action === "rename") this.machineAction(edit.machine, "rename", cx, edit.name);
    });
    this.person = {};
    this.raw = null;
    this.previews = {}; // layout name -> its "preview" picture
    this.previewsOf = "";
    this.layoutFile = null; // the shown layout's file: { name, tree }
    this.showData = false;
    this.machineList = null;
    this.machineLoading = false;
    this.machineError = null;
    this.machineBusy = {};
    this.machineResults = {};
    this.machineEdit = null;
    this.machineNotice = null;
    // Search: the typed words, an index of every page's settings (built again
    // only when what the pages draw from changes), and what the words match.
    this.query = "";
    this.words = [];
    this.place = { page: this.section, heading: "" };
    this.index = null;
    this.matched = null;
    this.headerInputState();
    this.cardInputState();
    this.searchInput = InputState.new({ placeholder: "Search settings" });
    this.searchInput.on("change", (_e, cx) => { this.setQuery(this.searchInput.value(), false); cx.notify(); });
    this.searchInput.on("focus", (_e, cx) => { this.searchFocused = true; cx.notify(); });
    this.searchInput.on("blur", (_e, cx) => { this.searchFocused = false; cx.notify(); });
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.spawn(async (cx) => {
      await this.load(true);
      if (this.section === "machines") await this.loadMachines();
      cx.notify();
    });
    // After "Check now": follow the look (the answer comes from the app), then
    // "Up to date" for a moment.
    cx.timer.every(300, (cx) => {
      if (!this.asked) return;
      if (updates().checking) this.asked = Date.now();
      else if (Date.now() - this.asked >= 3000) this.asked = null;
      cx.notify();
    });
    cx.timer.every(3000, (cx) => {
      if (this.section === "layout") {
        // The layout file may change under us (an agent, an editor).
        cx.spawn(async (cx) => {
          if (await this.loadLayoutFile()) cx.notify();
        });
        return;
      }
      if (this.section !== "machines") return;
      cx.spawn(async (cx) => {
        await this.loadMachines();
        cx.notify();
      });
    });
  }

  file() {
    return `${appHome()}/settings.json`;
  }

  // Each layout file may name a picture of itself (`"preview"` on its root).
  loadPreviews(names, cx) {
    this.previewsOf = names.join("\n");
    cx.spawn(async (cx) => {
      const found = {};
      for (const name of names) {
        try {
          const preview = JSON.parse((await readTextFile(`${appHome()}/layouts/${name}.json`)) ?? "{}").preview;
          if (typeof preview === "string" && preview) found[name] = preview;
        } catch {}
      }
      this.previews = found;
      cx.notify();
    });
  }

  // `broken`: settings.json doesn't parse (mid-edit), or isn't an object, so
  // nothing may be written over it.
  // `syncHeader`: put settings.json's pane_header_info in its field (on opening;
  // not on a save's re-read, which would undo what the field is writing).
  async load(syncHeader = false) {
    this.raw = await readTextFile(this.file());
    try {
      const person = this.raw ? JSON.parse(this.raw) : {};
      this.broken = !person || typeof person !== "object" || Array.isArray(person);
      this.person = this.broken ? {} : person;
    } catch {
      this.person = {};
      this.broken = true;
    }
    const typed = this.person.pane_header_info ?? "";
    if (syncHeader && !this.headerFocused && this.headerInput && this.headerInput.value() !== typed) this.headerInput.set_value(typed);
    const card = this.person.agents_card_info ?? DEFAULT_CARD_INFO;
    if (syncHeader && !this.cardFocused && this.cardInput && this.cardInput.value() !== card) this.cardInput.set_value(card);
  }

  // pane_header_info as typed: previewed live, written on Enter or leaving the field.
  // Made in init (input state can't be made while rendering).
  headerInputState() {
    if (this.headerInput) return this.headerInput;
    const input = (this.headerInput = InputState.new({ placeholder: "Off" }));
    input.set_value(this.person.pane_header_info ?? "");
    input.on("change", (_e, cx) => cx.notify());
    input.on("focus", (_e, cx) => { this.headerFocused = true; cx.notify(); });
    input.on("blur", (_e, cx) => { this.headerFocused = false; this.setHeaderInfo(input.value(), cx); });
    input.on("submit", (_e, cx) => this.setHeaderInfo(input.value(), cx));
    return input;
  }

  // agents_card_info as typed, the default when unset: previewed live, written on Enter or leaving the field.
  cardInputState() {
    if (this.cardInput) return this.cardInput;
    const input = (this.cardInput = InputState.new({ placeholder: "Off" }));
    input.set_value(this.person.agents_card_info ?? DEFAULT_CARD_INFO);
    input.on("change", (_e, cx) => cx.notify());
    input.on("focus", (_e, cx) => { this.cardFocused = true; cx.notify(); });
    input.on("blur", (_e, cx) => { this.cardFocused = false; this.setCardInfo(input.value(), cx); });
    input.on("submit", (_e, cx) => this.setCardInfo(input.value(), cx));
    return input;
  }

  // agents_card_info: the default leaves it out of settings.json; blank is kept, as no card info.
  setCardInfo(text, cx) {
    if (text === (this.person.agents_card_info ?? DEFAULT_CARD_INFO)) return cx.notify();
    this.change((p) => {
      if (text === DEFAULT_CARD_INFO) delete p.agents_card_info;
      else p.agents_card_info = text;
    }, cx);
  }

  // pane_header_info: blank leaves it out of settings.json (headers as without it).
  setHeaderInfo(text, cx) {
    if (text === (this.person.pane_header_info ?? "")) return cx.notify();
    this.change((p) => {
      if (text.trim()) p.pane_header_info = text;
      else delete p.pane_header_info;
    }, cx);
  }

  // Changes settings.json the way an agent would: read, change, write back
  // only if nobody changed it meanwhile (else read again and redo). Says so
  // when it couldn't.
  change(edit, cx) {
    cx.spawn(async (cx) => {
      const failed = await this.save(edit);
      if (failed) notify({ ...failed, level: "error" });
      cx.notify();
    });
  }

  // The toast saying why `edit` wasn't written, or null once it is.
  async save(edit) {
    let error;
    for (let attempt = 0; attempt < 3; attempt++) {
      let next;
      try {
        await this.load();
        if (this.broken) return { title: "settings.json has errors: fix it first" };
        next = JSON.parse(JSON.stringify(this.person));
        edit(next);
      } catch (e) {
        return { title: "Cannot read settings.json", body: String(e?.message ?? e) };
      }
      try {
        await writeTextFile(this.file(), JSON.stringify(next, null, 2) + "\n", this.raw);
        this.person = next;
        return null;
      } catch (e) {
        error = e;
      }
    }
    return { title: "Not saved to settings.json", body: String(error?.message ?? error) };
  }

  // The shown layout's file as written, to tell its values from clicks on
  // them; null when it can't be read or doesn't parse. True when it changed
  // since the last read, so an unchanged file costs no redraw.
  async loadLayoutFile() {
    const name = shared("layout") ?? "main";
    let raw;
    try {
      raw = await readTextFile(`${appHome()}/layouts/${name}.json`);
    } catch (e) {
      // Kept as text so the same failure compares equal next time (no parse).
      raw = `\u0000${e?.message ?? e}`;
    }
    if (this.layoutFile?.name === name && this.layoutFile.raw === raw) return false;
    let tree = null;
    try {
      tree = raw ? JSON.parse(raw) : null;
    } catch {}
    // No tree: Herdr shows its shipped stand-in. It takes layout.spacing for
    // a missing file but refuses it for one it can't read or parse.
    this.layoutFile = { name, raw, missing: raw === null, tree: tree && typeof tree === "object" && !Array.isArray(tree) ? tree : null };
    return true;
  }

  // One timer, only while a step is pending: once the newest one expires,
  // whatever layoutSpacing() hasn't shown (a refused layout.spacing) goes.
  settleLater(cx) {
    this.spacingTimer?.cancel();
    this.spacingTimer = null;
    if (Object.keys(this.spacingPending ?? {}).length === 0) return;
    this.spacingTimer = cx.timer.after(SPACING_PENDING_MS + 50, (cx) => {
      this.spacingTimer = null;
      if (settlePending(this.spacingPending, spacingOf(layoutSpacing()), Date.now())) cx.notify();
      this.settleLater(cx);
    });
  }

  close() {
    closeOverlay(props().overlay);
  }

  // Search ---------------------------------------------------------------

  setQuery(query, toInput = true) {
    this.query = query;
    this.words = searchWords(query);
    if (toInput && this.searchInput.value() !== query) {
      this.searchInput.set_value(query);
      const end = encodeURIComponent(query).replace(/%[0-9A-F]{2}/g, "_").length;
      this.searchInput.set_selected_range({ start: end, end });
    }
  }

  // True while the pages draw as search results (not while the index is built).
  filtering() {
    return this.words?.length > 0 && !this.indexing;
  }

  // Every page's settings, from the pages themselves: what row() and block() see.
  buildIndex(t, cx) {
    // Pages not opened yet haven't read what they list: read it now, and index
    // again once it's here (results() sees the list change).
    if (this.machineList === null && !this.machineLoading && !this.machineError) cx.spawn(async (cx) => { await this.loadMachines(); cx.notify(); });
    this.index = new Map();
    this.indexing = true;
    try {
      for (const p of PAGES) {
        this.place = { page: p.id, heading: "" };
        this.page(p.id, t, cx);
      }
    } finally {
      this.indexing = false;
    }
    this.indexStale = false;
    this.matched = null;
  }

  page(id, t, cx) {
    switch (id) {
      case "themes": return this.themesPage(t);
      case "layout": return this.layoutPage(t, cx);
      case "window": return this.windowPage(t);
      case "panes": return this.panes(t);
      case "cursor": return this.cursorPage(t);
      case "zoom":
      case "fonts":
      case "grid":
      case "rendering": return this.text(t, id);
      case "keyboard": return this.keyboard(t);
      case "mouse": return this.followsMouse(t);
      case "activity": return this.activity(t);
      case "alerts": return this.alerts(t);
      case "machines": return this.machines(t);
      case "privacy": return this.privacy(t);
      default: return this.updates(t);
    }
  }

  // Every page with a match, each showing only its matches under their headings.
  results(t, cx) {
    // What the pages draw from: descriptions and conditional rows follow it (an
    // update check, a theme's panels), so a change means a new index.
    const from = JSON.stringify([settings(), updates(), telemetry(), layoutSpacing(), this.person, this.machineList, this.layoutFile?.name, this.layoutFile?.raw]);
    if (!this.index || this.indexStale || from !== this.indexedFrom) this.buildIndex(t, cx);
    this.indexedFrom = from;
    const m = this.match();
    const found = PAGES.filter((p) => m.pages.has(p.id));
    if (!found.length) {
      return v_flex()
        .id("st-none")
        .w_full()
        .pt(48)
        .gap(6)
        .items_center()
        .child(div().text_size(14).text_color(t.foreground).child("No settings match"))
        .child(div().text_size(12.5).text_color(t.dim).child(`Nothing has “${this.query.trim()}” in its name, description or settings.json key.`));
    }
    return v_flex()
      .w_full()
      .children(
        found.map((p) => {
          const g = GROUPS.find((g) => g.id === p.group);
          this.place = { page: p.id, heading: "" };
          return v_flex()
            .w_full()
            .pb(22)
            .child(
              h_flex()
                .id(`st-found-${p.id}`)
                .gap(8)
                .pb(10)
                .items_center()
                .cursor_pointer()
                .tooltip(`Open ${p.title}`)
                .on_click((_e, cx) => this.go(p.id, cx))
                .child(svg(I(g.icon)).size(14).text_color(t.accent))
                .child(div().text_size(15).font_semibold().text_color(t.foreground).child(`${g.title} › ${p.title}`)),
            )
            .child(this.page(p.id, t, cx));
        }),
      );
  }

  // Shows page `id`, leaving any search, with its group open.
  go(id, cx) {
    this.setQuery("");
    this.section = id;
    if (!this.open.has(groupOf(id))) this.toggleGroup(groupOf(id));
    this.themeScrolledFor = null;
    this.picking = null;
    this.machineEdit = null;
    this.handle.focus();
    if (id === "machines") cx.spawn(async (cx) => { await this.loadMachines(); cx.notify(); });
    // The layout file may have changed while another page showed.
    if (id === "layout") cx.spawn(async (cx) => { if (await this.loadLayoutFile()) cx.notify(); });
    cx.notify();
  }

  // Opens or closes group `id`, remembered for the next time Settings opens.
  toggleGroup(id) {
    if (!this.open.delete(id)) this.open.add(id);
    setShared(OPEN_GROUPS, GROUPS.map((g) => g.id).filter((g) => this.open.has(g)));
  }

  // A key typed with nothing focused that takes keys (no field, no list) starts
  // a search with it.
  typesIntoSearch(e) {
    const m = e.modifiers ?? {};
    const c = e.key_char;
    if (this.searchFocused || this.picking || this.renameFocused || this.machineEdit) return false;
    // Option types characters on macOS; elsewhere Alt is a shortcut.
    if (m.control || m.platform || m.function || (m.alt && window()?.platform !== "macos")) return false;
    if (typeof c !== "string" || c.length !== 1 || c < " " || c === "\u007f") return false;
    return !(c === " " && !this.query);
  }

  // What the words match in the index, once per change of words or index.
  match() {
    const q = this.words.join(" ");
    if (this.matched?.q === q && this.matched.index === this.index) return this.matched;
    const spans = new Map();
    const headings = new Set();
    const pages = new Map();
    for (const e of this.index.values()) {
      const hit = matchSetting(this.words, e);
      spans.set(e.id, hit);
      if (hit) {
        headings.add(`${e.page}|${e.heading}`);
        pages.set(e.page, (pages.get(e.page) ?? 0) + 1);
      }
    }
    this.matched = { q, index: this.index, spans, headings, pages };
    return this.matched;
  }

  // While searching, how a setting matches: its spans, or false to hide it;
  // null otherwise. Building the index records it instead.
  searchHit(title, about, keys) {
    if (!this.words?.length) return null;
    const { page, heading } = this.place;
    const id = `${page}|${heading}|${title}`;
    const entry = { id, page, heading, title, about: typeof about === "string" ? about : "", keys: [keys ?? []].flat().filter(Boolean) };
    if (this.indexing) {
      this.index.set(id, entry);
      return null;
    }
    const m = this.match();
    if (m.spans.has(id)) return m.spans.get(id) ?? false;
    // Shown since the index was built: matched here, indexed with the next build.
    const hit = matchSetting(this.words, entry);
    m.spans.set(id, hit);
    this.indexStale = true;
    return hit ?? false;
  }

  // A piece of a page that isn't a row (a picker, a list) but is a setting:
  // search finds it by `title`, `about` and `keys`, and shows it unmarked.
  block(title, about, keys, el) {
    return this.searchHit(title, about, keys) === false ? div() : el;
  }

  // Notes and spacing between settings: not shown in search results.
  plain(el) {
    return this.filtering() ? div() : el;
  }

  // Text with `spans` marked. `wrap`: it may run over lines, so each word is its
  // own piece and lines break between them. Marks tint the background only, so
  // nothing moves as they change.
  marked(t, text, spans, wrap) {
    const piece = ([s, lit]) => div().child(s).when(lit, (el) => el.bg(t.accent.length === 7 ? `${t.accent}40` : t.raised).rounded(2));
    const runs = markRuns(text, spans);
    if (!wrap) return h_flex().children(runs.map(piece));
    const words = [[]];
    for (const [s, lit] of runs) {
      s.split(/( )/).forEach((part) => {
        if (part === " ") words.push([]);
        else if (part) words[words.length - 1].push([part, lit]);
      });
    }
    return h_flex().flex_wrap().children(words.filter((w) => w.length).map((w) => h_flex().mr(3.5).children(w.map(piece))));
  }

  // A row as a search result: its matches marked, and the settings.json names
  // it writes.
  foundRow(t, title, about, control, keys, hit) {
    return h_flex()
      .w_full()
      .gap(16)
      .py(12)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(
        v_flex()
          .flex_1()
          .min_w(0)
          .gap(3)
          .child(div().text_size(14).text_color(t.foreground).child(this.marked(t, title, hit.title, false)))
          .when(Boolean(about), (el) => el.child(div().text_size(12.5).line_height("18px").text_color(t.dim).child(typeof about === "string" ? this.marked(t, about, hit.about, true) : about)))
          .when(keys.length > 0, (el) =>
            el.child(h_flex().gap(8).flex_wrap().children(keys.map((k, i) => div().font_family(DEFAULT_FONT).text_size(11.5).text_color(t.muted).child(this.marked(t, k, hit.keys[i], false)))))),
      )
      .child(div().flex_none().child(control));
  }

  // Pieces ---------------------------------------------------------------

  heading(t, text) {
    (this.place ??= { page: this.section, heading: "" }).heading = text;
    if (this.filtering() && !this.matched.headings.has(`${this.place.page}|${text}`)) return div();
    return div().pt(6).pb(8).text_size(12).font_semibold().text_color(t.dim).child(text.toUpperCase());
  }

  // One setting: what it is on the left, the control on the right. `keys`: the
  // settings.json names it writes, which search finds and shows.
  row(t, title, about, control, keys) {
    const hit = this.searchHit(title, about, keys);
    if (hit === false) return div();
    if (hit) return this.foundRow(t, title, about, control, [keys ?? []].flat().filter(Boolean), hit);
    return h_flex()
      .w_full()
      .gap(16)
      .py(12)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(
        v_flex()
          .flex_1()
          .min_w(0)
          .gap(3)
          .child(div().text_size(14).text_color(t.foreground).child(title))
          .when(Boolean(about), (el) => el.child(div().text_size(12.5).line_height("18px").text_color(t.dim).child(about))),
      )
      .child(div().flex_none().child(control));
  }

  button(t, id, label, onClick) {
    return div()
      .id(id)
      .px(12)
      .py(5)
      .rounded(7)
      .border_1()
      .border_color(t.border)
      .bg(t.raised)
      .cursor_pointer()
      .text_size(13)
      .text_color(t.foreground)
      .hover((s) => s.bg(t.hover))
      .on_click(onClick)
      .child(label);
  }

  // − value +
  stepper(t, id, value, onMinus, onPlus) {
    const b = (k, label, f) =>
      div()
        .id(`${id}-${k}`)
        .w(28)
        .h(28)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(15)
        .text_color(t.muted)
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click(f)
        .child(label);
    return h_flex()
      .items_center()
      .rounded(7)
      .border_1()
      .border_color(t.border)
      .bg(t.raised)
      .overflow_hidden()
      .child(b("minus", "\u2212", onMinus))
      .child(div().w(54).text_size(13).text_color(t.foreground).flex().justify_center().child(value))
      .child(b("plus", "+", onPlus));
  }

  toggle(t, id, on, onClick) {
    return div()
      .id(id)
      .w(36)
      .h(20)
      .rounded(10)
      .p(2)
      .cursor_pointer()
      .bg(on ? t.accent : t.hover)
      .flex()
      .when(on, (el) => el.justify_end())
      .on_click(onClick)
      .child(div().w(16).h(16).rounded(8).bg(on ? t.accent_foreground : t.muted));
  }

  // Look -----------------------------------------------------------------

  // A small drawing of each layout, in the theme's colors (as the welcome draws them).
  preview(t, name) {
    const row = () => div().flex().flex_row();
    const col = () => div().flex().flex_col();
    const term = (bg = t.background, edge = t.border) => div().flex_1().rounded(2).bg(bg).border_1().border_color(edge);
    const line = (w, c = t.dim) => div().h(3).w(w).flex_none().rounded(2).bg(c);
    const lines = (n, c) => col().gap(4).p(5).children([...Array(n)].map((_, i) => line(i % 2 ? 22 : 32, c)));
    const frame = (el) => el.size_full().rounded(6).overflow_hidden().border_1().border_color(t.border).bg(t.surface);
    if (name === "main")
      return frame(row())
        .child(col().w(42).border_r(1).border_color(t.border).child(lines(4)))
        .child(
          col()
            .flex_1()
            .child(row().h(11).gap(2).p(2).border_b(1).border_color(t.border).child(div().w(20).rounded(2).bg(t.accent)).child(div().w(16).rounded(2).bg(t.raised)).child(div().w(16).rounded(2).bg(t.raised)))
            .child(row().flex_1().p(4).gap(3).child(term()).child(term())),
        );
    if (name === "terminals") {
      const card = () => col().flex_1().rounded(4).border_1().border_color("#2c2f34").bg("#17191c").child(row().h(7).px(3).items_center().border_b(1).border_color("#25282c").child(line(14, "#5d6168")));
      return frame(col())
        .bg("#131619")
        .p(4)
        .gap(4)
        .child(row().h(10).gap(3).p(1).rounded(4).bg("#1d1f22").child(div().w(30).rounded(3).bg("#34373c").border_1().border_color("#4a4d53")).child(div().w(30)))
        .child(row().flex_1().gap(4).child(card()).child(card()));
    }
    // A layout of the person's own: a plain frame.
    return frame(col()).child(row().h(11).border_b(1).border_color(t.border)).child(row().flex_1().p(4).child(term()));
  }

  layoutPicker(t, cx) {
    const current = shared("layout") ?? "main";
    // Shipped looks in the welcome's order, then the person's own; new files show up by themselves.
    const shipped = Object.keys(LOOKS);
    const rank = (n) => (shipped.includes(n) ? shipped.indexOf(n) : shipped.length);
    const names = [...layouts()].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
    if (names.join("\n") !== this.previewsOf) this.loadPreviews(names, cx);
    return h_flex()
      .w_full()
      .flex_wrap()
      .gap(8)
      .children(
        names.map((name) => {
          const on = name === current;
          return v_flex()
            .id(`st-l-${name}`)
            .w(124)
            .gap(6)
            .p(6)
            .rounded(10)
            .border_2()
            .border_color(on ? t.accent : "#00000000")
            .bg(on ? t.raised : "#00000000")
            .cursor_pointer()
            .hover((s) => s.bg(t.raised))
            .on_click((_e, cx) => {
              run("layout.switch", { name });
              cx.notify();
            })
            .child(
              div()
                .h(62)
                .w_full()
                .overflow_hidden()
                .child(
                  this.previews[name]
                    ? image(this.previews[name]).w_full().h(62).rounded(6).border_1().border_color(t.border)
                    : this.preview(t, name),
                ),
            )
            .child(
              h_flex()
                .gap(5)
                .items_center()
                .child(div().text_size(12.5).text_color(on ? t.foreground : t.muted).child(LOOKS[name] ?? name))
                .when(on, (el) => el.child(svg(I("check")).size(12).text_color(t.accent))),
            );
        }),
      );
  }

  // The app icon: a tile per built-in; an image file (set in settings.json) shows as its path.
  iconPicker(t) {
    const now = this.person.app_icon ?? "classic";
    const file = !APP_ICONS.some(([name]) => name === now);
    return v_flex()
      .gap(6)
      .items_end()
      .child(
        h_flex()
          .gap(6)
          .flex_wrap()
          .justify_end()
          .children(
            APP_ICONS.map(([name, label]) =>
              v_flex()
                .id(`st-icon-${name}`)
                .items_center()
                .gap(3)
                .p(4)
                .rounded(10)
                .border_2()
                .border_color(now === name ? t.accent : "#00000000")
                .cursor_pointer()
                .hover((s) => s.bg(t.raised))
                .on_click((_e, cx) =>
                  this.change((p) => {
                    if (name === "classic") delete p.app_icon;
                    else p.app_icon = name;
                  }, cx),
                )
                .child(image(`frame/settings/app-icons/${name}.png`).w(40).h(40))
                .child(div().text_size(11).text_color(now === name ? t.foreground : t.muted).child(label)),
            ),
          ),
      )
      .when(file, (el) => el.child(div().id("st-icon-file").text_size(11.5).text_color(t.dim).child(`Your file: ${now}`)));
  }

  // One of a few values; picking the first (the default) leaves it out of settings.json.
  // `now` and `write` read and write a value that isn't one of the choices as written.
  choicePicker(t, id, key, choices, { now: shown, write } = {}) {
    const def = choices[0][0];
    const now = shown ?? this.person[key] ?? def;
    return h_flex()
      .p(2)
      .gap(2)
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      .children(
        choices.map(([value, label]) =>
          div()
            .id(`${id}-${label}`)
            .px(12)
            .py(4)
            .rounded(6)
            .text_size(13)
            .cursor_pointer()
            .when(now === value, (el) => el.bg(t.raised).text_color(t.foreground).border_1().border_color(t.border))
            .when(now !== value, (el) => el.text_color(t.muted).hover((s) => s.text_color(t.foreground)))
            .on_click((_e, cx) =>
              this.change((p) => {
                if (write) write(p, value);
                else if (value === def) delete p[key];
                else p[key] = value;
              }, cx),
            )
            .child(label),
        ),
      );
  }

  // A small window: its top row and bottom row of places, the chosen one the tonal pill,
  // each holding a toast-shaped bar on its edge and side. Its name underneath.
  toastPositionPicker(t) {
    const now = this.person.toast_position ?? TOAST_POSITION_DEFAULT;
    const sel = tonal(t, window()?.active !== false);
    const cell = ([value, label]) => {
      const on = now === value;
      const side = value.endsWith("left") ? "start" : value.endsWith("right") ? "end" : "center";
      return h_flex()
        .id(`st-tp-${value}`)
        .w(46)
        .h(24)
        .px(6)
        .py(4)
        .rounded(6)
        .border_1()
        .cursor_pointer()
        .when(value.startsWith("top"), (el) => el.items_start())
        .when(value.startsWith("bottom"), (el) => el.items_end())
        .when(side === "start", (el) => el.justify_start())
        .when(side === "center", (el) => el.justify_center())
        .when(side === "end", (el) => el.justify_end())
        .when(on, (el) => el.bg(sel.fill).border_color(sel.ring))
        .when(!on, (el) => el.border_color(t.background).hover((s) => s.bg(hoverFill(t))))
        .on_click((_e, cx) =>
          this.change((p) => {
            if (value === TOAST_POSITION_DEFAULT) delete p.toast_position;
            else p.toast_position = value;
          }, cx),
        )
        .child(div().w(16).h(5).rounded(2).bg(on ? sel.text : t.dim));
    };
    const label = (TOAST_POSITIONS.find(([v]) => v === now) ?? TOAST_POSITIONS.find(([v]) => v === TOAST_POSITION_DEFAULT))[1];
    return v_flex()
      .gap(4)
      .items_end()
      .child(
        v_flex()
          .p(2)
          .gap(10)
          .rounded(8)
          .border_1()
          .border_color(t.border)
          .bg(t.background)
          .child(h_flex().gap(2).children(TOAST_POSITIONS.slice(0, 3).map(cell)))
          .child(h_flex().gap(2).children(TOAST_POSITIONS.slice(3).map(cell))),
      )
      .child(div().id("st-tp-now").text_size(12).text_color(t.dim).child(label));
  }

  // One small toast per style, in the current theme's colours for that style, with the
  // chosen corners and level mark, the chosen style the tonal pill. Its name underneath.
  toastStylePicker(t) {
    const now = this.person.toast_style ?? "standard";
    const looks = toastLooks() ?? {};
    const styles = looks.styles ?? {};
    const sel = tonal(t, window()?.active !== false);
    // The corners and mark the chosen design draws for the person's own (toastLooks().takes).
    const drawn = looks.takes?.[this.person.toast_design ?? "card"];
    const radius = drawn?.radius ?? looks.radius?.[this.person.toast_corners] ?? looks.radius?.rounded ?? 8;
    const mark = drawn?.draws ?? this.person.toast_mark ?? "stripe";
    const cell = ([value, label]) => {
      const on = now === value;
      const c = styles[value] ?? t;
      const toast = h_flex()
        .w(58)
        .h(22)
        .gap(4)
        .items_center()
        .overflow_hidden()
        .rounded(radius / 2)
        .border_1()
        .border_color(c.toast_border)
        .bg(c.toast)
        .when(mark === "stripe", (el) => el.child(div().id(`st-ts-${value}-stripe`).w(2).h_full().flex_none().bg(c.toast_info)))
        .child(div().w(mark === "stripe" ? 1 : 4).flex_none())
        .when(mark !== "none", (el) => el.child(div().id(`st-ts-${value}-dot`).size(5).rounded(3).flex_none().bg(c.toast_info)))
        .child(div().w(22).h(3).rounded(1).bg(c.toast_foreground));
      return v_flex()
        .id(`st-ts-${value}`)
        .p(4)
        .gap(3)
        .items_center()
        .rounded(6)
        .border_1()
        .cursor_pointer()
        .when(on, (el) => el.bg(sel.fill).border_color(sel.ring))
        .when(!on, (el) => el.border_color(t.background).hover((s) => s.bg(hoverFill(t))))
        .on_click((_e, cx) => this.change((p) => writeOrDrop("toast_style", "standard")(p, value), cx))
        .child(toast)
        .child(div().text_size(11).text_color(on ? sel.text : t.dim).child(label));
    };
    return h_flex().p(2).gap(2).rounded(8).border_1().border_color(t.border).bg(t.background).children(TOAST_STYLES.map(cell));
  }

  // One small toast per design, in the current theme's colours for that design and the
  // chosen style (toastLooks().designs), the chosen one the tonal pill. Its name underneath.
  toastDesignPicker(t) {
    const now = this.person.toast_design ?? "card";
    const looks = toastLooks() ?? {};
    const designs = looks.designs ?? {};
    const sel = tonal(t, window()?.active !== false);
    const radius = (looks.radius?.[this.person.toast_corners] ?? looks.radius?.rounded ?? 8) / 2;
    const bar = (id, w, color) => div().id(id).w(w).h(3).rounded(1).flex_none().bg(color);
    const preview = (value, c) => {
      const box = (el) => el.id(`st-tg-${value}-toast`).items_center().overflow_hidden().border_1().border_color(c.toast_border).bg(c.toast);
      switch (value) {
        case "pill":
          return box(h_flex().w(46).h(14).px(6).gap(4).rounded(7)).child(div().size(5).rounded(3).flex_none().bg(c.toast_info)).child(bar(`st-tg-${value}-text`, 22, c.toast_foreground));
        case "terminal":
          return box(h_flex().w(58).h(22).px(4).gap(3))
            .font_family(looks.font_family ?? "monospace")
            .child(div().id(`st-tg-${value}-tag`).text_size(8).text_color(c.toast_info).child("[OK]"))
            .child(bar(`st-tg-${value}-text`, 18, c.toast_foreground));
        case "outline":
          return h_flex().id(`st-tg-${value}-toast`).w(58).h(22).px(6).gap(4).items_center().rounded(radius).border_1().border_color(c.toast_info).bg(c.toast_ghost ?? c.toast)
            .child(bar(`st-tg-${value}-text`, 26, c.toast_info));
        case "icon-card":
          return box(h_flex().w(58).h(26).px(4).gap(4).rounded(radius))
            .child(div().id(`st-tg-${value}-icon`).size(12).rounded(6).flex_none().bg(c.toast_info))
            .child(v_flex().gap(3).child(bar(`st-tg-${value}-text`, 26, c.toast_foreground)).child(bar(`st-tg-${value}-button`, 10, c.toast_info)));
        case "banner":
          return v_flex().w(58).h(26).rounded(3).border_1().border_color(t.border).bg(t.background).overflow_hidden()
            .child(box(h_flex().w_full().h(8).px(3).gap(3)).border_0().border_b(1).child(bar(`st-tg-${value}-text`, 20, c.toast_foreground)));
        default:
          return box(h_flex().w(58).h(22).gap(4).rounded(radius))
            .child(div().w(2).h_full().flex_none().bg(c.toast_info))
            .child(div().size(5).rounded(3).flex_none().bg(c.toast_info))
            .child(bar(`st-tg-${value}-text`, 22, c.toast_foreground));
      }
    };
    const cell = ([value, label]) => {
      const on = now === value;
      return v_flex()
        .id(`st-tg-${value}`)
        .p(4)
        .gap(3)
        .items_center()
        .rounded(6)
        .border_1()
        .cursor_pointer()
        .when(on, (el) => el.bg(sel.fill).border_color(sel.ring))
        .when(!on, (el) => el.border_color(t.background).hover((s) => s.bg(hoverFill(t))))
        .on_click((_e, cx) => this.change((p) => writeOrDrop("toast_design", "card")(p, value), cx))
        .child(preview(value, designs[value] ?? t))
        .child(div().text_size(11).text_color(on ? sel.text : t.dim).child(label));
    };
    return v_flex()
      .p(2)
      .gap(2)
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      .child(h_flex().gap(2).children(TOAST_DESIGNS.slice(0, 3).map(cell)))
      .child(h_flex().gap(2).children(TOAST_DESIGNS.slice(3).map(cell)));
  }

  themesPage(t) {
    const s = settings();
    const all = themes();
    const sel = tonal(t, window()?.active !== false);
    const tab = (slot, title, about, name) => {
      const on = this.themeSlot === slot;
      const c = all.find((x) => x.name === name)?.colors ?? {};
      return this.block(title, about, `theme.${slot}`, h_flex()
        .id(`st-tt-${slot}`)
        .flex_1()
        .min_w(0)
        .gap(10)
        .px(10)
        .py(8)
        .items_center()
        .rounded(9)
        .border_1()
        .border_color(on ? sel.ring : t.border)
        .bg(on ? sel.fill : t.background)
        .cursor_pointer()
        .when(!on, (el) => el.hover((st) => st.bg(hoverFill(t))))
        .on_click((_e, cx) => {
          this.themeSlot = slot;
          cx.notify();
        })
        .child(h_flex().w(34).h(22).flex_none().p(3).gap(2).items_end().rounded(5).border_1().border_color(t.border).bg(c.background ?? t.background).children(["ansi1", "ansi2", "ansi3", "ansi4"].map((k) => div().w(4).h(4).rounded(2).bg(c[k] ?? t.dim))))
        .child(v_flex().min_w(0).child(div().text_size(11).text_color(on ? sel.text : t.dim).child(title)).child(div().truncate().text_size(13.5).text_color(on ? sel.text : t.foreground).when(on, (el) => el.font_semibold()).child(name.replace(/-/g, " ")))));
    };
    return v_flex()
      .w_full()
      .child(this.heading(t, "Theme"))
      .child(this.row(t, "Appearance", "Light or dark, or as your system is now.", this.choicePicker(t, "st-ap", "appearance", APPEARANCES), "appearance"))
      .child(this.plain(div().pt(12).pb(10).text_size(12.5).text_color(t.dim).child("For the layout you're in: each layout keeps its own light and dark theme.")))
      .child(
        h_flex()
          .w_full()
          .gap(8)
          .child(tab("dark", "Dark theme", "Colours for the layout you're in, when dark.", s.theme_dark))
          .child(tab("light", "Light theme", "Colours for the layout you're in, when light.", s.theme_light)),
      )
      // While searching, the tabs say what's picked; the grid is on the page.
      .when(!this.words.length, (el) => el.child(this.themeGrid(t, all, sel)));
  }

  // The themes the shown slot can take that match the filter.
  themeCandidates(all) {
    const words = searchWords(this.themeFilter);
    const light = this.themeSlot === "light";
    const [layout, others] = [shared("layout"), layouts()];
    return all.filter(
      // A theme named like a layout (themes/win98.json) is that layout's own.
      (x) => Boolean(x.light) === light && (x.name === layout || !others.includes(x.name)) && words.every((w) => fold(x.name).includes(w)),
    );
  }

  // Whether a theme has the picked look chips, leaving out the `except` row's.
  themeHasLooks(x, except) {
    const look = themeLook(x.colors);
    return Object.entries(this.themeLooks).every(([row, id]) => row === except || !id || look[row] === id);
  }

  // The candidates with the picked looks, by source: [[source, title, themes]],
  // sources with none left out.
  themeGroups(all) {
    const shown = this.themeCandidates(all).filter((x) => this.themeHasLooks(x));
    return THEME_SOURCES.map(([id, title]) => [id, title, shown.filter((x) => themeSourceOf(x) === id)]).filter(([, , list]) => list.length);
  }

  // How many themes each look chip would list, with the source and the other row's pick.
  themeLookCounts(all) {
    const base = this.themeCandidates(all).filter((x) => !this.themeSource || themeSourceOf(x) === this.themeSource);
    const counts = {};
    for (const row of Object.keys(this.themeLooks)) {
      counts[row] = {};
      for (const x of base) {
        if (!this.themeHasLooks(x, row)) continue;
        const id = themeLook(x.colors)[row];
        if (id) counts[row][id] = (counts[row][id] ?? 0) + 1;
      }
    }
    return counts;
  }

  // The grid's rows for `groups`: a heading per source while more than one
  // shows (closed ones without their cards), then the cards THEME_COLUMNS to a row.
  themeRows(groups) {
    const shown = groups.filter(([id]) => !this.themeSource || id === this.themeSource);
    const rows = [];
    for (const [id, title, list] of shown) {
      if (shown.length > 1) rows.push({ head: id, title, count: list.length });
      if (shown.length > 1 && this.themesClosed.has(id)) continue;
      for (let i = 0; i < list.length; i += THEME_COLUMNS) rows.push({ cards: list.slice(i, i + THEME_COLUMNS) });
    }
    return rows;
  }

  // Source chips, a filter, and the cards as a list that draws only the rows
  // on screen. It shows the picked theme whenever what it lists changes.
  themeGrid(t, all, sel) {
    const s = settings();
    const current = this.themeSlot === "light" ? s.theme_light : s.theme_dark;
    const groups = this.themeGroups(all);
    const rows = this.themeRows(groups);
    const listed = `${this.themeSlot}|${this.themeSource}|${this.themeLooks.temperature}|${this.themeLooks.contrast}|${this.themeFilter}|${[...this.themesClosed]}`;
    if (listed !== this.themeScrolledFor) {
      this.themeScrolledFor = listed;
      const at = rows.findIndex((r) => r.cards?.some((x) => x.name === current));
      // Listed: the picked theme; filtered out: the top, not wherever the last listing was.
      if (rows.length) this.themeScroll.scroll_to_item(Math.max(at, 0), at >= 0 ? "center" : "top");
    }
    const pill = (key, on, label, count, pick) =>
      div()
        .id(key)
        .px(10)
        .py(3)
        .rounded(12)
        .border_1()
        .text_size(12)
        .cursor_pointer()
        .border_color(on ? sel.ring : t.border)
        .when(on, (el) => el.bg(sel.fill).text_color(sel.text).font_semibold())
        .when(!on, (el) => el.text_color(t.muted).hover((st) => st.bg(hoverFill(t))))
        .on_click((_e, cx) => {
          pick();
          cx.notify();
        })
        .child(`${label} ${count}`);
    const chip = (id, label, count) => pill(`st-tc-${id ?? "all"}`, this.themeSource === id, label, count, () => (this.themeSource = id));
    // A look chip toggles: a second click on the picked one shows every look again.
    const counts = this.themeLookCounts(all);
    const lookChips = (row, chips) =>
      chips.map(([id, label]) => {
        const pick = () => (this.themeLooks[row] = this.themeLooks[row] === id ? null : id);
        return pill(`st-tl-${id}`, this.themeLooks[row] === id, label, counts[row][id] ?? 0, pick);
      });
    const total = groups.reduce((n, [, , list]) => n + list.length, 0);
    const head = (r) =>
      h_flex()
        .id(`st-th-${r.head}`)
        .h(THEME_HEAD_H)
        .gap(6)
        .items_center()
        .cursor_pointer()
        .on_click((_e, cx) => {
          if (!this.themesClosed.delete(r.head)) this.themesClosed.add(r.head);
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/chevron-${this.themesClosed.has(r.head) ? "right" : "down"}.svg`).size(12).text_color(t.dim))
        .child(div().text_size(12.5).font_medium().text_color(t.muted).child(r.title))
        .child(div().text_size(12.5).text_color(t.dim).child(`${r.count}`));
    return v_flex()
      .w_full()
      .pt(12)
      .gap(8)
      .child(
        h_flex()
          .w_full()
          .gap(6)
          .items_center()
          .child(chip(null, "All", total))
          // The picked source keeps its chip when a look or the filter empties it.
          .children(THEME_SOURCES.map(([id, title]) => [id, title, groups.find(([g]) => g === id)?.[2].length ?? 0]).filter(([id, , n]) => n || id === this.themeSource).map(([id, title, n]) => chip(id, title, n)))
          .child(div().flex_1())
          .child(h_flex().w(160).h(26).px(8).items_center().rounded(7).border_1().border_color(this.themeInputFocused ? t.accent : t.border).bg(t.background).text_size(12).child(Input.new(this.themeInput).flex_1().h(24).text_color(t.foreground))),
      )
      .child(
        h_flex()
          .w_full()
          .gap(6)
          .items_center()
          .children(lookChips("temperature", THEME_TEMPERATURES))
          .child(div().w(1).h(16).mx(4).bg(t.border))
          .children(lookChips("contrast", THEME_CONTRASTS)),
      )
      .child(
        rows.length
          ? div()
              .relative()
              .w_full()
              .h(THEME_GRID_H)
              // Let the list scroll before stopping the page's wheel event.
              .on_scroll_wheel((_e, cx) => cx.stop_propagation())
              .child(
                v_virtual_list(
                  "st-themes",
                  rows.length,
                  rows.map((r) => (r.head ? THEME_HEAD_H : THEME_ROW_H)),
                  (i) => (rows[i].head ? `head:${rows[i].head}` : `row:${rows[i].cards[0].name}`),
                  (range) => rows.slice(range.start, range.end).map((r) => (r.head ? head(r) : h_flex().gap(4).children(r.cards.map((x) => this.themeCard(t, x, current, sel))))),
                )
                  .track_scroll(this.themeScroll)
                  .size_full(),
              )
              .child(Scrollbar.vertical("st-themes").absolute().inset_0())
          : div().id("st-themes-none").py(24).text_size(12.5).text_color(t.dim).child("No themes match."),
      );
  }

  themeCard(t, { name, colors: c }, current, sel) {
    const on = name === current;
    const label = (name.startsWith(REX_PREFIX) ? name.slice(REX_PREFIX.length) : name).replace(/-/g, " ");
    const dot = (color) => div().w(8).h(8).rounded(4).bg(color);
    return v_flex()
      .id(`st-t-${name}`)
      .w(THEME_CARD_W)
      .gap(4)
      .p(4)
      .rounded(10)
      .border_2()
      .border_color(on ? sel.ring : "#00000000")
      .cursor_pointer()
      .when(on, (el) => el.bg(sel.fill))
      .when(!on, (el) => el.hover((st) => st.bg(hoverFill(t))))
      .on_click((_e, cx) => {
        run("app.theme", { name });
        cx.notify();
      })
      .child(
        v_flex()
          .h(44)
          .w_full()
          .px(7)
          .py(6)
          .gap(5)
          .rounded(6)
          .border_1()
          .border_color(t.border)
          .bg(c.background)
          .child(h_flex().gap(5).font_family(DEFAULT_FONT).text_size(10).child(div().text_color(c.accent).child("❯")).child(div().text_color(c.terminal_foreground).child("ls")))
          .child(h_flex().gap(4).children(["ansi1", "ansi2", "ansi3", "ansi4", "ansi5", "ansi6"].map((k) => dot(c[k])))),
      )
      .child(
        h_flex()
          .w_full()
          .min_w(0)
          .gap(5)
          .items_center()
          .child(div().h(18).min_w(0).truncate().tooltip(label).text_size(12).text_color(on ? sel.text : t.muted).when(on, (el) => el.font_semibold()).child(label))
          .when(on, (el) => el.child(svg(I("check")).size(12).flex_none().text_color(sel.text))),
      );
  }

  // The font in use, written in itself; a click opens the list.
  fontButton(t, which, family, def) {
    const open = this.picking === which;
    const trigger = div()
      .id(`st-f-${which}`)
      .w(220)
      .px(12)
      .py(6)
      .rounded(7)
      .border_1()
      .border_color(open ? t.accent : t.border)
      .bg(t.raised)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => {
        this.picking = open ? null : which;
        this.filter = "";
        this.fontInput.set_value("");
        if (this.picking) this.fontInput.focus();
        else this.handle.focus();
        cx.notify();
      })
      .child(div().font_family(family).text_size(13.5).text_color(t.foreground).child(family));
    return Popup.new(`st-fp-${which}`, trigger)
      .when(open, (el) => el.content(this.fontList(t, which, def)));
  }

  fontList(t, which, def) {
    const q = this.filter.toLowerCase();
    const matches = fonts().filter((f) => f.toLowerCase().includes(q)).slice(0, 200);
    const choose = (name, cx) => {
      this.change((p) => {
        if (name === null) delete p[which];
        else p[which] = name;
      }, cx);
      this.picking = null;
      this.handle.focus();
      cx.notify();
    };
    const item = (id, label, family, onClick) =>
      div()
        .id(id)
        .px(10)
        .py(5)
        .rounded(6)
        .cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_click(onClick)
        .child(div().font_family(family).text_size(13.5).text_color(t.foreground).child(label));
    return v_flex()
      .id(`st-font-list-${which}`)
      .w(360)
      .on_mouse_down_out((_e, cx) => {
        this.picking = null;
        this.handle.focus();
        cx.notify();
      })
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      // Let the child's native scroll run before stopping the page's wheel event.
      .on_scroll_wheel((_e, cx) => cx.stop_propagation())
      .child(
        div()
          .px(12)
          .py(8)
          .border_b(1)
          .border_color(t.border)
          .text_size(13)
          .child(Input.new(this.fontInput).h(28).text_color(t.foreground)),
      )
      .child(
        div()
          .id(`st-fl-${which}`)
          .h(200)
          .p(4)
          .overflow_y_scrollbar()
          .when(!q, (el) => el.child(item(`st-fd-${which}`, `Default (${def})`, def, (_e, cx) => choose(null, cx))))
          .children(matches.map((f, i) => item(`st-fi-${which}-${i}`, f, f, (_e, cx) => choose(f, cx)))),
      );
  }

  layoutPage(t, cx) {
    return v_flex()
      .w_full()
      .child(this.heading(t, "Layout"))
      .child(this.block("Layout", "The layout Herdr draws its window with.", "layout", this.layoutPicker(t, cx)))
      .child(this.plain(div().h(18)))
      .child(this.spacing(t, cx));
  }

  windowPage(t) {
    return v_flex()
      .w_full()
      .child(this.heading(t, "Window"))
      .child(this.row(t, "App icon", "The Dock's icon. An install from a checkout puts it on the app too.", this.iconPicker(t), "app_icon"))
      .child(this.row(t, "Panel colours", settings().panels_in_use === "classic" && this.person.panel_colors !== "classic"
        ? "This theme or layout keeps classic panels, so it has no sidebar shade."
        : "Tinted takes each theme's own hue. Classic is plain grey with a darker sidebar, so it has no sidebar shade.", this.choicePicker(t, "st-pc", "panel_colors", PANEL_COLORS), "panel_colors"))
      .when(this.person.panel_colors !== "classic" && settings().panels_in_use !== "classic", (el) =>
        el.child(this.row(t, "Sidebar shade", "A shade darker sidebar on dark themes, while Herdr is the active window or always.", this.choicePicker(t, "st-ss", "sidebar_shade", SIDEBAR_SHADES), "sidebar_shade")),
      )
      .child(this.row(t, "Notification position", "Where toasts show. toast_offset in settings.json moves them in from the edges, to clear a status bar.", this.toastPositionPicker(t), ["toast_position", "toast_offset"]))
      .child(this.row(t, "Notification time", "How long toasts stay up: errors and warnings stay longer than the rest. Until clicked keeps them until you click one. A toast that sets its own time, such as Copied, keeps it.", this.choicePicker(t, "st-td", "toast_duration", TOAST_DURATIONS, { now: this.person.toast_duration ?? "normal", write: writeOrDrop("toast_duration", "normal") }), "toast_duration"))
      .child(this.row(t, "Notification design", "A toast's shape: a card, a one-line pill, terminal text, an outline over the window, a card with a big icon and buttons, or a banner across the window's top or bottom edge (the half of the position it's in), one at a time.", this.toastDesignPicker(t), "toast_design"))
      .child(this.row(t, "Notification style", "How toasts stand off the window: Subtle sits closer to the panels, High contrast has a stronger border and text, Accent takes the theme's accent. A theme's own toast colours still win.", this.toastStylePicker(t), "toast_style"))
      .when(toastTakes(this.person.toast_design).corners, (el) =>
        el.child(this.row(t, "Notification corners", "Square, slightly rounded or rounded toasts.", this.choicePicker(t, "st-tr", "toast_corners", TOAST_CORNERS, { now: this.person.toast_corners ?? "rounded", write: writeOrDrop("toast_corners", "rounded") }), "toast_corners")),
      )
      .when(toastTakes(this.person.toast_design).mark, (el) =>
        el.child(this.row(t, "Notification level mark", "What shows a toast's level: a stripe down its left side with a dot, the dot alone, or neither. A pill draws a dot for the stripe.", this.choicePicker(t, "st-tm", "toast_mark", TOAST_MARKS), "toast_mark")),
      )
      .child(
        this.row(
          t,
          "Notification countdown",
          "A thin bar along a toast's bottom edge showing the time left before it closes. Toasts kept until clicked have none.",
          this.toggle(t, "st-toast_progress", this.person.toast_progress === true, (_e, cx) =>
            this.change((p) => {
              if (p.toast_progress) delete p.toast_progress;
              else p.toast_progress = true;
            }, cx),
          ),
          "toast_progress",
        ),
      )
      .child(this.plain(div().h(18)))
      .child(this.heading(t, "Inactive window"))
      .child(this.contrast(t, "unfocused_dim", "Dim when inactive", "Fades this far toward the background while Herdr isn't the active window."))
      .child(this.row(t, "Dim", "The whole window, or only terminal text.", this.choicePicker(t, "st-ds", "unfocused_dim_scope", DIM_SCOPES), "unfocused_dim_scope"));
  }

  panes(t) {
    return v_flex()
      .w_full()
      .child(this.heading(t, "Panes"))
      .child(this.row(t, "New tabs arrange", "New tabs start in this arrangement: splits and closes arrange their panes again.", this.choicePicker(t, "st-da", "default_arrange", ARRANGES), "default_arrange"))
      .child(
        this.row(
          t,
          "Name panes automatically",
          "A pane at a bare shell that nobody named shows two words of its own, such as hazy-inlet, the same every time. Renaming it starts from that name.",
          this.toggle(t, "st-pane_auto_names", this.person.pane_auto_names === true, (_e, cx) =>
            this.change((p) => {
              if (p.pane_auto_names) delete p.pane_auto_names;
              else p.pane_auto_names = true;
            }, cx),
          ),
          "pane_auto_names",
        ),
      )
      .child(this.row(t, "Focus border", "The color around the pane you're typing in, where the layout draws it. From theme is the accent, or the theme's own focus color.", this.focusColorPicker(t), "colors.focus"))
      .child(this.row(t, "Focus border width", "Layout keeps the layout's own width. Panes keep their size as the focus moves.", this.choicePicker(t, "st-fw", "focus_border_width", FOCUS_WIDTHS), "focus_border_width"));
  }

  // Cursor effects: the trail (Off, Between panes, or that and the focused pane's cursor
  // jumps), its style while it's on, the arrival pulse, how long they take (20 ms steps)
  // while either is on, and, for jumps, how still the cursor must be (10 ms steps) and
  // how small a move it ignores. Off, Comet, Off, 120 ms, 10 ms and 2 cells are the defaults,
  // left out of settings.json.
  cursorPage(t) {
    const on = (this.person.cursor_trail ?? "off") !== "off";
    const jumps = this.person.cursor_trail === "all";
    const pulse = (this.person.cursor_arrival ?? "off") !== "off";
    const clamp = (ms) => Math.max(TRAIL_MS[0], Math.min(TRAIL_MS[1], ms));
    const ms = clamp(this.person.cursor_trail_duration_ms ?? TRAIL_MS_DEFAULT);
    const step = (d, cx) =>
      this.change((q) => {
        const next = clamp(Math.round((clamp(q.cursor_trail_duration_ms ?? TRAIL_MS_DEFAULT) + d) / 20) * 20);
        if (next === TRAIL_MS_DEFAULT) delete q.cursor_trail_duration_ms;
        else q.cursor_trail_duration_ms = next;
      }, cx);
    // A stepper over [lo, hi] in steps of `by` whose default `def` is left out of settings.json.
    const within = (key, [lo, hi], def, by) => {
      const clampTo = (v) => Math.max(lo, Math.min(hi, v));
      const go = (d, cx) =>
        this.change((q) => {
          const next = clampTo(Math.round((clampTo(q[key] ?? def) + d) / by) * by);
          if (next === def) delete q[key];
          else q[key] = next;
        }, cx);
      return [clampTo(this.person[key] ?? def), (_e, cx) => go(-by, cx), (_e, cx) => go(by, cx)];
    };
    const [settle, settleLess, settleMore] = within("cursor_trail_settle_ms", SETTLE_MS, SETTLE_MS_DEFAULT, 10);
    // cursor_trail_min_cells: one number for both axes, or [columns, rows];
    // written back as one number while they're equal, left out at the default.
    const cellsKey = "cursor_trail_min_cells";
    const cellsOf = (v) => {
      const keep = (n) => (Number.isFinite(n) ? Math.max(MIN_CELLS[0], Math.min(MIN_CELLS[1], Math.round(n))) : MIN_CELLS_DEFAULT);
      const [x, y] = Array.isArray(v) ? v : [v ?? MIN_CELLS_DEFAULT, v ?? MIN_CELLS_DEFAULT];
      return [keep(x ?? MIN_CELLS_DEFAULT), keep(y ?? MIN_CELLS_DEFAULT)];
    };
    const cells = cellsOf(this.person[cellsKey]);
    const cellStep = (axis, d) => (_e, cx) =>
      this.change((q) => {
        const pair = cellsOf(q[cellsKey]);
        pair[axis] = Math.max(MIN_CELLS[0], Math.min(MIN_CELLS[1], pair[axis] + d));
        if (pair[0] === pair[1] && pair[0] === MIN_CELLS_DEFAULT) delete q[cellsKey];
        else q[cellsKey] = pair[0] === pair[1] ? pair[0] : pair;
      }, cx);
    const cellsLabel = (n) => `${n} ${n === 1 ? "cell" : "cells"}`;
    return v_flex()
      .w_full()
      .child(this.heading(t, "Cursor effects"))
      .child(this.row(t, "Cursor trail", "A trail from one pane's cursor to the next as the focus moves between panes, and with Panes and jumps, as the cursor jumps in the pane you're typing in.", this.choicePicker(t, "st-ct", "cursor_trail", CURSOR_TRAILS), "cursor_trail"))
      .when(on, (el) => el.child(this.row(t, "Trail style", "Comet: a glowing streak. Tail: the cursor stretched, as kitty does. Sweep: a block shrinking into the cursor. Warp: a soft stretched block, as Neovide does.", this.choicePicker(t, "st-cs", "cursor_trail_style", TRAIL_STYLES), "cursor_trail_style")))
      .when(jumps, (el) =>
        el
          .child(this.row(t, "Settle time", "How long the program leaves the cursor still before a jump counts, so streaming output draws nothing until it pauses.", this.stepper(t, "st-cursor_trail_settle_ms", `${settle} ms`, settleLess, settleMore), "cursor_trail_settle_ms"))
          .child(this.row(t, "Ignore moves across up to", "Moves of this many columns or fewer, and no more rows than below, from where the cursor last settled draw nothing, so typing never does, as in kitty. 0 draws every move.", this.stepper(t, "st-cursor_trail_min_cells_x", cellsLabel(cells[0]), cellStep(0, -1), cellStep(0, 1)), cellsKey))
          .child(this.row(t, "Ignore moves down up to", "The same for rows: a move of more rows than this draws, however few columns it crosses.", this.stepper(t, "st-cursor_trail_min_cells_y", cellsLabel(cells[1]), cellStep(1, -1), cellStep(1, 1)), cellsKey)),
      )
      .child(this.row(t, "Arrival effect", "A pulse at the cursor as the focus lands in another pane, with the trail or without it; with Panes and jumps, also after a jump or a cursor shape change.", this.choicePicker(t, "st-ca", "cursor_arrival", ARRIVALS), "cursor_arrival"))
      .when(on || pulse, (el) =>
        el.child(this.row(t, "Effect time", "How long the trail and the pulse take.", this.stepper(t, "st-cursor_trail_duration_ms", `${ms} ms`, (_e, cx) => step(-20, cx), (_e, cx) => step(20, cx)), "cursor_trail_duration_ms")),
      );
  }

  activity(t) {
    return v_flex()
      .w_full()
      .child(this.heading(t, "Agents"))
      .child(this.row(t, "Working spinner", "What spins while an agent works: in pane headers, tabs, the agents list, the picker and peeks.", this.spinnerPicker(t), "working_spinner"))
      .child(
        this.row(
          t,
          "Sub-agents",
          "Each agent's live sub-agents in the agents list: a tree under it, the same folded to a count until you open it, or Off (no rows, no count). A fold you make is kept. Header info's {subagents} is set there, apart from this.",
          this.choicePicker(t, "st-sa", "agents_subagents", AGENTS_SUBAGENTS),
          "agents_subagents",
        ),
      )
      .child(
        this.row(
          t,
          "Sub-agent click",
          "What a click on a sub-agent row opens: the sub-agent in Claude Code's own view, or a transcript popup. A preference agents mods may honour: the shipped agents list doesn't read it and keeps focusing the parent agent.",
          this.choicePicker(t, "st-sc", "agents_subagent_click", AGENTS_SUBAGENT_CLICK),
          "agents_subagent_click",
        ),
      )
      .child(
        this.row(
          t,
          "Header info",
          "Extra words in an agent pane's header, as fit: {ask} what it needs from you, {state} {elapsed}, {branch}{ahead}{behind}{dirty}, {subagents} its live sub-agents, {machine} {remote} {space}. · or | separates segments; empty ones and those that don't fit are left out. Empty: off.",
          this.headerInfoField(t),
          "pane_header_info",
        ),
      )
      .child(
        this.row(
          t,
          "Card info",
          "What an agent's card in the agents list shows beside its name, as Header info's template: the same keys, and {machine} (local for this computer), {remote} (another machine's name, else nothing), {space}. Each segment is a chip; {space} is left out where a space header or the name already says it. Empty: off. The shipped agents list keeps its own look (the machine's name) until you set one.",
          this.cardInfoField(t),
          "agents_card_info",
        ),
      )
      .child(this.plain(div().h(18)))
      .child(this.heading(t, "After a restart"))
      .child(
        this.row(
          t,
          "Resume agents after restart",
          "Agents that ran in terminals now back to a plain shell, such as Claude Code: Ask shows a note to resume them, Auto resumes them, Off leaves the shells.",
          this.choicePicker(t, "st-ra", "resume_agents", RESUME_AGENTS),
          "resume_agents",
        ),
      );
  }

  // pane_header_info: the template, the suggestion, and the template as a
  // blocked agent's header would show it.
  headerInfoField(t) {
    const input = this.headerInputState();
    const typed = input.value();
    const preview = typed.trim() ? headerInfoPreview(typed) : null;
    return v_flex()
      .w(340)
      .gap(6)
      .child(
        h_flex()
          .gap(6)
          .items_center()
          .child(Input.new(input).flex_1().min_w(0).h(28).px(8).rounded(7).border_1().border_color(this.headerFocused ? t.accent : t.input).bg(t.background).text_size(12).font_family(DEFAULT_FONT).text_color(t.foreground))
          .when(typed !== SUGGESTED_HEADER_INFO, (el) =>
            el.child(
              div()
                .id("st-hi-suggested")
                .flex_none()
                .px(8)
                .py(4)
                .rounded(6)
                .text_size(12)
                .text_color(t.muted)
                .cursor_pointer()
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .on_click((_e, cx) => {
                  input.set_value(SUGGESTED_HEADER_INFO);
                  this.setHeaderInfo(SUGGESTED_HEADER_INFO, cx);
                })
                .child("Use suggested"),
            ),
          ),
      )
      .child(
        h_flex()
          .id("st-hi-preview")
          .h(18)
          .min_w(0)
          .items_center()
          .text_size(12)
          .when(!preview, (el) => el.text_color(t.dim).child(typed.trim() ? "Nothing to show" : "Off: headers show no extra info"))
          .when(Boolean(preview), (el) =>
            el.tooltip(preview.text).overflow_hidden().children(
              preview.segments.flatMap((s, i) => [
                ...(s.sep ? [headerSeparator(t, s.sep)] : []),
                div().when(i === preview.segments.length - 1, (d) => d.min_w(0).truncate()).when(i < preview.segments.length - 1, (d) => d.flex_none().whitespace_nowrap()).text_color(s.ask ? t.blocked : t.dim).child(s.text),
              ]),
            ),
          ),
      );
  }

  // agents_card_info: the template, the default, and the chips a card for an
  // agent on another machine would show.
  cardInfoField(t) {
    const input = this.cardInputState();
    const typed = input.value();
    const preview = typed.trim() ? cardInfoPreview(typed) : null;
    return v_flex()
      .w(340)
      .gap(6)
      .child(
        h_flex()
          .gap(6)
          .items_center()
          .child(Input.new(input).flex_1().min_w(0).h(28).px(8).rounded(7).border_1().border_color(this.cardFocused ? t.accent : t.input).bg(t.background).text_size(12).font_family(DEFAULT_FONT).text_color(t.foreground))
          .when(typed !== DEFAULT_CARD_INFO, (el) =>
            el.child(
              div()
                .id("st-ci-suggested")
                .flex_none()
                .px(8)
                .py(4)
                .rounded(6)
                .text_size(12)
                .text_color(t.muted)
                .cursor_pointer()
                .hover((s) => s.bg(t.hover).text_color(t.foreground))
                .on_click((_e, cx) => {
                  input.set_value(DEFAULT_CARD_INFO);
                  this.setCardInfo(DEFAULT_CARD_INFO, cx);
                })
                .child("Use suggested"),
            ),
          ),
      )
      .child(
        h_flex()
          .id("st-ci-preview")
          .h(18)
          .min_w(0)
          .gap(4)
          .items_center()
          .overflow_hidden()
          .text_size(11)
          .when(!preview, (el) => el.text_size(12).text_color(t.dim).child(typed.trim() ? "Nothing to show" : "Off: cards show only the name"))
          .when(Boolean(preview), (el) =>
            el.tooltip(preview.text).children(
              preview.segments.map((s, i) => div().when(i === preview.segments.length - 1, (d) => d.min_w(0).truncate()).when(i < preview.segments.length - 1, (d) => d.flex_none().whitespace_nowrap()).px(5).py(1).rounded(4).bg(t.chrome).text_color(s.ask ? t.blocked : t.muted).child(s.text)),
            ),
          ),
      );
  }

  // From theme (no colors.focus), or one of the shown theme's colors as it is
  // now; a hand-written color shows as its own swatch.
  focusColorPicker(t) {
    const now = this.person.colors?.focus;
    const swatches = [...new Set([...FOCUS_SWATCHES.map((name) => t[name]), now].filter(Boolean).map((c) => c.toLowerCase()))];
    const set = (value, cx) =>
      this.change((p) => {
        const colors = { ...p.colors };
        if (value) colors.focus = value;
        else delete colors.focus;
        if (Object.keys(colors).length) p.colors = colors;
        else delete p.colors;
      }, cx);
    const picked = (c) => now?.toLowerCase() === c;
    return h_flex()
      .p(2)
      .gap(4)
      .items_center()
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      .child(
        div()
          .id("st-fc-theme")
          .px(12)
          .py(4)
          .rounded(6)
          .text_size(13)
          .cursor_pointer()
          .when(!now, (el) => el.bg(t.raised).text_color(t.foreground).border_1().border_color(t.border))
          .when(Boolean(now), (el) => el.text_color(t.muted).hover((s) => s.text_color(t.foreground)))
          .on_click((_e, cx) => set(null, cx))
          .child("From theme"),
      )
      .children(
        swatches.map((c) =>
          div()
            .id(`st-fc-${c}`)
            .w(18)
            .h(18)
            .rounded(9)
            .bg(c)
            .border_2()
            .border_color(picked(c) ? t.foreground : "#00000000")
            .cursor_pointer()
            .tooltip(c)
            .on_click((_e, cx) => set(c, cx)),
        ),
      );
  }

  // working_spinner: each choice spinning as it will; the ring (the default)
  // leaves it out of settings.json, a name Herdr doesn't know shows the ring
  // it draws, and your own frames select none.
  spinnerPicker(t) {
    const now = settings().working_spinner ? this.person.working_spinner : "ring";
    const choices = [{ name: "ring" }, ...(settings().working_spinner_presets ?? [])];
    return h_flex()
      .max_w(360)
      .flex_wrap()
      .justify_end()
      .gap(4)
      .children(
        choices.map((c) =>
          h_flex()
            .id(`st-ws-${c.name}`)
            .gap(6)
            .px(8)
            .py(3)
            .items_center()
            .rounded(6)
            .border_1()
            .text_size(12)
            .cursor_pointer()
            .when(now === c.name, (el) => el.bg(t.raised).border_color(t.border).text_color(t.foreground))
            .when(now !== c.name, (el) => el.border_color(t.background).text_color(t.muted).hover((s) => s.text_color(t.foreground)))
            .on_click((_e, cx) =>
              this.change((p) => {
                if (c.name === "ring") delete p.working_spinner;
                else p.working_spinner = c.name;
              }, cx),
            )
            .child(
              c.frames
                ? div().min_w(12).flex_none().flex().justify_center().font_family(settings().font_family).text_color(t.working).frames(c.frames, c.interval_ms).child(c.frames[0])
                : svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900),
            )
            .child(c.name),
        ),
      );
  }

  // The Text group's pages: zoom, fonts, grid or rendering.
  text(t, page) {
    const s = settings();
    const p = this.person;
    // Numbers come back from the host as 32-bit floats: 1.1 reads as 1.100000023841858.
    const same = (a, b) => (typeof a === "number" && typeof b === "number" ? Math.abs(a - b) < 1e-4 : a === b);
    const own = (key, composed, def) => !same(p[key] ?? def, composed);
    const size = (key, value, d, cx) => this.change((q) => (q[key] = Math.max(8, Math.min(32, (q[key] ?? value) + d))), cx);
    const step = (key, value, d, lo, hi, cx) => this.change((q) => (q[key] = Math.round(Math.max(lo, Math.min(hi, (q[key] ?? value) + d)) * 100) / 100), cx);
    // 100% is the default, left out of settings.json.
    const scrollStep = (d, cx) =>
      this.change((q) => {
        const next = Math.round(Math.max(0.1, Math.min(5, (q.terminal_scroll_speed ?? s.terminal_scroll_speed) + d)) * 10) / 10;
        if (next === 1) delete q.terminal_scroll_speed;
        else q.terminal_scroll_speed = next;
      }, cx);
    // Upstream's steps (#962): 5% within 0.8..2; 100% is the default, left out of settings.json.
    const cellWidthStep = (d, cx) =>
      this.change((q) => {
        const next = Math.max(0.8, Math.min(2, Math.round(Math.round(((q.terminal_cell_width ?? s.terminal_cell_width) + d) / 0.05) * 0.05 * 100) / 100));
        if (next === 1) delete q.terminal_cell_width;
        else q.terminal_cell_width = next;
      }, cx);
    // Writes terminal_line_height, taking line_height (its other name) with it.
    const lineStep = (d, cx) =>
      this.change((q) => {
        q.terminal_line_height = Math.round(Math.max(0.8, Math.min(2, (q.terminal_line_height ?? q.line_height ?? s.terminal_line_height) + d)) * 100) / 100;
        delete q.line_height;
      }, cx);
    // Fractional settings arrive as f32 widened to f64 (1.15 as 1.1499999...).
    const ownFraction = (key, composed, def) => Math.abs((p[key] ?? def) - composed) > 0.001;
    const fraction = (v) => `${Math.round(v * 100) / 100}`;
    // A percent in 5% steps within Herdr's range; 100% leaves it out of settings.json.
    const scaleStepper = (key, value) => {
      const now = value ?? 1;
      const go = (d, cx) =>
        this.change((q) => {
          const next = scaleStep(q[key], now, d, s.ui_scale_ranges?.[key] ?? [now, now]);
          if (next === 1) delete q[key];
          else q[key] = next;
        }, cx);
      return this.stepper(t, `st-${key}`, `${Math.round(now * 100)}%`, (_e, cx) => go(-0.05, cx), (_e, cx) => go(0.05, cx));
    };
    // 400 is the default, left out of settings.json. Layouts don't set it.
    const weightIn = (w) => Math.max(100, Math.min(900, w ?? DEFAULT_UI_FONT_WEIGHT));
    const uiWeight = weightIn(this.person.ui_font_weight);
    const uiWeightStep = (d, cx) =>
      this.change((q) => {
        const next = weightIn(weightIn(q.ui_font_weight) + d);
        if (next === DEFAULT_UI_FONT_WEIGHT) delete q.ui_font_weight;
        else q.ui_font_weight = next;
      }, cx);
    // Unset, bold draws at 700 or the weight when that is heavier; stepping back there leaves it out of settings.json.
    const boldAuto = Math.max(s.font_weight, 700);
    const bold = s.font_weight_bold ?? boldAuto;
    const boldStep = (d, cx) =>
      this.change((q) => {
        const next = Math.max(100, Math.min(900, (q.font_weight_bold ?? bold) + d));
        if (next === boldAuto) delete q.font_weight_bold;
        else q.font_weight_bold = next;
      }, cx);
    const fontRow = (which, sizeKey, title, family, def, sizeNow) =>
      v_flex()
        .w_full()
        .child(
          this.row(
            t,
            title,
            own(which, family, def) ? "This layout sets its own; yours shows in the others." : null,
            h_flex()
              .gap(8)
              .items_center()
              .child(this.fontButton(t, which, family, def))
              // The panels' text follows zoom: they size their rows to it.
              .when(Boolean(sizeKey), (el) => el.child(this.stepper(t, `st-${sizeKey}`, `${Math.round(sizeNow)}`, (_e, cx) => size(sizeKey, sizeNow, -1, cx), (_e, cx) => size(sizeKey, sizeNow, 1, cx)))),
            [which, sizeKey],
          ),
        );
    // At whole-point steps, the terminals' text size in points.
    const zoomShown = s.zoom_step === "point" ? `${Math.round(s.font_size * s.zoom * 10) / 10} pt` : `${Math.round(s.zoom * 100)}%`;
    const el = v_flex().w_full();
    switch (page) {
      case "zoom":
        return el
          .child(this.heading(t, "Size"))
          .child(
            this.row(
              t,
              "Zoom",
              "Everything: terminals, panels, text.",
              h_flex()
                .gap(8)
                .items_center()
                .child(this.stepper(t, "st-zoom", zoomShown, () => run("app.zoom_out"), () => run("app.zoom_in")))
                .child(this.button(t, "st-zoom-reset", "Reset", () => run("app.zoom_reset"))),
              "zoom",
            ),
          )
          .child(this.row(t, "Zoom steps", "Whole points move the terminals' text a point at a time, as Ghostty does; the panels scale with it.", this.choicePicker(t, "st-zs", "zoom_step", ZOOM_STEPS), "zoom_step"))
          .child(
            this.row(
              t,
              "Pane zoom keys",
              `${window()?.platform === "macos" ? "Ctrl = and Ctrl − (Ctrl 0 resets)" : "Keys bound to pane.zoom_text_in, _out and _reset in keys.json"} zoom only the focused terminal's text, in zoom steps, on top of Zoom. Off, those keys reach the program.`,
              this.toggle(t, "st-pane_zoom_keys", s.pane_zoom_keys !== false, (_e, cx) =>
                this.change((q) => {
                  if (s.pane_zoom_keys !== false) q.pane_zoom_keys = false;
                  else delete q.pane_zoom_keys;
                }, cx),
              ),
              "pane_zoom_keys",
            ),
          )
          .child(this.row(t, "Interface zoom", "Panels, tabs and popups, on top of zoom. Terminals keep their size.", scaleStepper("ui_zoom", s.ui_zoom), "ui_zoom"))
          .child(this.row(t, "Interface text size", "The panels' text only: spacing and icons keep their size.", scaleStepper("ui_text_scale", s.ui_text_scale), "ui_text_scale"));
      case "fonts":
        return el
          .child(this.heading(t, "Interface text"))
          .child(fontRow("ui_font", null, "Interface font", s.ui_font_family, DEFAULT_UI_FONT))
          .child(this.row(t, "Interface font weight", "Panels, tabs and popups. Bold text stays as much bolder; terminals keep theirs.", this.stepper(t, "st-ui_font_weight", `${uiWeight}`, (_e, cx) => uiWeightStep(-25, cx), (_e, cx) => uiWeightStep(25, cx)), "ui_font_weight"))
          .child(this.plain(div().h(18)))
          .child(this.heading(t, "Terminal text"))
          .child(fontRow("font", "font_size", "Font", s.font_family, DEFAULT_FONT, s.font_size))
          .child(
            this.row(
              t,
              "Weight",
              own("font_weight", s.font_weight, 500) ? "This layout sets its own; yours shows in the others." : "Bold text is at least 700.",
              this.stepper(t, "st-font_weight", `${s.font_weight}`, (_e, cx) => step("font_weight", s.font_weight, -25, 100, 900, cx), (_e, cx) => step("font_weight", s.font_weight, 25, 100, 900, cx)),
              "font_weight",
            ),
          )
          .child(
            this.row(
              t,
              "Bold weight",
              own("font_weight_bold", s.font_weight_bold, null) ? "This layout sets its own; yours shows in the others." : "Unset, 700 or the weight when that is heavier.",
              this.stepper(t, "st-font_weight_bold", `${bold}`, (_e, cx) => boldStep(-25, cx), (_e, cx) => boldStep(25, cx)),
              "font_weight_bold",
            ),
          )
          .child(
            this.row(
              t,
              "Ligatures",
              "Joins pairs like -> and != where the font has them.",
              this.toggle(t, "st-font_ligatures", s.font_ligatures, (_e, cx) =>
                this.change((q) => {
                  if (s.font_ligatures) delete q.font_ligatures;
                  else q.font_ligatures = true;
                }, cx),
              ),
              "font_ligatures",
            ),
          );
      case "grid":
        return el
          .child(this.heading(t, "Terminal grid"))
          .child(
            this.row(
              t,
              "Line height",
              Math.abs((p.terminal_line_height ?? p.line_height ?? 1.15) - s.terminal_line_height) > 0.001 ? "This layout sets its own; yours shows in the others." : "Row height, as a multiple of the font's height.",
              this.stepper(t, "st-line_height", fraction(s.terminal_line_height), (_e, cx) => lineStep(-0.05, cx), (_e, cx) => lineStep(0.05, cx)),
              "terminal_line_height",
            ),
          )
          .child(
            this.row(
              t,
              "Scroll speed",
              ownFraction("terminal_scroll_speed", s.terminal_scroll_speed, 1) ? "This layout sets its own; yours shows in the others." : "Wheel and trackpad scrolling, mouse reports to programs included.",
              this.stepper(t, "st-terminal_scroll_speed", `${Math.round(s.terminal_scroll_speed * 100)}%`, (_e, cx) => scrollStep(-0.1, cx), (_e, cx) => scrollStep(0.1, cx)),
              "terminal_scroll_speed",
            ),
          )
          .child(
            this.row(
              t,
              "Cell width",
              ownFraction("terminal_cell_width", s.terminal_cell_width, 1) ? "This layout sets its own; yours shows in the others." : "Column width, as Ghostty's adjust-cell-width (95% is -5%; Ghostty's very cells when cells snap to the nearest pixel). Letters are placed in their cells as Ghostty places them.",
              this.stepper(t, "st-terminal_cell_width", `${Math.round(s.terminal_cell_width * 100)}%`, (_e, cx) => cellWidthStep(-0.05, cx), (_e, cx) => cellWidthStep(0.05, cx)),
              "terminal_cell_width",
            ),
          )
          .child(this.row(t, "Cell rounding", "Nearest keeps cells and rows within half a pixel of the font's size; Outward rounds cells down and rows up.", this.choicePicker(t, "st-cr", "cell_rounding", CELL_ROUNDINGS), "cell_rounding"));
      default:
        return el
          .child(this.heading(t, "Interface text"))
          .child(this.thicken(t, "ui_font_thicken", "Thicken interface text", "Panels, tabs and popups as heavy whatever their color (On), or unsmoothed (Off). Terminals follow their own."))
          .child(this.contrast(t, "ui_text_contrast", "Interface contrast", "Crisper edges on the panels' text, at the same weight. Terminals follow their own."))
          .child(this.row(t, "Interface text smoothing", "Trial. Subpixel colors each edge by your display's stripes, on 1x displays; others stay grayscale.", this.choicePicker(t, "st-uitr", "ui_text_rendering", TEXT_RENDERINGS), "ui_text_rendering"))
          .child(this.plain(div().h(18)))
          .child(this.heading(t, "Terminal text"))
          .child(this.thicken(t, "font_thicken", "Thicken", "Terminal text as heavy whatever its color (On), as Ghostty's font-thicken does, or unsmoothed (Off)."))
          .child(this.contrast(t, "text_contrast", "Contrast", "Crisper edges on terminal text, at the same weight."))
          .child(this.contrast(t, "faint_opacity", "Faint text", "How opaque dim (faint) terminal text draws. 0.5, as Ghostty's faint-opacity.", 0.5))
          .child(this.row(t, "Text smoothing", "Trial. Subpixel colors each edge by your display's stripes, on 1x displays; others stay grayscale.", this.choicePicker(t, "st-tr", "text_rendering", TEXT_RENDERINGS), "text_rendering"))
          .when(this.person.text_rendering === "subpixel" || this.person.ui_text_rendering === "subpixel", (el) =>
            el.child(this.row(t, "Subpixel order", "Your display's color stripes, left to right. Most are RGB.", this.choicePicker(t, "st-so", "subpixel_order", SUBPIXEL_ORDERS), "subpixel_order")),
          );
    }
  }

  // The shown layout's spacing (layoutSpacing() has its clicks on it):
  // set as clicks on it, so its file stays as written. Reset shows where a
  // value differs from the file's (set or not) and puts the file's back.
  spacing(t, cx) {
    const name = shared("layout") ?? "main";
    if (this.layoutFile?.name !== name && this.layoutFileFor !== name) {
      this.layoutFileFor = name;
      cx.spawn(async (cx) => {
        await this.loadLayoutFile();
        this.layoutFileFor = null;
        cx.notify();
      });
    }
    const box = v_flex().w_full().child(this.heading(t, "Spacing"));
    const note = (text) => this.plain(div().pt(10).text_size(12.5).line_height("18px").text_color(t.dim).child(text));
    // layoutSpacing(), not layout(): this redraws when spacing changes, not on
    // every resize or click on the layout.
    const shown = layoutSpacing();
    const s = spacingOf(shown);
    if (!s) return box.child(note("This layout shows no tab's panes."));
    // Herdr refuses layout.spacing while the file doesn't parse: no steppers then.
    if (this.layoutFile?.name !== name) return box;
    if (!this.layoutFile.tree && !this.layoutFile.missing) return box.child(note(`layouts/${name}.json can't be read or has errors: fix it to change spacing.`));
    // A step not yet in layoutSpacing() (layout.spacing applies after this returns):
    // the next step counts from it, so quick clicks each move one. Steps
    // belong to the layout they were made on.
    if (this.spacingPendingFor !== name) this.spacingPending = {};
    this.spacingPendingFor = name;
    this.spacingPending ??= {};
    settlePending(this.spacingPending, s, Date.now());
    for (const [key, { value }] of Object.entries(this.spacingPending)) s[key] = value;
    const raw = shown;
    // A missing file's values are the shipped stand-in's, which this can't
    // see: no Reset then.
    const fileRaw = this.layoutFile.tree ? spacingFieldsOf(this.layoutFile.tree) ?? {} : null;
    const clicked = (key) => fileRaw !== null && !sameSpacing(raw[key], fileRaw[key]);
    const round = (v) => Math.round(v * 100) / 100;
    // Pairs read x, y (sides, then top and bottom), as terminal padding does;
    // layouts write padding [y, x].
    const show = (v, key) => {
      if (!Array.isArray(v)) return `${round(v)}`;
      const [a, b] = key === "area_padding" ? [v[1], v[0]] : v;
      return sameSpacing(a, b) ? `${round(a)}` : `${round(a)}, ${round(b)}`;
    };
    const set = (key, value, cx) => {
      if (key === "reset") for (const k of value) delete this.spacingPending[k];
      else this.spacingPending[key] = { value, at: Date.now() };
      run("layout.spacing", { [key]: value });
      this.settleLater(cx);
      cx.notify();
    };
    const steps = (id, value, to, key) => this.stepper(t, id, show(value, key), (_e, cx) => to(-1, cx), (_e, cx) => to(1, cx));
    const reset = (key) => (clicked(key) ? this.button(t, `st-sp-${key}-reset`, "Reset", (_e, cx) => set("reset", [key], cx)) : null);
    const px = (key, title, about) => {
      if (s[key] === null) return null;
      const now = s[key];
      const to = (d, cx) => {
        const next = Array.isArray(now) ? now.map((v) => stepSpacing(v, d)) : stepSpacing(now, d);
        set(key, Array.isArray(next) && next[0] === next[1] ? next[0] : next, cx);
      };
      const r = reset(key);
      return this.row(t, title, about, h_flex().gap(8).items_center().child(steps(`st-sp-${key}`, now, to, key)).when(Boolean(r), (el) => el.child(r)), `layout.spacing.${key}`);
    };
    const [x, y] = s.terminal_padding;
    const axis = (i) => (d, cx) => set("terminal_padding", i === 0 ? [stepSpacing(x, d), y] : [x, stepSpacing(y, d)], cx);
    const tr = reset("terminal_padding");
    const missing = [
      s.pane_gap === null ? "doesn't frame its panes, so they have no gap to set" : null,
      s.area_gap === null ? "has no row or column around its panes, so their container has no spacing to set" : null,
    ].filter(Boolean);
    return box
      .children(
        [
          px("pane_gap", "Pane gap", "Space between a tab's panes."),
          px("area_padding", "Pane container padding", "Padding inside the row or column the panes sit in: sides, then top and bottom."),
          px("area_gap", "Pane container gap", "Space between the panes and what sits beside them in that row or column."),
        ].filter(Boolean),
      )
      .child(
        this.row(
          t,
          "Terminal padding",
          "Space between a terminal's edge and its text: sides, then top and bottom.",
          h_flex()
            .gap(8)
            .items_center()
            .child(steps("st-sp-terminal_padding-x", x, axis(0)))
            .child(steps("st-sp-terminal_padding-y", y, axis(1)))
            .when(Boolean(tr), (el) => el.child(tr)),
          "layout.spacing.terminal_padding",
        ),
      )
      .children(missing.map((m) => note(`This layout ${m}.`)))
      .child(note("For the layout you're in. Its file stays as written; a later edit of the same value there wins."));
  }

  // Font thickening (`key` is font_thicken for terminals, ui_font_thicken for
  // the panels): Auto, On or Off, then the strength while it's On. Layouts
  // don't set either. Strength steps by 16 within 0..255, so it lands on each
  // level macOS smoothing draws (about 112, 192, 224 and 255). Auto and 255 are
  // the defaults, left out of settings.json.
  thicken(t, key, label, hint) {
    const strengthKey = `${key}_strength`;
    const on = this.person[key] === true;
    const strength = Math.max(0, Math.min(255, this.person[strengthKey] ?? 255));
    const go = (d, cx) =>
      this.change((q) => {
        const now = Math.max(0, Math.min(255, q[strengthKey] ?? 255));
        const next = Math.max(0, Math.min(255, Math.round((now + d) / 16) * 16));
        if (next === 255) delete q[strengthKey];
        else q[strengthKey] = next;
      }, cx);
    return v_flex()
      .w_full()
      .child(this.row(t, label, hint, this.choicePicker(t, `st-${key}`, key, THICKENS), key))
      .when(on, (el) =>
        el.child(this.row(t, "Thicken strength", "Like Ghostty's font-thicken-strength; 0 to 96 look the same.", this.stepper(t, `st-${strengthKey}`, `${strength}`, (_e, cx) => go(-16, cx), (_e, cx) => go(16, cx)), strengthKey)),
      );
  }

  // A fraction (text_contrast for terminals, ui_text_contrast for the panels,
  // unfocused_dim, faint_opacity): 0 to 1 in steps of 0.1. Layouts don't set
  // any. The default (0 unless given) is left out of settings.json.
  contrast(t, key, label, hint, base = 0) {
    const now = Math.max(0, Math.min(1, this.person[key] ?? base));
    const go = (d, cx) =>
      this.change((q) => {
        const was = Math.max(0, Math.min(1, q[key] ?? base));
        const next = Math.max(0, Math.min(1, Math.round((was + d) * 10) / 10));
        if (next === base) delete q[key];
        else q[key] = next;
      }, cx);
    return this.row(t, label, hint, this.stepper(t, `st-${key}`, `${Math.round(now * 10) / 10}`, (_e, cx) => go(-0.1, cx), (_e, cx) => go(0.1, cx)), key);
  }

  // Focus follows mouse: a toggle, then its delay (50 ms steps) while it's on.
  // Off and 150 ms are the defaults, left out of settings.json.
  followsMouse(t) {
    const on = this.person.focus_follows_mouse === true;
    const ms = Math.max(0, Math.min(2000, this.person.focus_follows_mouse_delay_ms ?? 150));
    const delay = (d, cx) =>
      this.change((q) => {
        const next = Math.max(0, Math.min(2000, Math.round(((q.focus_follows_mouse_delay_ms ?? 150) + d) / 50) * 50));
        if (next === 150) delete q.focus_follows_mouse_delay_ms;
        else q.focus_follows_mouse_delay_ms = next;
      }, cx);
    return v_flex()
      .w_full()
      .child(this.heading(t, "Mouse"))
      .child(
        this.row(
          t,
          "Focus follows mouse",
          "Resting the pointer on a terminal pane focuses it. Keys you press keep their pane until the pointer moves.",
          this.toggle(t, "st-focus_follows_mouse", on, (_e, cx) =>
            this.change((q) => {
              if (q.focus_follows_mouse) delete q.focus_follows_mouse;
              else q.focus_follows_mouse = true;
            }, cx),
          ),
          "focus_follows_mouse",
        ),
      )
      .when(on, (el) =>
        el.child(this.row(t, "Focus delay", "How long the pointer rests on a pane first.", this.stepper(t, "st-focus_follows_mouse_delay_ms", `${ms} ms`, (_e, cx) => delay(-50, cx), (_e, cx) => delay(50, cx)), "focus_follows_mouse_delay_ms")),
      );
  }

  keyboard(t) {
    const choices = [
      ["auto", "Automatic (Alt on US keyboards)"],
      ["on", "Alt"],
      ["off", "Types characters"],
      ["left", "Left Option is Alt"],
      ["right", "Right Option is Alt"],
    ];
    return v_flex()
      .w_full()
      .child(this.heading(t, "Option key"))
      .child(this.plain(div().pb(12).text_size(13).text_color(t.dim).child("Use Option for terminal shortcuts or to type accents and special characters.")))
      .child(this.block("Option key", "Use Option for terminal shortcuts (Alt) or to type accents and special characters. Keyboard.", "option_as_alt", v_flex().w_full().children(choices.map(([value, label]) => {
        const on = (this.person.option_as_alt ?? "auto") === value;
        return h_flex()
          .id(`st-option-${value}`)
          .gap(10)
          .px(12)
          .py(10)
          .rounded(7)
          .items_center()
          .cursor_pointer()
          .hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => this.change((p) => { p.option_as_alt = value; }, cx))
          .child(div().text_color(on ? t.accent : t.muted).child(on ? "◉" : "○"))
          .child(div().text_size(14).text_color(t.foreground).child(label));
      }))));
  }

  // Machines -------------------------------------------------------------

  async loadMachines() {
    if (this.machineLoading) return;
    this.machineLoading = true;
    try {
      const r = await hc(["machine", "ls", "--json"]);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Couldn't read machines.");
      const list = JSON.parse(r.stdout).machines;
      if (!Array.isArray(list)) throw new Error("Couldn't read machines.");
      this.machineList = list.filter((m) => !m.temporary);
      this.machineError = null;
    } catch (e) {
      this.machineError = String(e);
    } finally {
      this.machineLoading = false;
    }
  }

  machineAction(machine, action, cx, name) {
    if (this.machineBusy[machine]) return;
    if (action === "rename" && (!name?.trim() || name.trim() === machine)) return;
    this.machineBusy[machine] = action;
    delete this.machineResults[machine];
    this.machineEdit = null;
    this.handle.focus();
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const args = ["machine", action, machine];
        if (action === "rename") args.push(name.trim());
        const r = await hc(args);
        let payload = null;
        try { payload = JSON.parse(r.stdout || r.stderr); } catch {}
        if (r.code !== 0) throw new Error(payload?.error?.message || (r.stderr || r.stdout || "It didn't work.").trim());
        const message = action === "update"
          ? `Updated ${machine}${payload?.version ? ` to hc ${payload.version}` : ""}; its terminals kept running.`
          : action === "rename" ? `Renamed ${machine} to ${name.trim()}.` : `Removed ${machine}; its terminals keep running there.`;
        if (action === "update") this.machineResults[machine] = { message, ok: true };
        else this.machineNotice = message;
      } catch (e) {
        this.machineResults[machine] = { message: String(e), ok: false };
      } finally {
        delete this.machineBusy[machine];
      }
      await this.loadMachines();
      cx.notify();
    });
  }

  olderCore(remote, local) {
    const parse = (v) => /^(\d+)\.(\d+)\.(\d+)(?:-([^+]+))?(?:\+.*)?$/.exec(v ?? "");
    const a = parse(remote), b = parse(local);
    if (!a || !b) return false;
    for (let i = 1; i <= 3; i++) {
      if (+a[i] !== +b[i]) return +a[i] < +b[i];
    }
    if (!a[4] || !b[4]) return Boolean(a[4] && !b[4]);
    const x = a[4].split("."), y = b[4].split(".");
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      if (x[i] === y[i]) continue;
      if (x[i] === undefined || y[i] === undefined) return x[i] === undefined;
      const xn = /^\d+$/.test(x[i]), yn = /^\d+$/.test(y[i]);
      return xn && yn ? +x[i] < +y[i] : xn !== yn ? xn : x[i] < y[i];
    }
    return false;
  }

  machineRow(t, m, localVersion) {
    const busy = this.machineBusy[m.name];
    const edit = this.machineEdit?.machine === m.name ? this.machineEdit : null;
    const state = String(m.state ?? "unknown").replace(/[_-]/g, " ");
    const color = ["connected", "online"].includes(m.state) ? t.ansi2 : ["connecting", "reconnecting"].includes(m.state) ? t.working : t.blocked;
    const result = this.machineResults[m.name];
    const controls = h_flex().gap(6).items_center();
    if (busy) controls.child(svg("mods/workspace-chrome/spinner.svg").size(12).text_color(t.working).spin(900)).child(div().text_size(12).text_color(t.muted).child(busy === "update" ? "Updating…" : busy === "rename" ? "Renaming…" : "Removing…"));
    else {
      if (this.olderCore(m.version, localVersion)) controls.child(this.button(t, `st-m-update-${m.name}`, "Update", (_e, cx) => this.machineAction(m.name, "update", cx)));
      for (const [action, label] of [["rename", "Rename"], ["rm", "Remove"]]) {
        controls.child(this.button(t, `st-m-${action}-${m.name}`, label, (_e, cx) => {
          this.machineEdit = { machine: m.name, action, name: m.name };
          if (action === "rename") {
            this.renameInput.set_value(m.name);
            this.renameInput.set_selected_range({ start: 0, end: encodeURIComponent(m.name).replace(/%[0-9A-F]{2}/g, "_").length });
            this.renameFocused = true;
            this.renameInput.focus();
          }
          cx.notify();
        }));
      }
    }
    const row = v_flex().w_full().py(12).gap(8).border_b(1).border_color(t.border)
      .child(h_flex().gap(10).items_center()
        .child(div().w(7).h(7).flex_none().rounded(4).bg(color))
        .child(v_flex().flex_1().min_w(0).gap(3)
          .child(div().text_size(14).text_color(t.foreground).child(m.name))
          .child(div().text_size(12.5).text_color(t.dim).child(`${m.target ?? ""} · ${state} · ${m.version ? `hc ${m.version}` : "hc version unknown"}`)))
        .when(m.keeps_running_after_logout === false, el => el.child(div().id(`st-m-logout-${m.name}`)
          .text_color(t.blocked).cursor_pointer().tooltip("Agents stop when you disconnect. Click for the fix.")
          .on_click(() => run("mod.open", { name: "machine-add" })).child("⚠")))
        .child(controls.flex_none()));
    if (m.state === "needs_login") {
      const command = `hc machine login ${m.name}`;
      row.child(v_flex().gap(5)
        .child(div().text_size(12.5).text_color(t.blocked).child("needs a login: run"))
        .child(h_flex().gap(8).items_center()
          .child(div().flex_1().min_w(0).text_size(12.5).text_color(t.foreground).child(command))
          .child(this.button(t, `st-m-login-copy-${m.name}`, this.copiedLogin === command ? "Copied" : "Copy", (_e, cx) => {
            cx.write_to_clipboard(command);
            this.copiedLogin = command;
            cx.notify();
          }))));
    }
    if (busy === "update") row.child(div().text_size(12.5).text_color(t.dim).child("Copying this computer's herdr-core. Its terminals keep running."));
    if (edit && !busy) {
      const cancel = this.button(t, `st-m-cancel-${m.name}`, "Cancel", (_e, cx) => { this.machineEdit = null; this.handle.focus(); cx.notify(); });
      if (edit.action === "rm") row.child(v_flex().gap(8)
        .child(div().text_size(12.5).text_color(t.muted).child(`Remove ${m.name}? Its terminals keep running there.`))
        .child(h_flex().gap(6).child(cancel).child(this.button(t, `st-m-confirm-${m.name}`, "Remove machine", (_e, cx) => this.machineAction(m.name, "rm", cx)))));
      else row.child(v_flex().gap(6)
        .child(div().text_size(12).text_color(t.dim).child("New name (addresses using the old name stop working)"))
        .child(h_flex().gap(6).items_center()
          .child(Input.new(this.renameInput).flex_1().min_w(0).h(32).px(10).rounded(7).border_1().border_color(this.renameFocused ? t.accent : t.input).bg(t.background).text_size(13).text_color(t.foreground))
          .child(cancel).child(this.button(t, `st-m-save-${m.name}`, "Save", (_e, cx) => this.machineAction(m.name, "rename", cx, edit.name)))));
    }
    if (result) row.child(div().text_size(12.5).text_color(result.ok ? t.ansi2 : t.blocked).child(result.message));
    return row;
  }

  machines(t) {
    const version = updates().core.version;
    return v_flex().w_full()
      .child(this.heading(t, "Machines"))
      .child(this.plain(div().pb(8).text_size(12.5).line_height("18px").text_color(t.dim).child("Your computers, reached over ssh. Their terminals keep running when you disconnect.")))
      .child(this.row(t, "This computer", `Connected · ${version ? `hc ${version}${forkTag(updates().core.build)}` : "hc version unknown"}`, div().w(7).h(7).rounded(4).bg(t.ansi2)))
      .children((this.machineList ?? []).map((m) => this.block(m.name, `${m.target ?? ""} machine ssh`, null, this.machineRow(t, m, version))))
      .when(this.machineList === null && !this.machineError && !this.filtering(), (el) => el.child(h_flex().py(14).gap(8).items_center().child(svg("mods/workspace-chrome/spinner.svg").size(12).text_color(t.working).spin(900)).child(div().text_size(12.5).text_color(t.dim).child("Reading machines…"))))
      .when(this.machineList?.length === 0 && !this.filtering(), (el) => el.child(div().py(14).text_size(12.5).text_color(t.dim).child("No other machines yet.")))
      .when(Boolean(this.machineError) && !this.filtering(), (el) => el.child(div().py(10).text_size(12.5).text_color(t.blocked).child(this.machineError)))
      .when(Boolean(this.machineNotice) && !this.filtering(), (el) => el.child(div().py(10).text_size(12.5).text_color(t.ansi2).child(this.machineNotice)))
      .child(this.block("Add a machine", "Another computer, reached over ssh.", null, h_flex().pt(16).child(this.button(t, "st-m-add", "Add a machine", () => run("mod.open", { name: "machine-add" })))));
  }

  // Alerts ---------------------------------------------------------------

  alerts(t) {
    const sounds = this.person.sounds ?? {};
    const on = sounds.enabled !== false;
    const file = (f) => (f ? f.split(/[\\/]/).pop() : "Herdr's sound");
    return v_flex()
      .w_full()
      .child(this.heading(t, "Sounds"))
      .child(
        this.row(
          t,
          "Play sounds",
          "When an agent needs you, and when one finishes while you're not looking. A note shows in the corner either way.",
          this.toggle(t, "st-sounds", on, (_e, cx) => this.change((p) => (p.sounds = { ...(p.sounds ?? {}), enabled: !on }), cx)),
          "sounds.enabled",
        ),
      )
      .child(this.row(t, "An agent needs you", file(sounds.blocked), this.button(t, "st-play-blocked", "Play", () => playSound("blocked")), "sounds.blocked"))
      .child(this.row(t, "An agent finished", file(sounds.done), this.button(t, "st-play-done", "Play", () => playSound("done")), "sounds.done"))
      .child(this.plain(div().pt(12).text_size(12.5).line_height("18px").text_color(t.dim).child("Want your own sounds? Ask your agent to set sounds.done and sounds.blocked in settings.json to an MP3.")))
      .child(this.heading(t, "Notifications"))
      .child(
        this.row(
          t,
          "System notifications",
          this.person.alert_command
            ? "Your alert_command runs instead of a system notification."
            : "When away: only while Herdr isn't the app in front. Never for the agent you're looking at.",
          this.choicePicker(t, "st-sn", "system_notifications", SYSTEM_NOTIFICATIONS),
          ["system_notifications", "alert_command"],
        ),
      )
      .child(
        this.row(
          t,
          "Notification sound",
          soundsKind(this.person.notification_sounds) === "state" && typeof this.person.notification_sounds === "object"
            ? `By state: ${["done", "blocked", "error"].map((k) => `${k} ${this.person.notification_sounds[k] ?? "none"}`).join(", ")}.`
            : soundsKind(this.person.notification_sounds) === "named"
              ? `${this.person.notification_sounds} for every notification.`
              : "Played by the system notification. By state: Glass when done, Purr when blocked, Basso on an error.",
          this.choicePicker(t, "st-ns", "notification_sounds", NOTIFICATION_SOUNDS, {
            now: soundsKind(this.person.notification_sounds),
            write: (p, value) => {
              if (value === "none") delete p.notification_sounds;
              else if (value === "default") p.notification_sounds = "default";
              else if (soundsKind(p.notification_sounds) !== "state") p.notification_sounds = { ...SOUNDS_BY_STATE };
            },
          }),
          "notification_sounds",
        ),
      )
      .child(
        this.row(
          t,
          "Marks",
          "The agent's glyph before the title (🤖 Claude) and the state's before the text (✋ ✅ 🔴).",
          this.toggle(t, "st-notification_marks", this.person.notification_marks === true, (_e, cx) =>
            this.change((p) => {
              if (p.notification_marks) delete p.notification_marks;
              else p.notification_marks = true;
            }, cx),
          ),
          "notification_marks",
        ),
      )
      .child(
        this.row(
          t,
          "Agent icons",
          "The agent's program icon on its notification.",
          this.toggle(t, "st-notification_icons", this.person.notification_icons !== false, (_e, cx) =>
            this.change((p) => {
              if (p.notification_icons === false) delete p.notification_icons;
              else p.notification_icons = false;
            }, cx),
          ),
          "notification_icons",
        ),
      )
      .child(
        this.row(
          t,
          "Bounce the Dock icon",
          "When an agent needs you while Herdr is in the background.",
          this.toggle(t, "st-dock_bounce", this.person.dock_bounce === true, (_e, cx) =>
            this.change((p) => {
              if (p.dock_bounce) delete p.dock_bounce;
              else p.dock_bounce = true;
            }, cx),
          ),
          "dock_bounce",
        ),
      )
      .child(
        this.row(
          t,
          "Count waiting agents on the Dock icon",
          "Agents that need you or finished while you weren't looking, as a badge.",
          this.toggle(t, "st-dock_badge", this.person.dock_badge !== false, (_e, cx) =>
            this.change((p) => {
              if (p.dock_badge === false) delete p.dock_badge;
              else p.dock_badge = false;
            }, cx),
          ),
          "dock_badge",
        ),
      );
  }

  // Updates --------------------------------------------------------------

  updates(t) {
    const u = updates();
    const at = (secs) => {
      const d = new Date(secs * 1000);
      return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    };
    const checked = u.checked ? ` Checked at ${at(u.checked)}.` : "";
    const result = u.checking ? "Checking\u2026" : u.checked
      ? `Up to date.${checked}`
      : "No successful check. Check your connection and try again.";
    // Pressed: "Checking\u2026" while it looks, then "Up to date" for a moment
    // (a find changes the row instead). One per row: an element shows once.
    const asked = this.asked && Date.now() - this.asked < 3000;
    const check = (id) =>
      !u.checks
        ? null
        : u.checking && this.asked
          ? div().px(12).py(5).text_size(13).text_color(t.dim).child("Checking\u2026")
          : asked && u.checked && !u.checking
            ? div().px(12).py(5).text_size(13).text_color(t.ansi2).child("\u2713 Up to date")
            : this.button(t, id, "Check now", (_e, cx) => {
                this.asked = Date.now();
                run("app.check_updates");
                cx.notify();
              });
    const primary = (id, label, f) =>
      div().id(id).px(12).py(5).rounded(7).cursor_pointer().bg(t.accent).text_color(t.accent_foreground).text_size(13).font_semibold().hover((s) => s.opacity(0.9)).on_click(f).child(label);
    const app = !u.app.updates
      ? ["A build from source: it updates with git, not here.", null]
      : u.app.ready
        ? [`${u.app.ready} is ready. Restart to switch now; it also switches when you quit.`, primary("st-app-update", "Restart to update", () => run("app.update"))]
        : [result, check("st-check-app")];
    const c = u.core;
    const core = c.managed
      ? [`Managed by ${c.managed}: update it there.${c.ready ? ` ${c.ready} is available.` : ""}`, u.checks ? check("st-check-core") : null]
      : c.updating
        ? ["Updating\u2026 your terminals keep running.", null]
        : c.error
          ? [`The update failed: ${c.error}`, primary("st-core-retry", "Try again", () => run("app.update_core"))]
          : c.ready
            ? [`${c.ready} is ready. Your terminals keep running through it.`, primary("st-core-update", "Update herdr-core", () => run("app.update_core"))]
            : u.checks
              ? [result, check("st-check-core")]
              : ["Not checked: a build from source looks for no updates.", null];
    return v_flex()
      .w_full()
      .child(this.heading(t, "Herdr"))
      .child(this.row(t, `Herdr ${u.app.version}${forkTag(u.app.build)}`, app[0], h_flex().gap(8).flex_none()
        .child(this.button(t, "st-whats-new", "What's new", () => { closeOverlay(props()?.overlay); run("app.whats_new"); }))
        .when(Boolean(app[1]), (el) => el.child(app[1])), "update"))
      .child(this.plain(div().h(18)))
      .child(this.heading(t, "herdr-core"))
      .child(this.row(t, `herdr-core ${c.version ?? ""}${forkTag(c.build)}`, core[0], core[1] ?? div(), "update"))
      .child(this.plain(div().pt(12).text_size(12.5).line_height("18px").text_color(t.dim).child("Herdr looks for both every hour and tells you in the corner. herdr-core runs your terminals, so they keep running through its updates.")));
  }


  // Privacy --------------------------------------------------------------

  // A report as it goes out, one line per line of JSON.
  report(t, id, title, about, report) {
    const lines = JSON.stringify(report, null, 2).split("\n");
    return v_flex()
      .w_full()
      .pt(14)
      .gap(4)
      .child(div().text_size(13.5).text_color(t.foreground).child(title))
      .when(Boolean(about), (el) => el.child(div().text_size(12.5).text_color(t.dim).child(about)))
      .child(
        v_flex()
          .id(id)
          .mt(4)
          .w_full()
          .p(10)
          .rounded(8)
          .border_1()
          .border_color(t.border)
          .bg(t.background)
          .children(lines.map((l) => div().font_family(DEFAULT_FONT).text_size(12).line_height("17px").text_color(t.muted).whitespace_nowrap().child(l || " "))),
      );
  }

  privacy(t) {
    const s = telemetry();
    const on = s ? s.setting : this.person.telemetry !== false;
    const at = (secs) => new Date(secs * 1000).toLocaleString();
    const why = s?.off_because ? `Off for now: ${s.off_because} is set where Herdr started.` : null;
    const body = v_flex()
      .w_full()
      .child(this.heading(t, "Usage data"))
      .child(
        this.row(
          t,
          "Share anonymous usage data",
          why ?? "Once a day: which agents and features you use, how many spaces and tabs, your system and Herdr's version. Never what's in your terminals, never names of files, folders or projects.",
          this.toggle(t, "st-telemetry", on, (_e, cx) =>
            this.change((p) => {
              if (on) p.telemetry = false;
              else delete p.telemetry;
            }, cx),
          ),
          "telemetry",
        ),
      )
      .child(
        this.row(
          t,
          "What we send",
          s && !s.sends ? "This build never sends: it's built from source. You can still see what a release would send." : "Exactly what leaves your computer, word for word.",
          this.button(t, "st-telemetry-show", this.showData ? "Hide" : "Show", (_e, cx) => {
            this.showData = !this.showData;
            cx.notify();
          }),
        ),
      );
    if (!this.showData || this.filtering()) return body;
    if (!s?.enabled) return body.child(div().pt(14).text_size(13).text_color(t.dim).child("Nothing is collected or sent while it's off."));
    return body
      .children(s.queued.map((r, i) => this.report(t, `st-tq-${i}`, `Waiting to send: ${r.day}`, "Sent as is, soon.", r)))
      .when(Boolean(s.draft), (el) => el.child(this.report(t, "st-td", "Today so far", "A draft: it changes until the day ends (UTC), then goes out once.", s.draft)))
      .when(Boolean(s.last_sent), (el) => el.child(this.report(t, "st-tl", `Last sent: ${s.last_sent.report.day}`, `Sent ${at(s.last_sent.at)}.`, s.last_sent.report)));
  }

  // cmd-f (ctrl-f) and typing start a search; esc clears it, then leaves the
  // field, then closes what's open.
  onKey(e, cx) {
    const k = e.key;
    const m = e.modifiers ?? {};
    if (k === "f" && (window()?.platform === "macos" ? m.platform : m.control) && !m.alt && !m.shift) {
      this.searchInput.focus();
      this.searchInput.set_selected_range({ start: 0, end: encodeURIComponent(this.query).replace(/%[0-9A-F]{2}/g, "_").length });
      cx.stop_propagation();
      cx.notify();
      return;
    }
    if (this.searchFocused || this.query) {
      if (k !== "escape") {
        if (!this.typesIntoSearch(e)) return;
        this.searchInput.focus();
        this.setQuery(this.query + e.key_char);
      } else if (this.query) {
        this.setQuery("");
        // A font list or rename opened in the results isn't on the page now shown.
        if (this.picking || this.machineEdit) {
          this.picking = null;
          this.machineEdit = null;
          this.handle.focus();
        }
      } else this.handle.focus();
      cx.stop_propagation();
      cx.notify();
      return;
    }
    // Keys typed into the theme filter are the filter's; esc leaves it.
    if (this.themeInputFocused) {
      if (k !== "escape") return;
      this.handle.focus();
      cx.stop_propagation();
      cx.notify();
      return;
    }
    // Arrows walk the sidebar: up and down to the page before or after (its
    // group opens), left and right close and open the shown page's group.
    const bare = !m.platform && !m.control && !m.alt && !m.shift && !m.function;
    if (bare && ["up", "down", "left", "right"].includes(k) && !this.picking && !this.renameFocused && !this.machineEdit) {
      const g = groupOf(this.section);
      if (k === "left" || k === "right") {
        if (this.open.has(g) !== (k === "right")) this.toggleGroup(g);
      } else {
        const i = PAGES.findIndex((p) => p.id === this.section) + (k === "down" ? 1 : -1);
        this.go(PAGES[Math.max(0, Math.min(PAGES.length - 1, i))].id, cx);
      }
      cx.stop_propagation();
      cx.notify();
      return;
    }
    if (this.typesIntoSearch(e)) {
      this.searchInput.focus();
      this.setQuery(e.key_char);
      cx.stop_propagation();
      cx.notify();
      return;
    }
    if (this.machineEdit) {
      if (k === "escape") this.machineEdit = null;
      else return;
      this.handle.focus();
      cx.stop_propagation();
      cx.notify();
      return;
    }
    if (k === "escape") {
      if (this.picking) this.picking = null;
      else return this.close();
      this.handle.focus();
    } else return;
    cx.stop_propagation();
    cx.notify();
  }

  // ----------------------------------------------------------------------

  // The sidebar's groups and pages. While searching, a group with a match is
  // open (what's remembered stays as it was) and each page says how many match.
  sidebar(t, waiting) {
    const counts = this.words.length && this.index && !this.indexing ? this.match().pages : null;
    const sel = tonal(t, window()?.active !== false);
    const badge = (n) => div().px(6).rounded(8).text_size(11).text_color(sel.text).bg(sel.fill).child(`${n}`);
    const dot = () => div().w(7).h(7).rounded(4).bg(t.accent);
    const pill = (el, on) => el.rounded(8).cursor_pointer().when(on, (e) => e.bg(sel.fill)).when(!on, (e) => e.hover((st) => st.bg(hoverFill(t))));
    return v_flex()
      .id("st-nav")
      .flex_1()
      .min_h(0)
      .gap(1)
      .overflow_y_scrollbar()
      .children(
        GROUPS.map((g) => {
          const found = counts ? g.pages.reduce((n, [id]) => n + (counts.get(id) ?? 0), 0) : 0;
          const open = this.open.has(g.id) || found > 0;
          return v_flex()
            .gap(1)
            .child(
              pill(h_flex().id(`st-g-${g.id}`).gap(8).px(8).py(6).items_center(), false)
                // While searching, groups show where the matches are; what's kept waits.
                .on_click((_e, cx) => {
                  if (counts) return;
                  this.toggleGroup(g.id);
                  cx.notify();
                })
                .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none().text_color(t.dim))
                .child(svg(I(g.icon)).size(15).flex_none().text_color(t.dim))
                .child(div().flex_1().text_size(13.5).text_color(open ? t.foreground : t.muted).child(g.title))
                .when(found > 0, (el) => el.child(badge(found)))
                .when(!open && waiting && g.pages.some(([id]) => id === "updates"), (el) => el.child(dot())),
            )
            .when(open, (el) =>
              el.children(
                g.pages.map(([id, title]) => {
                  // While searching no page is the one shown.
                  const on = id === this.section && !this.words.length;
                  const n = counts?.get(id) ?? 0;
                  return pill(h_flex().id(`st-s-${id}`).pl(41).pr(8).py(5).items_center(), on)
                    .on_click((_e, cx) => this.go(id, cx))
                    .child(div().flex_1().text_size(13).text_color(on ? sel.text : counts && !n ? t.dim : t.muted).when(on, (e) => e.font_semibold()).child(title))
                    .when(n > 0, (e) => e.child(badge(n)))
                    .when(id === "updates" && waiting, (e) => e.child(dot()));
                }),
              ),
            );
        }),
      );
  }

  render(cx) {
    const t = theme();
    const u = updates();
    const waiting = Boolean(u.app.ready || (u.core.ready && !u.core.managed));
    // The page first: while searching it builds the index the sidebar counts from.
    const page = this.words.length ? this.results(t, cx) : ((this.place = { page: this.section, heading: "" }), this.page(this.section, t, cx));
    const nav = v_flex()
      .flex_none()
      .w(200)
      .p(10)
      .gap(2)
      .border_r(1)
      .border_color(t.border)
      .child(div().px(10).pt(6).pb(12).text_size(16).font_semibold().text_color(t.foreground).child("Settings"))
      .child(
        h_flex()
          .id("st-search")
          .mb(8)
          .h(30)
          .px(8)
          .gap(6)
          .items_center()
          .rounded(7)
          .border_1()
          .border_color(this.searchFocused ? t.accent : t.border)
          .bg(t.background)
          .child(svg("mods/workspace-chrome/search.svg").size(13).flex_none().text_color(t.dim))
          .child(Input.new(this.searchInput).flex_1().min_w(0).h(28).text_size(13).text_color(t.foreground)),
      )
      .child(this.sidebar(t, waiting));
    return h_flex()
      .id("settings")
      .track_focus(this.handle)
      .when(Boolean(this.picking), (el) => el.on_action("overlay.close", (_e, cx) => {
        this.picking = null;
        this.handle.focus();
        cx.stop_propagation();
        cx.notify();
      }))
      .w_full()
      .h(620)
      .min_h(0)
      .items_start()
      .on_key_down((e, cx) => this.onKey(e, cx))
      .child(nav.h_full().child(this.button(t, "st-feedback", "Send feedback", () => { closeOverlay(props()?.overlay); run("app.feedback"); })))
      .child(
        // A bar that always shows: the pages are taller than the dialog and
        // people should see that they scroll.
        div()
          .relative()
          .flex_1()
          .min_w(0)
          .h_full()
          .child(
            div()
              .id("st-body")
              .size_full()
              .overflow_y_scroll()
              // Padding inside, not on the scroll area: there it counts as
              // content and a page that fits still shows a bar.
              .child(
                div()
                  .px(28)
                  .py(22)
                  .child(page),
              ),
          )
          .child(Scrollbar.vertical("st-body").mode("always").absolute().inset_0()),
      );
  }
}
