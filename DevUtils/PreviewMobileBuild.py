#!/usr/bin/env python3
"""Launch the responsive Tauri UI in an iPhone-sized Linux window."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def main() -> int:
    if sys.platform != "linux":
        print("PreviewMobileBuild.py is intended to run on Linux.", file=sys.stderr)
        return 2

    repo_root = Path(__file__).resolve().parents[1]
    environment = os.environ.copy()
    environment.setdefault("RUST_BACKTRACE", "1")

    command = [
        "cargo", "tauri", "dev", "--config",
        '{"app":{"windows":[{"title":"Markerup mobile preview","width":390,"height":844,"minWidth":390,"minHeight":700}]}}',
    ]
    print("Launching Markerup responsive mobile preview...", flush=True)
    return subprocess.run(command, cwd=repo_root, env=environment).returncode


if __name__ == "__main__":
    raise SystemExit(main())
