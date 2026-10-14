#!/usr/bin/env python3
"""Writes the app icon, everything in packaging/icons/.

Drawn from scratch by construction (no fonts, no traced artwork), so the
icon is the project's own and MIT like the rest:

- a rounded square, cut by two diagonal gaps (lines x + y = const) into a
  top-left piece, a middle band and a bottom-right piece;
- a black rounded tile in the middle, with a gap around it, splitting the
  band into an upper-right and a lower-left piece;
- every convex corner of the pieces rounded;
- "yc" on the tile in bold, blocky 8-bit pixels (two-pixel strokes, so
  they stay readable at launcher sizes). A letter pixel is 1/32 of the
  canvas and the letters are an even number of pixels wide and high, so
  they sit centered with every edge on a whole screen pixel at 32 and 64 px
  (and 128 and up). The PNGs draw them through `hinted`, sharp at every
  size.

The output is plain filled paths (no masks, clip paths or strokes), which
every SVG renderer draws the same, including Qt's for KDE icons. Files:

- yagni-commander.svg: the icon (installed as Linux hicolor/scalable);
- hicolor/<n>x<n>/apps/yagni-commander.png: 16 to 512 px (installed in
  the Linux icon theme; below 32 px from the small SVG);
- yagni-commander.iconset/: macOS, the input of `iconutil -c icns`
  (up to 32 px with the small "y");
- yagni-commander-small.svg: a bold "y" alone, the source of the
  16 to 24 px PNGs (not installed);
- yagni-commander-macos.svg: the art on Apple's rounded-square grid, the
  source of the iconset (not installed).

PNGs are rendered with cairo. Needs shapely and cairosvg
(`pip install shapely cairosvg`, e.g. in a venv).
"""
from pathlib import Path

import cairosvg
from shapely import affinity, unary_union
from shapely.geometry import LineString, MultiPolygon, Polygon, box

OUT = Path(__file__).resolve().parent.parent / "packaging" / "icons"

PINK = "#FF1F8E"
ORANGE = "#FF8A00"
RED = "#FD2E0F"
PURPLE = "#7B2FF7"
TILE = "#000000"
INK = "#EEEDED"

SIZE = 1024
C = SIZE / 2
BODY = 928  # side of the rounded square
BODY_RADIUS = 200
BAND = 420  # gaps on x + y = SIZE -/+ BAND
GAP = 30
TILE_SIDE = 640
TILE_RADIUS = 120
CORNER = 26  # rounding of the pieces' corners
PIXEL = SIZE // 32  # side of one letter pixel: one screen pixel at 32 px
ARC_STEPS = 24


def rounded_square(cx, cy, side, radius):
    half = side / 2 - radius
    return box(cx - half, cy - half, cx + half, cy + half).buffer(radius, quad_segs=ARC_STEPS)


def half_plane(sign, offset):
    """x + y < SIZE + offset (sign -1) or x + y > SIZE + offset (sign 1)."""
    far = SIZE * 4
    k = SIZE + offset
    if sign < 0:
        return Polygon([(-far, -far), (k + far, -far), (-far, k + far)])
    return Polygon([(far, far), (k - far, far), (far, k - far)])


def gap_line(offset):
    k = SIZE + offset
    return LineString([(-SIZE, k + SIZE), (k + SIZE, -SIZE)]).buffer(GAP / 2, cap_style="flat")


def round_corners(shape):
    return shape.buffer(-CORNER, quad_segs=ARC_STEPS).buffer(CORNER, quad_segs=ARC_STEPS)


def pieces():
    body = rounded_square(C, C, BODY, BODY_RADIUS)
    body = body.difference(gap_line(-BAND)).difference(gap_line(BAND))
    body = body.difference(rounded_square(C, C, TILE_SIDE + 2 * GAP, TILE_RADIUS + GAP))
    top_left = body.intersection(half_plane(-1, -BAND))
    bottom_right = body.intersection(half_plane(1, BAND))
    band = body.difference(half_plane(-1, -BAND)).difference(half_plane(1, BAND))
    # The tile splits the band; the part above the main diagonal y = x is upper right.
    upper = band.intersection(Polygon([(-SIZE, -SIZE), (2 * SIZE, -SIZE), (2 * SIZE, 2 * SIZE)]))
    lower = band.difference(upper)
    return [
        (PINK, round_corners(top_left)),
        (ORANGE, round_corners(upper)),
        (RED, round_corners(lower)),
        (PURPLE, round_corners(bottom_right)),
    ]


# "yc", one character per pixel.
GLYPHS = [
    "##..##..######",
    "##..##..######",
    "##..##..##....",
    "##..##..##....",
    "##..##..##....",
    "######..##....",
    "######..##....",
    "....##..##....",
    "######..######",
    "######..######",
]


# The small icon's bold "y": one pixel is 64 units, a screen pixel at 16 px.
SMALL_GLYPHS = [
    "##..##",
    "##..##",
    "##..##",
    "######",
    "######",
    "....##",
    "######",
    "######",
]
SMALL_PIXEL = 64


def pixel_art(glyphs, pixel):
    pixels = [
        box(x * pixel, y * pixel, (x + 1) * pixel, (y + 1) * pixel)
        for y, row in enumerate(glyphs)
        for x, cell in enumerate(row)
        if cell == "#"
    ]
    shape = unary_union(pixels).simplify(0)
    # Center the drawing on the tile; the glyphs' even sizes keep it on the pixel grid.
    x0, y0, x1, y1 = shape.bounds
    dx, dy = C - (x0 + x1) / 2, C - (y0 + y1) / 2
    assert dx % pixel == 0 and dy % pixel == 0, "glyphs must be an even number of pixels wide and high"
    return affinity.translate(shape, dx, dy)


def path(shape):
    polys = shape.geoms if isinstance(shape, MultiPolygon) else [shape]
    out = []
    for poly in polys:
        for ring in [poly.exterior, *poly.interiors]:
            pts = list(ring.coords)[:-1]
            out.append("M" + "L".join(f"{x:.1f} {y:.1f}" for x, y in pts) + "Z")
    return "".join(out)


def svg(shapes, background=None):
    body = []
    if background is not None:
        body.append(f'<path fill="{TILE}" d="{path(background)}"/>')
    body += [f'<path fill="{color}" d="{path(shape)}"/>' for color, shape in shapes]
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" '
        f'viewBox="0 0 {SIZE} {SIZE}">\n' + "\n".join(body) + "\n</svg>\n"
    )


def hinted(glyphs, pixel, size, scale=1.0):
    """The letters for a PNG of `size` px, every edge on a whole screen pixel.

    Scaling `pixel_art` down puts edges between screen pixels wherever a
    letter pixel isn't a whole number of them (22, 24 and 48 px, every
    macOS size), which blurs them. Here each run between two edges used
    anywhere in the glyphs gets its own rounded width (at least one screen
    pixel), so equal strokes stay equal, and the letters are centered on a
    whole pixel. `scale` is the art's scale (macOS). Returned in canvas units.
    """
    unit = pixel * scale * size / SIZE  # screen pixels per letter pixel

    def edges(lines):
        cuts = {0, len(lines[0])}
        for line in lines:
            cuts.update(i for i in range(1, len(line)) if line[i] != line[i - 1])
        cuts = sorted(cuts)
        at, pos = {cuts[0]: 0}, 0
        for a, b in zip(cuts, cuts[1:]):
            pos += max(1, int((b - a) * unit + 0.5))
            at[b] = pos
        return at

    xs = edges(glyphs)
    ys = edges(["".join(row[x] for row in glyphs) for x in range(len(glyphs[0]))])
    left = int((size - xs[len(glyphs[0])]) / 2 + 0.5)
    top = int((size - ys[len(glyphs)]) / 2 + 0.5)
    to_canvas = SIZE / size
    cells = []
    for y, row in enumerate(glyphs):
        for x, cell in enumerate(row):
            if cell == "#":
                x0, x1 = left + xs[max(k for k in xs if k <= x)], left + xs[min(k for k in xs if k > x)]
                y0, y1 = top + ys[max(k for k in ys if k <= y)], top + ys[min(k for k in ys if k > y)]
                cells.append(box(x0 * to_canvas, y0 * to_canvas, x1 * to_canvas, y1 * to_canvas))
    return unary_union(cells).simplify(0)


def main():
    tile = (TILE, rounded_square(C, C, TILE_SIDE, TILE_RADIUS))
    art = pieces() + [tile]
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "yagni-commander.svg").write_text(svg(art + [(INK, pixel_art(GLYPHS, PIXEL))]))
    (OUT / "yagni-commander-small.svg").write_text(svg(art + [(INK, pixel_art(SMALL_GLYPHS, SMALL_PIXEL))]))
    # macOS: Apple's grid, an 824 px rounded square with a 100 px margin, the art inside it.
    mac_art = on_mac(art)
    (OUT / "yagni-commander-macos.svg").write_text(
        svg(mac_art + [(INK, scaled(pixel_art(GLYPHS, PIXEL), MAC_SCALE))], background=MAC_BACKGROUND)
    )
    write_pngs(art, mac_art)


MAC_SCALE = 764 / BODY
MAC_BACKGROUND = rounded_square(C, C, 824, 185)


def scaled(shape, factor):
    return affinity.scale(shape, factor, factor, origin=(C, C))


def on_mac(art):
    return [(color, scaled(shape, MAC_SCALE)) for color, shape in art]


LINUX_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]
SMALL_BELOW = 32
MAC_SIZES = [16, 32, 128, 256, 512]


def png(source, size, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    cairosvg.svg2png(bytestring=source.encode(), write_to=str(target), output_width=size, output_height=size)


def write_pngs(art, mac_art):
    """Each PNG is the art with letters hinted for its size; the SVGs stay unhinted."""
    for size in LINUX_SIZES:
        glyphs, pixel = (SMALL_GLYPHS, SMALL_PIXEL) if size < SMALL_BELOW else (GLYPHS, PIXEL)
        source = svg(art + [(INK, hinted(glyphs, pixel, size))])
        png(source, size, OUT / "hicolor" / f"{size}x{size}" / "apps" / "yagni-commander.png")
    # The macOS margin leaves the art a quarter smaller, so the small "y" serves up to 32 px.
    iconset = OUT / "yagni-commander.iconset"
    for size in MAC_SIZES:
        for factor, suffix in [(1, ""), (2, "@2x")]:
            pixels = size * factor
            glyphs, pixel = (SMALL_GLYPHS, SMALL_PIXEL) if pixels <= SMALL_BELOW else (GLYPHS, PIXEL)
            source = svg(mac_art + [(INK, hinted(glyphs, pixel, pixels, MAC_SCALE))], background=MAC_BACKGROUND)
            png(source, pixels, iconset / f"icon_{size}x{size}{suffix}.png")


if __name__ == "__main__":
    main()
