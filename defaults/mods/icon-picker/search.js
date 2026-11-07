// The icon picker's catalogue: every Symbols Nerd Font glyph and every emoji by
// name (names/glyphs.json, names/emoji.json), its sets, and search over them.
// No Herdr imports, so it runs anywhere.

// Nerd Font name prefixes as the picker groups them: [chip, label, prefixes].
export const SETS = [
  ["md", "Material Design", ["md"]],
  ["fa", "Font Awesome", ["fa"]],
  ["fae", "Font Awesome Extension", ["fae"]],
  ["cod", "Codicons", ["cod"]],
  ["oct", "Octicons", ["oct"]],
  ["dev", "Devicons", ["dev"]],
  ["seti", "Seti UI", ["seti"]],
  ["custom", "Custom", ["custom"]],
  ["weather", "Weather", ["weather"]],
  ["linux", "Linux", ["linux"]],
  ["pl", "Powerline", ["pl", "ple"]],
  ["pom", "Pomicons", ["pom"]],
  ["iec", "IEC power", ["iec"]],
  ["other", "Other", ["extra", "indent", "indentation"]],
];
const CHIP = new Map(SETS.flatMap(([chip, , prefixes]) => prefixes.map((p) => [p, chip])));
const ORDER = new Map(SETS.map(([chip], i) => [chip, i]));
export const setLabel = (chip) => SETS.find(([c]) => c === chip)?.[1] ?? chip;

// A query or a name as both are compared: lower case, words joined by "_".
const fold = (s) => s.toLowerCase().trim().replace(/[\s-]+/g, "_");

/**
 * names/glyphs.json's text to entries:
 * { kind: "glyph", text, name ("fa-rocket"), set (chip), keys (folded names), words ("") }.
 * Sorted by set, as the Glyphs tab shows them.
 */
export function parseGlyphs(text) {
  return JSON.parse(text).glyphs.map(([name, code, prefix, others]) => {
    const full = `${prefix}-${name}`;
    const keys = [fold(name), fold(full)];
    for (const o of others ? others.split(" ") : []) keys.push(fold(o), fold(o.slice(o.indexOf("-") + 1)));
    return { kind: "glyph", text: String.fromCodePoint(parseInt(code, 16)), name: full, set: CHIP.get(prefix) ?? "other", keys, words: [] };
  }).sort((a, b) => ORDER.get(a.set) - ORDER.get(b.set));
}

/** names/emoji.json's text to { groups, entries }: entries like glyphs', `set` the group's name. */
export function parseEmoji(text) {
  const { groups, emoji } = JSON.parse(text);
  return {
    groups,
    entries: emoji.map(([text, name, tags, group]) => ({ kind: "emoji", text, name, set: groups[group], keys: [fold(name)], words: tags ? tags.split(" ") : [] })),
  };
}

// How well an entry matches a folded query: 0 a name is it, 1 a name starts
// with it (or a keyword is it), 2 a name holds it (or a keyword starts with it),
// -1 not at all.
function rank(e, q) {
  let best = -1;
  for (const k of e.keys) {
    if (k === q) return 0;
    const at = k.indexOf(q);
    const r = at === 0 ? 1 : at > 0 ? 2 : -1;
    if (r >= 0 && (best < 0 || r < best)) best = r;
  }
  if (best === 1) return 1;
  for (const w of e.words) {
    if (w === q) return 1;
    if (best < 0 && w.startsWith(q)) best = 2;
  }
  return best;
}

/**
 * The entries matching `query`, best first: exact names, then names starting
 * with it, then names holding it; shorter names first within each. `set`
 * keeps one set (a glyph chip or an emoji group). An empty query keeps all, in order.
 */
export function search(entries, query, set = null) {
  const pool = set ? entries.filter((e) => e.set === set) : entries;
  const q = fold(query ?? "");
  if (!q) return pool;
  const tiers = [[], [], []];
  for (const e of pool) {
    const r = rank(e, q);
    if (r >= 0) tiers[r].push(e);
  }
  const byLength = (a, b) => a.name.length - b.name.length;
  return tiers.flatMap((t) => t.sort(byLength));
}

/** Entries as rows of `perRow`, with a heading row before each set when `headed`. */
export function rows(entries, perRow, headed) {
  const out = [];
  let i = 0;
  while (i < entries.length) {
    const set = entries[i].set;
    if (headed) out.push({ head: set });
    let j = i;
    while (j < entries.length && (!headed || entries[j].set === set)) j++;
    for (let k = i; k < j; k += perRow) out.push({ cells: entries.slice(k, Math.min(k + perRow, j)), first: k });
    i = j;
  }
  return out;
}
