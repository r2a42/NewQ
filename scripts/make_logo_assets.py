#!/usr/bin/env python3
"""Make NewQ's logo files (assets/logo/) from the shapes and colours below.

The SVGs are written here; the PNGs, icon files and the window icon are made
from them. Run it after changing the artwork, and commit what it writes.

Needs: python3 with fontTools (to turn the wordmark into outlines), node with
playwright (to draw the SVGs), and ImageMagick's `convert` (for the .ico and
the raw window icon). The wordmark is Space Grotesk (SIL OFL); pass the paths
of its Bold and Medium .ttf files:

    scripts/make_logo_assets.py SpaceGrotesk-Bold.ttf SpaceGrotesk-Medium.ttf
"""
import os
import struct
import subprocess
import sys
import tempfile

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
OUT = os.path.join(ROOT, "assets", "logo")

# Palette. The ring and the play triangle are ink on light, ivory ring on dark.
LIGHT = dict(ring="#14110F", accent="#EF9A14", hi="#FFC85E", lo="#E07F06",
             word="#14110F", q="#D9770A", pool=30,
             stops=[(0, 0, 1), (0.36, 1, 1), (0.68, 2, 1), (0.86, 2, 0.5), (1, 2, 0)])
DARK = dict(ring="#FAF6EE", accent="#F5A524", hi="#FFD98A", lo="#D08209",
            word="#FAF6EE", q="#F5A524", pool=27,
            stops=[(0, 0, 1), (0.42, 1, 1), (0.64, 2, 1), (1, 2, 0)])
INK = "#14110F"

# The mark's bounding box in its own coordinates, after the shift below.
BBOX = (9.0, 9.0, 108.0, 108.0)


def mark(theme, uid):
    """The logo mark: a spotlight beam under a ring, a pool of light and a play triangle."""
    t = theme
    shade = [t["hi"], t["accent"], t["lo"]]
    stops = "".join(
        f'<stop offset="{o}" stop-color="{shade[c]}"'
        + (f' stop-opacity="{a}"' if a != 1 else "")
        + "/>"
        for o, c, a in t["stops"]
    )
    return f"""<defs>
<linearGradient id="{uid}-beam" gradientUnits="userSpaceOnUse" x1="112" y1="112" x2="54" y2="54"><stop offset="0" stop-color="{t['accent']}" stop-opacity="0.95"/><stop offset="1" stop-color="{t['accent']}" stop-opacity="0.25"/></linearGradient>
<radialGradient id="{uid}-pool" gradientUnits="userSpaceOnUse" cx="54" cy="54" r="{t['pool']}" fx="47" fy="46">{stops}</radialGradient>
</defs>
<g transform="translate(-4 -4)">
<polygon points="112,112 68,54 45,69" fill="url(#{uid}-beam)"/>
<circle cx="54" cy="54" r="36" fill="none" stroke="{t['ring']}" stroke-width="10"/>
<circle cx="54" cy="54" r="{t['pool']}" fill="url(#{uid}-pool)"/>
<path d="M46.5 40.5 L46.5 70.5 L69.5 55.5 Z" fill="#000" fill-opacity="0.28"/>
<path d="M45 39 L45 69 L68 54 Z" fill="{INK}"/>
</g>"""


def svg(width, height, view, body, title):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="{view}" role="img" aria-label="{title}">\n<title>{title}</title>\n{body}\n</svg>\n'
    )


def mark_svg(theme, uid):
    x0, y0, x1, y1 = BBOX
    pad = 1.0
    w, h = x1 - x0 + 2 * pad, y1 - y0 + 2 * pad
    return svg(int(w * 2), int(h * 2), f"{x0 - pad} {y0 - pad} {w} {h}", mark(theme, uid), "NewQ")


def icon_svg(theme, uid, margin):
    """The mark on a dark rounded tile. `margin` is the share of the canvas left empty
    round the tile (macOS wants some)."""
    side = 120 * (1 - 2 * margin)
    at = 120 * margin
    x0, y0, x1, y1 = BBOX
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    s = side * 0.66 / (x1 - x0)
    inner = mark(theme, uid)
    body = (
        f'<rect x="{at}" y="{at}" width="{side}" height="{side}" rx="{side * 0.225}" fill="{INK}"/>\n'
        f'<g transform="translate({60} {60}) scale({s}) translate({-cx} {-cy})">\n{inner}\n</g>'
    )
    return svg(1024, 1024, "0 0 120 120", body, "NewQ")


def wordmark(font_path, size, spacing, ink, q_color):
    """'NewQ' as outlines, so the SVG needs no font. Returns (path markup, width, cap height)."""
    font = TTFont(font_path)
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = size / font["head"].unitsPerEm
    cap = font["OS/2"].sCapHeight * scale
    x = 0.0
    parts = []
    for i, ch in enumerate("NewQ"):
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs, ntos=lambda v: ("%.2f" % v).rstrip("0").rstrip("."))
        # flip y, scale, and move to the pen position
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, 0)))
        fill = q_color if ch == "Q" else ink
        parts.append(f'<path d="{pen.getCommands()}" fill="{fill}"/>')
        x += glyphs[name].width * scale + (spacing if i < 3 else 0)
    return "\n".join(parts), x, cap


def lockup_svg(theme, uid, bold):
    x0, y0, x1, y1 = BBOX
    mark_h = y1 - y0
    size = 80.0
    text, text_w, cap = wordmark(bold, size, -0.03 * size, theme["word"], theme["q"])
    gap = 16.0
    ring_cy = 54 - 4 - y0  # the ring's centre, in the lockup's coordinates
    baseline = ring_cy + cap / 2
    width = (x1 - x0) + gap + text_w
    body = (
        f'<g transform="translate({-x0} {-y0})">\n{mark(theme, uid)}\n</g>\n'
        f'<g transform="translate({x1 - x0 + gap:.2f} {baseline:.2f})">\n{text}\n</g>'
    )
    return svg(round(width * 3), round(mark_h * 3), f"0 0 {width:.2f} {mark_h:.2f}", body, "NewQ")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def render(jobs, fonts_css=""):
    """Draw SVGs to PNGs with Chromium: (svg path or html, out path, width, height, transparent)."""
    script = r"""
const { chromium } = require('playwright');
const fs = require('fs');
(async () => {
  const jobs = JSON.parse(fs.readFileSync(process.argv[2]));
  const browser = await chromium.launch();
  for (const j of jobs) {
    const page = await browser.newPage({ viewport: { width: j.w, height: j.h } });
    await page.goto('file://' + j.src);
    await page.screenshot({ path: j.out, omitBackground: j.transparent });
    await page.close();
  }
  await browser.close();
})();
"""
    tmp = tempfile.mkdtemp()
    runner = os.path.join(tmp, "render.js")
    open(runner, "w").write(script)
    plan = os.path.join(tmp, "jobs.json")
    import json
    out = []
    for src, png, w, h, transparent in jobs:
        page = os.path.join(tmp, os.path.basename(png) + ".html")
        if src.endswith(".html"):
            page = src
        else:
            markup = open(src).read()
            write(page, f'<!doctype html><meta charset="utf-8"><style>html,body{{margin:0;background:transparent}}'
                        f'svg{{display:block;width:{w}px;height:{h}px}}</style>{markup}')
        out.append(dict(src=page, out=png, w=w, h=h, transparent=transparent))
    json.dump(out, open(plan, "w"))
    env = dict(os.environ, NODE_PATH=subprocess.check_output(["npm", "root", "-g"]).decode().strip())
    subprocess.check_call(["node", runner, plan], env=env)


def icns(path, sizes):
    """An .icns of PNGs (what macOS reads since 10.7)."""
    kinds = {128: b"ic07", 256: b"ic08", 512: b"ic09", 1024: b"ic10"}
    chunks = b""
    for size, png in sizes:
        if size in kinds:
            data = open(png, "rb").read()
            chunks += kinds[size] + struct.pack(">I", len(data) + 8) + data
    with open(path, "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    bold, medium = sys.argv[1:]
    write(f"{OUT}/newq-mark.svg", mark_svg(LIGHT, "m"))
    write(f"{OUT}/newq-mark-dark.svg", mark_svg(DARK, "m"))
    write(f"{OUT}/newq-logo.svg", lockup_svg(LIGHT, "l", bold))
    write(f"{OUT}/newq-logo-dark.svg", lockup_svg(DARK, "l", bold))
    write(f"{OUT}/newq-icon.svg", icon_svg(DARK, "i", 0.0))
    write(f"{OUT}/newq-icon-macos.svg", icon_svg(DARK, "i", 0.0977))

    png = f"{OUT}/png"
    os.makedirs(png, exist_ok=True)
    jobs = []
    sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
    for s in sizes:
        jobs.append((f"{OUT}/newq-icon.svg", f"{png}/newq-icon-{s}.png", s, s, True))
    for s in (512, 1024):
        jobs.append((f"{OUT}/newq-icon-macos.svg", f"{png}/newq-icon-macos-{s}.png", s, s, True))
    # The macOS sizes below 512 are drawn from the same artwork.
    for s in (128, 256):
        jobs.append((f"{OUT}/newq-icon-macos.svg", f"{png}/newq-icon-macos-{s}.png", s, s, True))
    jobs.append((f"{OUT}/newq-mark.svg", f"{png}/newq-mark-512.png", 512, 512, True))
    jobs.append((f"{OUT}/newq-mark-dark.svg", f"{png}/newq-mark-dark-512.png", 512, 512, True))
    render(jobs)

    # Lockups: size from the SVG's own viewBox.
    def lockup_job(name, bg):
        src = f"{OUT}/{name}.svg"
        vb = [float(v) for v in open(src).read().split('viewBox="')[1].split('"')[0].split()]
        w = 1400
        h = round(w * vb[3] / vb[2])
        return (src, f"{png}/{name}.png", w, h, True)

    render([lockup_job("newq-logo", None), lockup_job("newq-logo-dark", None)])

    # Social preview: the dark lockup and a line, 1280x640 (GitHub's size).
    page = f"{OUT}/png/_social.html"
    dark = open(f"{OUT}/newq-logo-dark.svg").read()
    write(page, f"""<!doctype html><meta charset="utf-8"><style>
@font-face{{font-family:SG;font-weight:500;src:url('file://{medium}')}}
html,body{{margin:0;width:1280px;height:640px;background:#14110F;color:#FAF6EE;font-family:SG,sans-serif}}
.c{{display:flex;flex-direction:column;align-items:center;justify-content:center;height:640px;gap:44px}}
svg{{width:760px;height:auto}}p{{margin:0;font-size:38px;font-weight:500;letter-spacing:-0.01em;color:#CFC8BB}}
</style><div class="c">{dark}<p>Cue-based playback for theatre and live events</p></div>""")
    render([(page, f"{OUT}/social-preview.png", 1280, 640, False)])
    os.remove(page)

    # Windows .ico, the macOS .icns and the window icon the app embeds.
    pngs = [f"{png}/newq-icon-{s}.png" for s in (16, 24, 32, 48, 64, 128, 256)]
    subprocess.check_call(["convert", *pngs, f"{OUT}/newq.ico"])
    icns(f"{OUT}/newq.icns", [(s, f"{png}/newq-icon-macos-{s}.png") for s in (128, 256, 512, 1024)])
    subprocess.check_call(["convert", f"{png}/newq-icon-128.png", "-depth", "8", f"RGBA:{OUT}/newq-128.rgba"])
    # Keep only the PNGs people use.
    keep = {"newq-icon-256.png", "newq-icon-512.png", "newq-logo.png", "newq-logo-dark.png",
            "newq-mark-512.png", "newq-mark-dark-512.png"}
    for name in os.listdir(png):
        if name not in keep:
            os.remove(os.path.join(png, name))
    print("wrote", OUT)


if __name__ == "__main__":
    main()
