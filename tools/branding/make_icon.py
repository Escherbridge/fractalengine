"""Regenerates fractalengine/assets/branding/icon.{png,ico} (petal-flower mark).

Usage: python tools/branding/make_icon.py   (needs Pillow)
"""
from pathlib import Path

from PIL import Image, ImageDraw

SUPERSAMPLE = 1024
OUT = Path(__file__).resolve().parents[2] / "fractalengine" / "assets" / "branding"
BACKGROUND = (22, 22, 28, 255)        # theme::BG_TOOLBAR
BORDER = (60, 100, 160, 255)          # theme::BG_BUTTON_ACTIVE
OUTER_PETAL = (90, 140, 220, 200)
INNER_PETAL = (120, 190, 240, 220)    # theme::ICON_TRACK
CORE = (240, 210, 90, 255)            # theme::ICON_POINT


def petal_ring(canvas, count, length, width, offset_degrees, color):
    """Composite `count` ellipse petals radiating from the canvas center."""
    center = SUPERSAMPLE / 2
    for index in range(count):
        layer = Image.new("RGBA", (SUPERSAMPLE, SUPERSAMPLE), (0, 0, 0, 0))
        ImageDraw.Draw(layer).ellipse(
            (center - width / 2, center - length, center + width / 2, center), fill=color
        )
        angle = offset_degrees + index * 360 / count
        canvas.alpha_composite(layer.rotate(angle, resample=Image.BICUBIC, center=(center, center)))


def main():
    canvas = Image.new("RGBA", (SUPERSAMPLE, SUPERSAMPLE), (0, 0, 0, 0))
    margin = SUPERSAMPLE * 0.04
    ImageDraw.Draw(canvas).rounded_rectangle(
        (margin, margin, SUPERSAMPLE - margin, SUPERSAMPLE - margin),
        radius=SUPERSAMPLE * 0.2, fill=BACKGROUND, outline=BORDER, width=int(SUPERSAMPLE * 0.03),
    )
    petal_ring(canvas, 6, SUPERSAMPLE * 0.40, SUPERSAMPLE * 0.20, 0, OUTER_PETAL)
    petal_ring(canvas, 6, SUPERSAMPLE * 0.24, SUPERSAMPLE * 0.12, 30, INNER_PETAL)
    radius = SUPERSAMPLE * 0.075
    center = SUPERSAMPLE / 2
    ImageDraw.Draw(canvas).ellipse(
        (center - radius, center - radius, center + radius, center + radius), fill=CORE
    )
    icon = canvas.resize((256, 256), Image.LANCZOS)
    OUT.mkdir(parents=True, exist_ok=True)
    icon.save(OUT / "icon.png", optimize=True)
    sizes = [(s, s) for s in (16, 24, 32, 48, 64, 128, 256)]
    icon.save(OUT / "icon.ico", sizes=sizes)
    print(f"wrote {OUT / 'icon.png'} and {OUT / 'icon.ico'}")


if __name__ == "__main__":
    main()
