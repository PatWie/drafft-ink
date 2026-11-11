#!/usr/bin/env python3
"""Rebuilds the glyph-fallback fonts of drafftink-typeset from the upstream Noto files.

The fonts are committed (crates/typeset/assets/fonts/*-Fallback.*); run this script only to regenerate or extend
them. It needs fontTools and brotli (`uv venv v && uv pip install --python v/bin/python fonttools brotli`).

    scripts/build-fallback-fonts.py <directory with the upstream files>

Upstream files (all SIL OFL 1.1, none has a Reserved Font Name):
  NotoSansSymbols-Regular.ttf, NotoSansSymbols2-Regular.ttf, NotoSansMath-Regular.ttf, NotoSansKannada-Regular.ttf,
  NotoSansArabic-Regular.ttf
      https://github.com/notofonts/notofonts.github.io/raw/main/fonts/<Family>/full/ttf/<Family>-Regular.ttf
  NotoSansCJKjp-Regular.otf
      https://github.com/notofonts/noto-cjk/raw/main/Sans/OTF/Japanese/NotoSansCJKjp-Regular.otf

Each font keeps only the code point ranges below, and the ranges do not overlap: a code point belongs to exactly one
fallback font. `crates/typeset/src/fallback.rs` repeats the ranges (the typesetter uses them to tell which font to
load for a missing character) and a test checks that the two agree.
"""
import pathlib
import sys

from fontTools import merge, subset
from fontTools.ttLib import TTFont

OUT = pathlib.Path(__file__).resolve().parent.parent / "crates/typeset/assets/fonts"

# The symbols font is Noto Sans Symbols 2, plus the glyphs of the same ranges that only Noto Sans Symbols has (the
# smileys U+263A and U+2639, for one). Both have 1000 units per em, so their glyphs merge without scaling.
SYMBOLS = "25A0-25FF,2600-27BF,2B00-2BFF"

# (upstream file, output file, ranges, keep OpenType layout features)
FONTS = [
    (
        "NotoSansSymbols2-Regular.ttf",
        "NotoSansSymbols2-Fallback.ttf",
        SYMBOLS,
        False,
    ),
    (
        "NotoSansMath-Regular.ttf",
        "NotoSansMath-Fallback.ttf",
        "2190-23FF",
        False,
    ),
    ("NotoSansKannada-Regular.ttf", "NotoSansKannada-Fallback.ttf", "0C80-0CFF", True),
    (
        "NotoSansArabic-Regular.ttf",
        "NotoSansArabic-Fallback.ttf",
        "0600-06FF,0750-077F,08A0-08FF,FB50-FDFF,FE70-FEFF",
        True,
    ),
    (
        "NotoSansCJKjp-Regular.otf",
        "NotoSansCJK-Fallback.otf",
        "1100-11FF,2500-259F,3000-30FF,3131-318E,31F0-31FF,FE30-FE4F,FF00-FFEF",
        False,
    ),
]


def unicodes(spec: str) -> list[int]:
    out = []
    for part in spec.split(","):
        lo, _, hi = part.partition("-")
        out.extend(range(int(lo, 16), int(hi or lo, 16) + 1))
    return out


def options(layout: bool) -> subset.Options:
    opts = subset.Options()
    opts.hinting = False
    opts.notdef_outline = True
    opts.name_IDs = ["*"]
    opts.name_languages = ["*"]
    opts.layout_features = ["*"] if layout else []
    opts.drop_tables += ["DSIG"]
    return opts


def cut(path: pathlib.Path, codepoints: list[int], layout: bool) -> TTFont:
    opts = options(layout)
    font = subset.load_font(path, opts)
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=codepoints)
    sub.subset(font)
    return font


def prune_cmap(font: TTFont, codepoints: list[int]) -> None:
    """Drops the cmap entries outside `codepoints`: the subsetter keeps those of glyphs it needs for other reasons."""
    keep = set(codepoints)
    for table in font["cmap"].tables:
        table.cmap = {cp: glyph for cp, glyph in table.cmap.items() if cp in keep}


def save(font: TTFont, name: str, layout: bool, codepoints: list[int]) -> None:
    prune_cmap(font, codepoints)
    subset.save_font(font, OUT / name, options(layout))
    print(name, (OUT / name).stat().st_size)


def main() -> None:
    src = pathlib.Path(sys.argv[1])
    for upstream, name, ranges, layout in FONTS:
        font = cut(src / upstream, unicodes(ranges), layout)
        if upstream == "NotoSansSymbols2-Regular.ttf":
            font = add_symbols_only_in_noto_symbols(src, font)
        save(font, name, layout, unicodes(ranges))


def add_symbols_only_in_noto_symbols(src: pathlib.Path, symbols2: TTFont) -> TTFont:
    """Merges the glyphs that Noto Sans Symbols has in `SYMBOLS` and Noto Sans Symbols 2 has not."""
    have = set(symbols2.getBestCmap())
    extra = [cp for cp in unicodes(SYMBOLS) if cp not in have]
    other = cut(src / "NotoSansSymbols-Regular.ttf", extra, False)
    tmp = OUT / "symbols-part.tmp.ttf"
    tmp2 = OUT / "symbols2-part.tmp.ttf"
    other.save(tmp)
    symbols2.save(tmp2)
    try:
        merged = merge.Merger().merge([str(tmp2), str(tmp)])
    finally:
        tmp.unlink(missing_ok=True)
        tmp2.unlink(missing_ok=True)
    return merged


if __name__ == "__main__":
    main()
