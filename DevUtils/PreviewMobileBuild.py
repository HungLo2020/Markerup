#!/usr/bin/env python3
"""Launch Markerup's existing mobile UI locally on Linux for visual preview.

This is a development preview only. It uses the normal debug build and the
existing MobileWindow component; it does not build an iOS binary or change the
default Linux desktop build.
"""

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

    command = ["cargo", "run", "--features", "mobile-preview"]
    print("Launching Markerup mobile preview in debug mode...", flush=True)
    return subprocess.run(command, cwd=repo_root, env=environment).returncode


if __name__ == "__main__":
    raise SystemExit(main())
