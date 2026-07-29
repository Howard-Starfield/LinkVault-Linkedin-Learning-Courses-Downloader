"""Generate Tauri app icon set from a single source image.

Produces:
  - icon.png         (1024x1024)  - main bundle icon
  - icon-taskbar.png (48x48)      - Windows taskbar (smallest sharp size)
  - icon-tray.png    (48x48)      - system tray (small)
  - icon.ico         (multi-res)  - Windows installer + executable icon
                                     (16, 24, 32, 48, 64, 128, 256)

The source PNG is RGB with a WHITE background (the "transparent_corners"
in the filename is misleading — the file has no alpha channel). We derive
a real alpha channel here: pixels close to white become transparent, the
icon body stays opaque, and the anti-aliased edges fade with a smooth
gradient. The Windows .ico format preserves the alpha channel when
saving RGBA PNG frames, so the installer icon and taskbar/tray icons
all show transparent corners on dark or light backgrounds.

Downscaling uses LANCZOS (best for shrinking without blur) with a mild
unsharp-mask pre-sharpening pass to recover perceived sharpness at small
sizes — addresses the 'don't make it blurry' requirement.
"""
import sys
from pathlib import Path
import numpy as np
from PIL import Image, ImageFilter

SOURCE = Path(
    r"C:\Users\howard\.minimax\v2\assets\2026\07\28\13-59-04-548-asset_20260728-135904-548_c968ccf8062c_8a809fcf-downloader_icon_transparent_corners.png"
)
ICON_DIR = Path(r"apps\desktop\src-tauri\icons")


def derive_alpha(rgb: np.ndarray) -> np.ndarray:
    """Turn a white-background RGB image into RGBA with real transparency.

    For each pixel, the alpha is 255 minus the max channel value, then
    rescaled so that:
      - max channel >= 250 (essentially white) -> alpha = 0
      - max channel <= 200 (saturated icon color) -> alpha = 255
      - in between -> linear gradient (anti-aliased edge)

    Pixels with a "blue" or "purple" icon color have at least one channel
    well below 200, so the icon body stays fully opaque. Pure white
    background goes to alpha=0. The glow edges that fade from icon
    color toward white become semi-transparent, which is what we want.
    """
    max_ch = rgb.max(axis=-1).astype(np.int16)
    # Linear ramp: alpha = 255 when max_ch <= 200, alpha = 0 when max_ch >= 250
    alpha = 255 - ((max_ch - 200) * 255 // 50)
    alpha = np.clip(alpha, 0, 255).astype(np.uint8)
    return alpha


def load_source() -> Image.Image:
    """Load source, pad to square, derive alpha from RGB, return RGBA."""
    img = Image.open(SOURCE).convert("RGB")
    if img.size != (img.size[0],) * 2:
        side = max(img.size)
        canvas = Image.new("RGB", (side, side), (255, 255, 255))
        canvas.paste(img, ((side - img.size[0]) // 2, (side - img.size[1]) // 2))
        img = canvas
    rgb = np.array(img)
    alpha = derive_alpha(rgb)
    rgba = np.dstack([rgb, alpha])
    return Image.fromarray(rgba, mode="RGBA")


def downscale(img: Image.Image, size: int) -> Image.Image:
    """LANCZOS downscale with a mild unsharp mask to keep small icons crisp.

    Splits the image into RGB+A first so the unsharp-mask only acts on
    the colour channels (not the alpha), which keeps the icon edge
    crisp without smearing the transparency.
    """
    rgb = np.array(img.convert("RGB"))
    alpha = np.array(img.getchannel("A"))
    rgb_pil = Image.fromarray(rgb).resize((size, size), Image.Resampling.LANCZOS)
    alpha_pil = Image.fromarray(alpha).resize((size, size), Image.Resampling.LANCZOS)
    if size <= 64:
        rgb_pil = rgb_pil.filter(ImageFilter.UnsharpMask(radius=1, percent=120, threshold=2))
    rgba = np.dstack([np.array(rgb_pil), np.array(alpha_pil)])
    return Image.fromarray(rgba, mode="RGBA")


def main() -> None:
    if not SOURCE.exists():
        print(f"ERROR: source not found: {SOURCE}", file=sys.stderr)
        sys.exit(1)

    ICON_DIR.mkdir(parents=True, exist_ok=True)
    src = load_source()
    print(f"Source: {SOURCE.name}  loaded as {src.size} {src.mode}")
    # Quick alpha sanity check: how many pixels are fully transparent?
    alpha_arr = np.array(src.getchannel("A"))
    fully_transparent = int((alpha_arr == 0).sum())
    fully_opaque = int((alpha_arr == 255).sum())
    print(f"  alpha: {fully_transparent} transparent, {fully_opaque} opaque, "
          f"{alpha_arr.size - fully_transparent - fully_opaque} partial")

    # 1. Master icon.png (1024x1024)
    master = downscale(src, 1024)
    master.save(ICON_DIR / "icon.png", "PNG", optimize=True)
    print(f"  wrote icon.png        ({master.size})")

    # 2. icon-taskbar.png (48x48)
    tb = downscale(src, 48)
    tb.save(ICON_DIR / "icon-taskbar.png", "PNG", optimize=True)
    print(f"  wrote icon-taskbar.png ({tb.size})")

    # 3. icon-tray.png (48x48) — same size as taskbar so the small-icon
    #    detail survives. Windows scales both surfaces from 32-256 px
    #    depending on DPI/theme; 48 is the sweet spot.
    tr = downscale(src, 48)
    tr.save(ICON_DIR / "icon-tray.png", "PNG", optimize=True)
    print(f"  wrote icon-tray.png    ({tr.size})")

    # 4. icon.ico (multi-resolution: 16, 24, 32, 48, 64, 128, 256).
    #    Each frame is a fully RGBA PNG, which Windows uses for the
    #    alpha-aware icon on the taskbar/tray/start-menu surfaces.
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    ico_frames = [downscale(src, s) for s in ico_sizes]
    ico_frames[0].save(
        ICON_DIR / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in ico_sizes],
        append_images=ico_frames[1:],
    )
    print(f"  wrote icon.ico         (multi-res: {ico_sizes})")

    print()
    print("Final icon set in", ICON_DIR)
    for p in sorted(ICON_DIR.glob("*")):
        print(f"  {p.name:<24} {p.stat().st_size:>9} bytes")


if __name__ == "__main__":
    main()
