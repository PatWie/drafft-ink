#!/usr/bin/env python3
"""Regenerate every app icon from packaging/icons/logo-original.png (the old repo's logo).

The logo is 293x385 with the word "Drafft" under the mark. The icons use the mark only: a square
crop above the word, centred on a white square with a small margin. Output (all committed, so
the packaging scripts need no image tools):

  png/<N>.png      N = 16 24 32 48 64 128 256 512 (Linux hicolor, the AppImage, the .app)
  drafftink.ico    16 24 32 48 64 128 256 (Windows)
  drafftink.icns   16 to 1024 (macOS; the 1024 is an upscale of the 293 px logo)

Needs Pillow:  python3 -m venv /tmp/v && /tmp/v/bin/pip install pillow && /tmp/v/bin/python packaging/icons/generate.py
"""
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
PNG_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICNS_SIZES = (16, 32, 64, 128, 256, 512, 1024)
# The mark spans the logo's top 300 rows; the word starts below row 310.
MARK_BOX = (0, 12, 293, 305)
MARGIN = 0.06


def master(size: int) -> Image.Image:
    logo = Image.open(HERE / "logo-original.png").convert("RGB")
    mark = logo.crop(MARK_BOX)
    inner = round(size * (1 - 2 * MARGIN))
    scale = inner / max(mark.size)
    mark = mark.resize((round(mark.width * scale), round(mark.height * scale)), Image.LANCZOS)
    canvas = Image.new("RGB", (size, size), "white")
    canvas.paste(mark, ((size - mark.width) // 2, (size - mark.height) // 2))
    return canvas


def main() -> None:
    (HERE / "png").mkdir(exist_ok=True)
    for s in PNG_SIZES:
        master(s).save(HERE / "png" / f"{s}.png", optimize=True)
    big = master(256)
    big.save(HERE / "drafftink.ico", sizes=[(s, s) for s in ICO_SIZES])
    master(1024).save(HERE / "drafftink.icns", sizes=[(s, s) for s in ICNS_SIZES])


if __name__ == "__main__":
    main()
