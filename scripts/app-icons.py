"""The app icon variants (settings.json `app_icon`), all drawn from Herdr's own
artwork (assets/icons/herdr-ui-icon-clean.svg, whose attribution they keep).

  python3 scripts/app-icons.py          writes assets/icons/variants/<name>.svg
                                        and .icns, and the Settings previews
  python3 scripts/app-icons.py --check  fails if the checked-in SVGs differ
                                        from what this script draws

"theme" is drawn by Herdr itself, from assets/icons/variants/theme.svg and
theme-small.svg: templates whose {tile_top}-style fields Herdr fills with
colours from the active theme (src/app_icon.rs), on the same grid. Its
Settings preview is written by Herdr's tests (src/app_icon.rs).

Writing needs rsvg-convert (librsvg) and macOS's iconutil and sips; checking
needs none of them. "classic" is assets/icons/Herdr.icns itself. Sizes up to
32 px are drawn without the artwork's glows, shadows, sheens and scan lines,
which blur a small icon into its tile.

Every variant sits on macOS's icon grid, as the system draws other apps' icons
(measured from macOS 27's own renderings of other apps' icons): an 824 px tile with
100 px margins on the 1024 canvas, corners that are superellipse quarters
(radius 256, exponent 2.5) rather than circular arcs, and the system's drop
shadow (black at 25%, blur 16, 8 down) in the margin. The Dock shows the image
Herdr hands it as it is, so the tile and shadow have to be in the image. The
artwork itself is drawn on its own 896 px tile and scaled onto the grid.
"""

import math
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets/icons/herdr-ui-icon-clean.svg"
CLASSIC = ROOT / "assets/icons/Herdr.icns"
VARIANTS = ROOT / "assets/icons/variants"
PREVIEWS = ROOT / "defaults/frame/settings/app-icons"
PREVIEW_PX = 96
SMALL_PX = 32

CANVAS = 1024
GRID_TILE = 824
CORNER_RADIUS = 256
CORNER_EXPONENT = 2.5
CORNER_STEPS = 24
# A blurred copy of the tile rather than feDropShadow or feMerge, which some
# renderers (older librsvg, QtSvg, AppKit's CoreSVG) lack.
SHADOW_OPACITY, SHADOW_BLUR, SHADOW_DY = ".25", 16, 8
ART_TILE = 896
ART_SCALE = GRID_TILE / ART_TILE
RAM_T = "translate(-80 967) scale(.22 -.22)"


def tile_path():
    """The grid's tile: a square with superellipse corners, as an SVG path."""
    lo, hi, r, n = (CANVAS - GRID_TILE) / 2, (CANVAS + GRID_TILE) / 2, CORNER_RADIUS, CORNER_EXPONENT
    points = []
    # Clockwise from the top edge; each corner's angle runs edge to edge.
    for i, (cx, cy, sx, sy) in enumerate(((hi - r, lo + r, 1, -1), (hi - r, hi - r, 1, 1), (lo + r, hi - r, -1, 1), (lo + r, lo + r, -1, -1))):
        for k in range(CORNER_STEPS + 1):
            t = (CORNER_STEPS - k if i % 2 == 0 else k) / CORNER_STEPS * math.pi / 2
            points.append((cx + sx * r * math.cos(t) ** (2 / n), cy + sy * r * math.sin(t) ** (2 / n)))
    return "M" + " L".join(f"{round(x, 2):g} {round(y, 2):g}" for x, y in points) + "Z"


def svg(src, title, defs, fill, inner, ram, overlay=""):
    """A variant: the tile filled with `fill`, then `inner` (an inset stroke or a
    sheen), the artwork and `overlay`, all clipped to the tile."""
    meta = re.search(r"<metadata>.*?</metadata>", src, re.S).group(0)
    path = re.search(r'<path id="ram" d="([^"]+)"', src).group(1)
    offset = round(CANVAS / 2 * (1 - ART_SCALE), 3)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{CANVAS}" height="{CANVAS}" viewBox="0 0 {CANVAS} {CANVAS}">
  <title>Herdr – {title}</title>
  {meta}
  <defs>
    <path id="tile" d="{tile_path()}"/>
    <clipPath id="clip"><use href="#tile"/></clipPath>
    <filter id="drop" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur stdDeviation="{SHADOW_BLUR}"/></filter>
    <path id="ram" d="{path}"/>
    {defs}
  </defs>
  <use href="#tile" transform="translate(0 {SHADOW_DY})" fill="#000" fill-opacity="{SHADOW_OPACITY}" filter="url(#drop)"/>
  <use href="#tile" fill="{fill}"/>
  <g clip-path="url(#clip)">{inner}<g transform="translate({offset:g} {offset:g}) scale({round(ART_SCALE, 6):g})">{ram}</g>{overlay}</g>
</svg>
'''


def inset(color, opacity, width):
    """A line `width` px wide just inside the tile's edge (half of a stroke
    on the edge, the clip cutting the other half)."""
    return f'<use href="#tile" fill="none" stroke="{color}" stroke-opacity="{opacity}" stroke-width="{2 * width:g}"/>'


def use(fill, extra=""):
    return f'<use href="#ram" transform="{RAM_T}" fill="{fill}" {extra}/>'.replace(" />", "/>")


def variants(src, small=False):
    """name -> SVG text; `small` leaves out the effects."""
    return {
        "midnight": svg(src, "Midnight",
            '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2a2f38"/><stop offset="1" stop-color="#15181d"/></linearGradient>',
            "url(#g)", inset("#ffffff", ".08", 2),
            use("#f4f1e9")),
        "mirage": svg(src, "Mirage",
            '<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2a3142"/><stop offset="1" stop-color="#1a1f29"/></linearGradient>'
            '<linearGradient id="r" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffd580"/><stop offset="1" stop-color="#f29e74"/></linearGradient>',
            "url(#g)", "",
            use("url(#r)")),
        "glass": svg(src, "Glass",
            '<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#7fd3e8"/><stop offset=".55" stop-color="#4a8fd9"/><stop offset="1" stop-color="#5b5bd6"/></linearGradient>'
            '<linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".45"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient>'
            '<filter id="sh" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="14" stdDeviation="18" flood-color="#0b1f4a" flood-opacity=".45"/></filter>',
            "url(#g)", "" if small else '<use href="#tile" fill="url(#sheen)"/>' + inset("#fff", ".5", 4),
            use("#ffffff", 'fill-opacity=".94"' + ("" if small else ' filter="url(#sh)"'))),
        "phosphor": svg(src, "Phosphor",
            '<radialGradient id="g" cx=".5" cy=".45" r=".75"><stop offset="0" stop-color="#0f2a17"/><stop offset="1" stop-color="#050b07"/></radialGradient>'
            '<filter id="glow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>'
            '<pattern id="scan" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="8" height="3" fill="#000" fill-opacity=".2"/></pattern>',
            "url(#g)", "",
            use("#3dff7a", "" if small else 'filter="url(#glow)"'),
            "" if small else '<use href="#tile" fill="url(#scan)"/>'),
        "graphite": svg(src, "Graphite",
            '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#d9dadc"/><stop offset="1" stop-color="#a9abaf"/></linearGradient>',
            "url(#g)", inset("#fff", ".6", 3),
            use("#3a3d42")),
    }


THEME_FIELDS = ("tile_top", "tile_bottom", "edge", "edge_opacity", "glyph_from", "glyph_to")


def theme_template(src, small=False):
    """The "theme" icon with its colours left as {field}s (THEME_FIELDS): the
    tile a vertical gradient, an inset edge, and the artwork a diagonal
    gradient lifted by a soft shadow, which `small` leaves out."""
    soft = "" if small else '<filter id="lift" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="14"/></filter>'
    lift = "" if small else use("#000", 'fill-opacity=".28" filter="url(#lift)"').replace(f'transform="{RAM_T}"', f'transform="translate(0 12) {RAM_T}"')
    return svg(src, "Theme",
        '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{tile_top}"/><stop offset="1" stop-color="{tile_bottom}"/></linearGradient>'
        '<linearGradient id="r" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{glyph_from}"/><stop offset="1" stop-color="{glyph_to}"/></linearGradient>' + soft,
        "url(#g)", inset("{edge}", "{edge_opacity}", 3),
        lift + use("url(#r)"))


def render(svg_path, px, out):
    subprocess.run(["rsvg-convert", "-w", str(px), "-h", str(px), str(svg_path), "-o", str(out)], check=True)


def icns(full_svg, small_svg, out):
    """An .icns from two drawings: the plain one up to SMALL_PX, the full one above."""
    with tempfile.TemporaryDirectory() as tmp:
        iconset = pathlib.Path(tmp) / "icon.iconset"
        iconset.mkdir()
        for base in (16, 32, 128, 256, 512):
            for mult, suffix in ((1, ""), (2, "@2x")):
                px = base * mult
                render(small_svg if px <= SMALL_PX else full_svg, px, iconset / f"icon_{base}x{base}{suffix}.png")
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out)], check=True)


def main():
    src = SOURCE.read_text()
    full, small = variants(src), variants(src, small=True)
    templates = {"theme": theme_template(src), "theme-small": theme_template(src, small=True)}
    if sys.argv[1:] == ["--check"]:
        stale = [n for n, text in (full | templates).items() if (VARIANTS / f"{n}.svg").read_text() != text]
        if stale:
            sys.exit(f"stale: {', '.join(stale)}; run python3 scripts/app-icons.py")
        return
    for tool in ("rsvg-convert", "iconutil", "sips"):
        if not shutil.which(tool):
            sys.exit(f"{tool} is needed to write the icons")
    VARIANTS.mkdir(parents=True, exist_ok=True)
    PREVIEWS.mkdir(parents=True, exist_ok=True)
    for name, text in templates.items():
        (VARIANTS / f"{name}.svg").write_text(text)
    # Classic's preview is its .icns, which is on the grid, rather than its SVG.
    subprocess.run(["sips", "-s", "format", "png", "-z", str(PREVIEW_PX), str(PREVIEW_PX), str(CLASSIC), "--out", str(PREVIEWS / "classic.png")], check=True, stdout=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory() as tmp:
        for name, text in full.items():
            (VARIANTS / f"{name}.svg").write_text(text)
            plain = pathlib.Path(tmp) / f"{name}-small.svg"
            plain.write_text(small[name])
            icns(VARIANTS / f"{name}.svg", plain, VARIANTS / f"{name}.icns")
            render(VARIANTS / f"{name}.svg", PREVIEW_PX, PREVIEWS / f"{name}.png")


if __name__ == "__main__":
    main()
