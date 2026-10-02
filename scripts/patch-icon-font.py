#!/usr/bin/env python3
"""Writes crates/yagni-commander/assets/fonts/ from the Nerd Fonts release.

gpui's Linux text system drops any font face without a glyph for 'm'
(gpui-pre-wgpu, cosmic_text_system.rs, `load_family`), and the symbols-only
font has no letters. So 'm' is mapped to the font's empty `nonmarkingreturn`
glyph: the icons are untouched, and an 'm' drawn in this font is blank. Needs fontTools
(`pip install fonttools`, e.g. in a venv).
"""
import hashlib
import io
import tarfile
import urllib.request
from pathlib import Path

from fontTools.ttLib import TTFont

TAG = "v3.5.1"
URL = f"https://github.com/ryanoasis/nerd-fonts/releases/download/{TAG}/NerdFontsSymbolsOnly.tar.xz"
SHA256 = "01172f37db8543edb102e5cb5c64101c9f4686630804d49b419aa07b23a69996"
FONT = "SymbolsNerdFontMono-Regular.ttf"
OUT = Path(__file__).resolve().parent.parent / "crates/yagni-commander/assets/fonts"


def main() -> None:
    data = urllib.request.urlopen(URL).read()
    assert hashlib.sha256(data).hexdigest() == SHA256, "the release archive changed"
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:xz") as tar:
        font = TTFont(io.BytesIO(tar.extractfile(FONT).read()))
        license_text = tar.extractfile("LICENSE").read()
    blank = "nonmarkingreturn"
    assert font["glyf"][blank].numberOfContours == 0, f"{blank} is not empty"
    for table in font["cmap"].tables:
        if table.isUnicode():
            table.cmap.setdefault(ord("m"), blank)
    OUT.mkdir(parents=True, exist_ok=True)
    font.save(OUT / FONT)
    (OUT / "LICENSE").write_bytes(license_text)
    print(f"{OUT / FONT}: 'm' mapped to {blank}")


if __name__ == "__main__":
    main()
