// Builds the icon picker's name files from their published sources:
//
//   bun scripts/icon-names.ts <glyphnames.json> <emojibase compact.json>
//
// glyphnames.json: github.com/ryanoasis/nerd-fonts at the tag of the bundled
// Symbols Nerd Font (assets/fonts, `strings` shows "Nerd Fonts <version>"), MIT.
// compact.json: emojibase-data's en/compact.json (MIT; names and keywords from
// Unicode CLDR, Unicode License). Writes defaults/mods/icon-picker/names/.
import { readFileSync, writeFileSync } from "node:fs";

const [glyphPath, emojiPath] = process.argv.slice(2);
if (!glyphPath || !emojiPath) throw new Error("usage: bun scripts/icon-names.ts <glyphnames.json> <compact.json>");
const out = new URL("../defaults/mods/icon-picker/names/", import.meta.url);

// The code points the bundled font has (its cmap, format 4 or 12 subtables).
function fontCodePoints(bytes: Uint8Array): Set<number> {
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const tables = v.getUint16(4);
  let cmap = -1;
  for (let i = 0; i < tables; i++) {
    const at = 12 + i * 16;
    if (String.fromCharCode(...bytes.subarray(at, at + 4)) === "cmap") cmap = v.getUint32(at + 8);
  }
  if (cmap < 0) throw new Error("the font has no cmap");
  const points = new Set<number>();
  for (let i = 0; i < v.getUint16(cmap + 2); i++) {
    const sub = cmap + v.getUint32(cmap + 4 + i * 8 + 4);
    const format = v.getUint16(sub);
    if (format === 12) {
      for (let g = 0; g < v.getUint32(sub + 12); g++) {
        const at = sub + 16 + g * 12;
        for (let c = v.getUint32(at); c <= v.getUint32(at + 4); c++) points.add(c);
      }
    } else if (format === 4) {
      const segs = v.getUint16(sub + 6) / 2;
      for (let s = 0; s < segs; s++) {
        const end = v.getUint16(sub + 14 + s * 2), start = v.getUint16(sub + 16 + segs * 2 + s * 2);
        for (let c = start; c <= end && c !== 0xffff; c++) points.add(c);
      }
    }
  }
  return points;
}

const font = fontCodePoints(readFileSync(new URL("../assets/fonts/SymbolsNerdFont-Regular.ttf", import.meta.url)));
const names = JSON.parse(readFileSync(glyphPath, "utf8"));
const version: string = names.METADATA.version;
delete names.METADATA;
// One row per code point: [name, hex, set, other names?]. The first name a code
// point has is its own; the others (seti- for custom-, a renamed md-) ride along for search.
const byCode = new Map<string, string[]>();
let dropped = 0;
for (const [full, { code }] of Object.entries(names) as [string, { code: string }][]) {
  if (!font.has(parseInt(code, 16))) { dropped++; continue; }
  byCode.set(code, [...(byCode.get(code) ?? []), full.replace(/^nf-/, "")]);
}
const glyphs = [...byCode].map(([code, [first, ...rest]]) => {
  const dash = first.indexOf("-");
  const row = [first.slice(dash + 1), code, first.slice(0, dash)];
  return rest.length ? [...row, rest.join(" ")] : row;
});
writeFileSync(new URL("glyphs.json", out), `{"source":"Nerd Fonts ${version} glyphnames.json (MIT)","glyphs":[\n${glyphs.map((r) => JSON.stringify(r)).join(",\n")}\n]}\n`);

// emojibase groups; 2 (components: skin tones, hair) isn't picked on its own.
const GROUPS: [number, string][] = [[0, "Smileys"], [1, "People"], [3, "Animals & nature"], [4, "Food & drink"], [5, "Travel & places"], [6, "Activities"], [7, "Objects"], [8, "Symbols"], [9, "Flags"]];
const group = new Map(GROUPS.map(([g], i) => [g, i]));
type Emoji = { unicode: string; label: string; tags?: string[]; group?: number; order: number };
const emoji = (JSON.parse(readFileSync(emojiPath, "utf8")) as Emoji[])
  .filter((e) => group.has(e.group ?? -1))
  .sort((a, b) => a.order - b.order)
  .map((e) => {
    const words = new Set(e.label.toLowerCase().split(/[\s:]+/));
    return [e.unicode, e.label, (e.tags ?? []).filter((t) => !words.has(t)).join(" "), group.get(e.group!)];
  });
writeFileSync(new URL("emoji.json", out), `{"source":"emojibase-data en/compact.json (MIT), names and keywords from Unicode CLDR (Unicode License)","groups":${JSON.stringify(GROUPS.map(([, n]) => n))},"emoji":[\n${emoji.map((r) => JSON.stringify(r)).join(",\n")}\n]}\n`);
console.log(`${glyphs.length} glyphs (${dropped} names not in the font), ${emoji.length} emoji`);
