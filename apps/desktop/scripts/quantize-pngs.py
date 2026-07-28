"""Quantize the large PNGs to a smaller palette.

Targets:
  - apps/desktop/src-tauri/icons/icon.png           (1024x1024, currently ~1.2 MB)
  - apps/desktop/src/assets/linkvault-wordmark.png  (1264x424, currently ~540 KB)

Uses PIL.Image.quantize with median-cut + Floyd-Steinberg dithering.
For glow-heavy icons, 128 colors is the sweet spot between size and quality.
"""
import sys
from pathlib import Path
from PIL import Image

TARGETS = [
    (Path(r"apps\desktop\src-tauri\icons\icon.png"), 128),
    (Path(r"apps\desktop\src\assets\linkvault-wordmark.png"), 128),
]


def quantize(path: Path, colors: int = 128) -> tuple[int, int]:
    """Quantize a PNG in place. Returns (before, after) sizes in bytes."""
    if not path.exists():
        print(f"  skip (not found): {path}", file=sys.stderr)
        return (0, 0)
    before = path.stat().st_size
    img = Image.open(path).convert("RGBA")
    # Quantize the RGB channels while preserving alpha. PIL's quantize on an
    # RGBA image quantizes all 4 channels which we don't want.
    alpha = img.getchannel("A")
    rgb = img.convert("RGB").quantize(colors=colors, method=2, dither=1)
    out = rgb.convert("RGBA")
    out.putalpha(alpha)
    out.save(path, "PNG", optimize=True)
    after = path.stat().st_size
    return (before, after)


def main() -> None:
    for path, colors in TARGETS:
        before, after = quantize(path, colors)
        if before == 0:
            continue
        delta = after - before
        pct = (delta / before) * 100
        print(
            f"  {path.name:<28} {before:>9,} -> {after:>9,} bytes "
            f"({delta:+,d} {pct:+.1f}%) [{colors} colors]"
        )


if __name__ == "__main__":
    main()
