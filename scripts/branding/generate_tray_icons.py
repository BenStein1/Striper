#!/usr/bin/env python3
"""Derive Striper's in-app logo and tray icons from the original artwork.

Source (never modified): assets/branding/striper-original-icon.png

The platform app icons in src-tauri/icons are produced separately with:

    bun run tauri icon assets/branding/striper-original-icon.png -o src-tauri/icons

This script writes the 64x64 resources the tray code loads (filenames are
kept from upstream Handy so no Rust paths change):

  handy.png / handy_warning.png      full-colour logo (Linux "colored" theme idle)
  recording.png / transcribing.png   full-colour state glyphs (Linux "colored" theme)
  tray_*.png                         white glyphs for dark menu bars
  tray_*_dark.png                    black glyphs for light menu bars

States (same convention as upstream: idle = bare glyph, active = filled disc
with the glyph knocked out, warning = idle glyph + "!" badge):
  idle          fish silhouette (stripes and eye carved out)
  recording     disc with the artwork's waveform bars (every other bar) knocked out
  transcribing  disc with the fish silhouette knocked out

Also writes src-tauri/icons/logo.png (1024x1024), src/assets/striper-logo.png
(256x256) and src/assets/striper-fish-glyph.png (128x128 monochrome mask).

Usage: python3 scripts/branding/generate_tray_icons.py  (needs pillow, numpy, scipy)
"""

from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw
from scipy import ndimage as nd

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "assets/branding/striper-original-icon.png"
RES = ROOT / "src-tauri/resources"
SIZE = 64
SS = 16  # supersampling factor for drawn shapes (64 * 16 = 1024)


def load():
    rgba = np.array(Image.open(SRC).convert("RGBA")).astype(float)
    solid = rgba[..., 3] > 128
    labels, _ = nd.label(solid)
    sizes = nd.sum(solid, labels, range(1, labels.max() + 1))
    order = np.argsort(-sizes) + 1
    fish = labels == order[0]

    # Waveform bars: the tall, narrow, vertically centred components that sit
    # between the fish and the bottom swoosh.
    bars = np.zeros_like(solid)
    for lab in order[1:]:
        comp = labels == lab
        ys, xs = np.where(comp)
        if len(xs) < 2000:
            continue
        w, h = np.ptp(xs) + 1, np.ptp(ys) + 1
        if h > 2 * w and 450 < xs.mean() < 1150:
            bars |= comp
    return rgba, fish, bars, labels


def tray_bars(labels, bars):
    """Every other waveform bar, widened, so the glyph survives 16-22 px trays."""
    ids = sorted(set(np.unique(labels[bars])) - {0},
                 key=lambda lab: np.where(labels == lab)[1].mean())
    keep = np.isin(labels, ids[::2])
    return nd.binary_dilation(keep, structure=np.ones((1, 3)), iterations=8)


def carve_fish(rgba, fish):
    lum = 0.299 * rgba[..., 0] + 0.587 * rgba[..., 1] + 0.114 * rgba[..., 2]
    inner = nd.binary_erosion(fish, iterations=14)
    carve = (lum < 60) & inner
    carve = nd.binary_opening(carve, iterations=6)
    carve = nd.binary_dilation(carve, iterations=6) & inner
    return fish & ~carve


def square_crop(mask, pad_frac):
    ys, xs = np.where(mask)
    x0, x1, y0, y1 = xs.min(), xs.max(), ys.min(), ys.max()
    side = int(max(x1 - x0, y1 - y0) * (1 + 2 * pad_frac))
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    out = np.zeros((side, side), dtype=bool)
    sx, sy = cx - side // 2, cy - side // 2
    for y in range(side):
        yy = sy + y
        if 0 <= yy < mask.shape[0]:
            lo, hi = max(sx, 0), min(sx + side, mask.shape[1])
            out[y, lo - sx : hi - sx] = mask[yy, lo:hi]
    return out


def mask_to_alpha(mask, size):
    img = Image.fromarray((mask * 255).astype(np.uint8))
    return np.array(img.resize((size, size), Image.LANCZOS)).astype(float) / 255.0


def solid_icon(alpha, rgb):
    a = (np.clip(alpha, 0, 1) * 255).astype(np.uint8)
    out = np.zeros((alpha.shape[0], alpha.shape[1], 4), dtype=np.uint8)
    out[..., :3] = rgb
    out[..., 3] = a
    return Image.fromarray(out, "RGBA")


def disc_alpha(size, inset):
    big = Image.new("L", (size * SS, size * SS), 0)
    ImageDraw.Draw(big).ellipse(
        [inset * SS, inset * SS, (size - inset) * SS - 1, (size - inset) * SS - 1], fill=255
    )
    return np.array(big.resize((size, size), Image.LANCZOS)).astype(float) / 255.0


def badge_layers(size):
    """Return (cutout, badge) alpha arrays for a bottom-right "!" badge."""
    cx, cy, r, gap = 48, 48, 14, 3
    s = SS
    cut = Image.new("L", (size * s, size * s), 0)
    ImageDraw.Draw(cut).ellipse(
        [(cx - r - gap) * s, (cy - r - gap) * s, (cx + r + gap) * s, (cy + r + gap) * s], fill=255
    )
    badge = Image.new("L", (size * s, size * s), 0)
    d = ImageDraw.Draw(badge)
    d.ellipse([(cx - r) * s, (cy - r) * s, (cx + r) * s, (cy + r) * s], fill=255)
    # exclamation mark knocked out of the badge
    d.rounded_rectangle(
        [(cx - 2.2) * s, (cy - 9) * s, (cx + 2.2) * s, (cy + 3) * s], radius=2.2 * s, fill=0
    )
    d.ellipse([(cx - 2.5) * s, (cy + 5) * s, (cx + 2.5) * s, (cy + 10) * s], fill=0)
    to = lambda im: np.array(im.resize((size, size), Image.LANCZOS)).astype(float) / 255.0
    return to(cut), to(badge)


def colour_crop(rgba, mask, size, pad_frac):
    """Crop the colour artwork to `mask`'s bbox (square) and keep only `mask`."""
    ys, xs = np.where(mask)
    x0, x1, y0, y1 = xs.min(), xs.max(), ys.min(), ys.max()
    side = int(max(x1 - x0, y1 - y0) * (1 + 2 * pad_frac))
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    art = rgba.copy()
    art[..., 3] = np.where(mask, art[..., 3], 0)
    img = Image.fromarray(art.astype(np.uint8), "RGBA")
    box = (cx - side // 2, cy - side // 2, cx - side // 2 + side, cy - side // 2 + side)
    return img.crop(box).resize((size, size), Image.LANCZOS)


def main():
    rgba, fish, bars, labels = load()
    fish_glyph = carve_fish(rgba, fish)

    fish_a = mask_to_alpha(square_crop(fish_glyph, 0.02), SIZE)
    disc = disc_alpha(SIZE, 1)
    # glyphs placed inside the disc get a smaller footprint
    inner_fish = mask_to_alpha(square_crop(fish_glyph, 0.22), SIZE)
    inner_bars = mask_to_alpha(square_crop(tray_bars(labels, bars), 0.24), SIZE)

    recording = np.clip(disc - inner_bars, 0, 1)
    transcribing = np.clip(disc - inner_fish, 0, 1)
    cut, badge = badge_layers(SIZE)
    warning = np.clip(np.maximum(fish_a * (1 - cut), badge), 0, 1)

    white, black = (255, 255, 255), (0, 0, 0)
    for name, alpha in {
        "tray_idle": fish_a,
        "tray_recording": recording,
        "tray_transcribing": transcribing,
        "tray_idle_warning": warning,
    }.items():
        solid_icon(alpha, white).save(RES / f"{name}.png", optimize=True)
        solid_icon(alpha, black).save(RES / f"{name}_dark.png", optimize=True)

    # Colour variants (Linux "colored" tray theme and in-app logo).
    full = Image.open(SRC).convert("RGBA")
    logo64 = full.resize((SIZE, SIZE), Image.LANCZOS)
    logo64.save(RES / "handy.png", optimize=True)

    la = np.array(logo64).astype(float)
    la[..., 3] *= 1 - cut
    warn = Image.fromarray(la.astype(np.uint8), "RGBA")
    warn.alpha_composite(solid_icon(badge, (31, 92, 140)))
    warn.save(RES / "handy_warning.png", optimize=True)

    colour_crop(rgba, bars, SIZE, 0.08).save(RES / "recording.png", optimize=True)
    colour_crop(rgba, fish, SIZE, 0.02).save(RES / "transcribing.png", optimize=True)

    full.resize((1024, 1024), Image.LANCZOS).save(ROOT / "src-tauri/icons/logo.png", optimize=True)
    full.resize((256, 256), Image.LANCZOS).save(ROOT / "src/assets/striper-logo.png", optimize=True)
    # Monochrome glyph used as a CSS mask for the sidebar icon (tinted by theme).
    glyph = mask_to_alpha(square_crop(fish_glyph, 0.02), 128)
    solid_icon(glyph, white).save(ROOT / "src/assets/striper-fish-glyph.png", optimize=True)


if __name__ == "__main__":
    main()
