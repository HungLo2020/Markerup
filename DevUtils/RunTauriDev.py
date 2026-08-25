#!/usr/bin/env python3
"""Run Markerup's Vite frontend and Tauri desktop shell together."""

from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    # `cargo tauri dev` owns the Vite lifecycle through beforeDevCommand, so the
    # debug WebView never starts before its configured devUrl is available.
    return subprocess.call(["cargo", "tauri", "dev"], cwd=ROOT)


if __name__ == "__main__":
    sys.exit(main())
