#!/usr/bin/env python3
"""Sizes of the wasm file as it ships: raw, gzip -9 and brotli -11 (of the wasm-opt output, which build.sh makes).

usage: python3 scripts/sizes.py dist/drafftink_canvas_js_bg.wasm
Needs the `brotli` module (pip install brotli), for example from ~/.venvs/perf.
"""
import gzip
import sys

try:
    import brotli
except ImportError:
    brotli = None

data = open(sys.argv[1], "rb").read()
print(f"raw     {len(data):>10,} bytes")
print(f"gzip -9 {len(gzip.compress(data, 9)):>10,} bytes")
if brotli:
    print(f"brotli  {len(brotli.compress(data, quality=11)):>10,} bytes")
else:
    print("brotli  (install the brotli module to measure)")
