"""The app icon variants (settings.json `app_icon`), all drawn from Herdr's own
artwork (assets/icons/herdr-ui-icon-clean.svg, whose attribution they keep).

  python3 scripts/app-icons.py          writes assets/icons/variants/<name>.svg
                                        and .icns, and the Settings previews
  python3 scripts/app-icons.py --check  fails if the checked-in SVGs differ
                                        from what this script draws

Writing needs rsvg-convert (librsvg) and macOS's iconutil; checking needs
neither. "classic" is assets/icons/Herdr.icns itself. Sizes up to 32 px are
drawn without glows, shadows, sheens and scan lines, which blur a small icon
into its tile.
"""

import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets/icons/herdr-ui-icon-clean.svg"
VARIANTS = ROOT / "assets/icons/variants"
PREVIEWS = ROOT / "defaults/frame/settings/app-icons"
PREVIEW_PX = 96
SMALL_PX = 32

RAM_T = "translate(-80 967) scale(.22 -.22)"
TILE = 'x="64" y="64" width="896" height="896" rx="192"'


def svg(src, title, defs, tile, ram, overlay=""):
    meta = re.search(r"<metadata>.*?</metadata>", src, re.S).group(0)
    path = re.search(r'<path id="ram" d="([^"]+)"', src).group(1)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <title>Herdr – {title}</title>
  {meta}
  <defs>
    <clipPath id="clip"><rect {TILE}/></clipPath>
    <path id="ram" d="{path}"/>
    {defs}
  </defs>
  {tile}
  <g clip-path="url(#clip)">{ram}{overlay}</g>
</svg>
'''


def use(fill, extra=""):
    return f'<use href="#ram" transform="{RAM_T}" fill="{fill}" {extra}/>'.replace(" />", "/>")


def variants(src, small=False):
    """name -> SVG text; `small` leaves out the effects."""
    return {
        "midnight": svg(src, "Midnight",
            '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2a2f38"/><stop offset="1" stop-color="#15181d"/></linearGradient>',
            f'<rect {TILE} fill="url(#g)"/><rect x="65" y="65" width="894" height="894" rx="191" fill="none" stroke="#ffffff" stroke-opacity=".08" stroke-width="2"/>',
            use("#f4f1e9")),
        "mirage": svg(src, "Mirage",
            '<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2a3142"/><stop offset="1" stop-color="#1a1f29"/></linearGradient>'
            '<linearGradient id="r" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffd580"/><stop offset="1" stop-color="#f29e74"/></linearGradient>',
            f'<rect {TILE} fill="url(#g)"/>',
            use("url(#r)")),
        "glass": svg(src, "Glass",
            '<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#7fd3e8"/><stop offset=".55" stop-color="#4a8fd9"/><stop offset="1" stop-color="#5b5bd6"/></linearGradient>'
            '<linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".45"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient>'
            '<filter id="sh" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="14" stdDeviation="18" flood-color="#0b1f4a" flood-opacity=".45"/></filter>',
            f'<rect {TILE} fill="url(#g)"/>' + ("" if small else f'<rect {TILE} fill="url(#sheen)"/><rect x="66" y="66" width="892" height="892" rx="190" fill="none" stroke="#fff" stroke-opacity=".5" stroke-width="4"/>'),
            use("#ffffff", 'fill-opacity=".94"' + ("" if small else ' filter="url(#sh)"'))),
        "phosphor": svg(src, "Phosphor",
            '<radialGradient id="g" cx=".5" cy=".45" r=".75"><stop offset="0" stop-color="#0f2a17"/><stop offset="1" stop-color="#050b07"/></radialGradient>'
            '<filter id="glow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>'
            '<pattern id="scan" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="8" height="3" fill="#000" fill-opacity=".2"/></pattern>',
            f'<rect {TILE} fill="url(#g)"/>',
            use("#3dff7a", "" if small else 'filter="url(#glow)"'),
            "" if small else f'<rect {TILE} fill="url(#scan)"/>'),
        "graphite": svg(src, "Graphite",
            '<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#d9dadc"/><stop offset="1" stop-color="#a9abaf"/></linearGradient>',
            f'<rect {TILE} fill="url(#g)"/><rect x="65" y="65" width="894" height="894" rx="191" fill="none" stroke="#fff" stroke-opacity=".6" stroke-width="3"/>',
            use("#3a3d42")),
    }


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
    if sys.argv[1:] == ["--check"]:
        stale = [n for n, text in full.items() if (VARIANTS / f"{n}.svg").read_text() != text]
        if stale:
            sys.exit(f"stale: {', '.join(stale)}; run python3 scripts/app-icons.py")
        return
    for tool in ("rsvg-convert", "iconutil"):
        if not shutil.which(tool):
            sys.exit(f"{tool} is needed to write the icons")
    VARIANTS.mkdir(parents=True, exist_ok=True)
    PREVIEWS.mkdir(parents=True, exist_ok=True)
    render(SOURCE, PREVIEW_PX, PREVIEWS / "classic.png")
    with tempfile.TemporaryDirectory() as tmp:
        for name, text in full.items():
            (VARIANTS / f"{name}.svg").write_text(text)
            plain = pathlib.Path(tmp) / f"{name}-small.svg"
            plain.write_text(small[name])
            icns(VARIANTS / f"{name}.svg", plain, VARIANTS / f"{name}.icns")
            render(VARIANTS / f"{name}.svg", PREVIEW_PX, PREVIEWS / f"{name}.png")


if __name__ == "__main__":
    main()
