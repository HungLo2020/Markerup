#!/usr/bin/env python3
"""Make Tauri-generated iOS AppIcon files Apple-compliant RGB PNGs.

The SVG in resources/ remains the canonical app icon. Tauri emits RGBA PNGs
for its iOS asset catalog, including antialiased pixels along SVG edges. Apple
rejects any AppIcon PNG that has an alpha channel. This utility composites each
RGBA icon over the same opaque `#eaf4ff` background passed to `cargo tauri
icon`, then writes a standards-compliant 8-bit RGB PNG.
"""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
BACKGROUND = (0xEA, 0xF4, 0xFF)
DEFAULT_ICONSET = Path("gen/apple/Assets.xcassets/AppIcon.appiconset")


def paeth(left: int, above: int, upper_left: int) -> int:
    estimate = left + above - upper_left
    distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
    return (left, above, upper_left)[distances.index(min(distances))]


def png_chunks(data: bytes):
    if not data.startswith(PNG_SIGNATURE):
        raise ValueError("not a PNG file")
    offset = len(PNG_SIGNATURE)
    while offset < len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        if len(payload) != length or offset + length + 12 > len(data):
            raise ValueError("truncated PNG chunk")
        yield kind, payload
        offset += length + 12


def unfilter_rgba(compressed: bytes, width: int, height: int) -> bytes:
    row_bytes = width * 4
    decoded = zlib.decompress(compressed)
    expected = height * (row_bytes + 1)
    if len(decoded) != expected:
        raise ValueError(f"unexpected decoded image length {len(decoded)} (expected {expected})")
    output = bytearray(height * row_bytes)
    source = 0
    for row in range(height):
        filter_type = decoded[source]
        source += 1
        current = memoryview(output)[row * row_bytes : (row + 1) * row_bytes]
        previous = (
            memoryview(output)[(row - 1) * row_bytes : row * row_bytes]
            if row
            else bytes(row_bytes)
        )
        for index in range(row_bytes):
            value = decoded[source + index]
            left = current[index - 4] if index >= 4 else 0
            above = previous[index]
            upper_left = previous[index - 4] if index >= 4 else 0
            if filter_type == 0:
                current[index] = value
            elif filter_type == 1:
                current[index] = (value + left) & 0xFF
            elif filter_type == 2:
                current[index] = (value + above) & 0xFF
            elif filter_type == 3:
                current[index] = (value + ((left + above) // 2)) & 0xFF
            elif filter_type == 4:
                current[index] = (value + paeth(left, above, upper_left)) & 0xFF
            else:
                raise ValueError(f"unsupported PNG filter type {filter_type}")
        source += row_bytes
    return bytes(output)


def chunk(kind: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)


def convert_icon(path: Path) -> None:
    chunks = list(png_chunks(path.read_bytes()))
    header = next((payload for kind, payload in chunks if kind == b"IHDR"), None)
    if header is None or len(header) != 13:
        raise ValueError("missing or invalid IHDR")
    width, height, depth, color_type, compression, filtering, interlace = struct.unpack(">IIBBBBB", header)
    if (depth, color_type, compression, filtering, interlace) == (8, 2, 0, 0, 0):
        return
    if (depth, color_type, compression, filtering, interlace) != (8, 6, 0, 0, 0):
        raise ValueError(
            f"expected a non-interlaced 8-bit RGBA PNG, got depth={depth}, color_type={color_type}, interlace={interlace}"
        )
    rgba = unfilter_rgba(b"".join(payload for kind, payload in chunks if kind == b"IDAT"), width, height)
    rows = bytearray()
    for row in range(height):
        rows.append(0)  # write simple unfiltered RGB scanlines
        start = row * width * 4
        for pixel in range(start, start + width * 4, 4):
            alpha = rgba[pixel + 3]
            rows.extend(
                (rgba[pixel + channel] * alpha + BACKGROUND[channel] * (255 - alpha) + 127) // 255
                for channel in range(3)
            )
    rgb_header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    output = PNG_SIGNATURE + chunk(b"IHDR", rgb_header) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
    temporary = path.with_suffix(".tmp")
    temporary.write_bytes(output)
    temporary.replace(path)

    # Verify the on-disk result, not only the intended encoder parameters.
    _, verified_header = next(png_chunks(path.read_bytes()))
    if verified_header[8:10] != bytes((8, 2)):
        raise ValueError("output is not an 8-bit RGB PNG")


def main() -> None:
    if len(sys.argv) > 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} [AppIcon.appiconset directory]")
    iconset = Path(sys.argv[1]) if len(sys.argv) == 2 else DEFAULT_ICONSET
    icons = sorted(iconset.glob("*.png"))
    if not icons:
        raise SystemExit(f"no generated AppIcon PNGs found in {iconset}")
    for icon in icons:
        convert_icon(icon)
    print(f"converted and verified {len(icons)} opaque RGB iOS AppIcon PNGs")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, zlib.error) as error:
        raise SystemExit(f"iOS app icon conversion failed: {error}") from error
