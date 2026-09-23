#!/usr/bin/env python3
"""Build a Debian package locally and optionally upload it through LinuxScripts."""

from __future__ import annotations

import argparse
import ast
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.request import Request, urlopen


LINUX_SCRIPTS_REPOSITORY = "HungLo2020/LinuxScripts"
LINUX_SCRIPTS_BRANCH = "master"
LINUX_SCRIPTS_PATH = "GenericScripts/ManageMattOSRepository.py"
LINUX_SCRIPTS_API = f"https://api.github.com/repos/{LINUX_SCRIPTS_REPOSITORY}"
REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
DEBIAN_PACKAGE_DIRECTORY = REPOSITORY_ROOT / "target" / "release" / "bundle" / "deb"


def download_latest_manager(target: Path) -> str:
    """Download the manager from the current upstream branch tip, atomically."""
    headers = {
        "Accept": "application/vnd.github+json",
        "Cache-Control": "no-cache",
        "Pragma": "no-cache",
        "User-Agent": "MarkerupDebianPublisher/1.0",
    }
    commit_request = Request(
        f"{LINUX_SCRIPTS_API}/commits/{LINUX_SCRIPTS_BRANCH}", headers=headers
    )
    with urlopen(commit_request, timeout=30) as response:
        commit = json.load(response)
    commit_sha = commit.get("sha")
    if not isinstance(commit_sha, str) or not commit_sha:
        raise RuntimeError("LinuxScripts did not return a current commit SHA")

    script_url = (
        f"https://raw.githubusercontent.com/{LINUX_SCRIPTS_REPOSITORY}/"
        f"{commit_sha}/{LINUX_SCRIPTS_PATH}"
    )
    script_request = Request(script_url, headers=headers)
    with urlopen(script_request, timeout=30) as response:
        content = response.read()
    try:
        source = content.decode("utf-8")
        tree = ast.parse(source, filename=LINUX_SCRIPTS_PATH)
    except (UnicodeDecodeError, SyntaxError) as error:
        raise RuntimeError("downloaded LinuxScripts manager is not valid UTF-8 Python") from error
    if not tree.body:
        raise RuntimeError("downloaded LinuxScripts manager is empty")

    target.write_text(source, encoding="utf-8")
    return commit_sha


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-only",
        action="store_true",
        help="Build the Debian package locally without uploading it",
    )
    args = parser.parse_args()

    previous_packages = {
        package: (package.stat().st_mtime_ns, package.stat().st_size)
        for package in DEBIAN_PACKAGE_DIRECTORY.glob("*.deb")
    } if DEBIAN_PACKAGE_DIRECTORY.exists() else {}
    print("Building the Debian package from the local checkout...")
    subprocess.run(
        ["cargo", "tauri", "build", "--bundles", "deb", "--", "--locked"],
        cwd=REPOSITORY_ROOT,
        check=True,
    )
    packages = sorted(
        package
        for package in DEBIAN_PACKAGE_DIRECTORY.glob("*.deb")
        if (package.stat().st_mtime_ns, package.stat().st_size) != previous_packages.get(package)
    )
    if len(packages) != 1:
        raise RuntimeError(f"Expected one locally built Debian package, found {len(packages)}")
    package = packages[0]
    print(f"Built Debian package: {package}")
    if args.build_only:
        print("Build-only mode selected; package was not uploaded.")
        return 0

    manager_directory = tempfile.TemporaryDirectory(prefix="markerup-linuxscripts-")
    try:
        manager_path = Path(manager_directory.name) / "ManageMattOSRepository.py"
        commit_sha = download_latest_manager(manager_path)
        print(f"Using LinuxScripts manager from commit {commit_sha}")
        subprocess.run(
            [sys.executable, str(manager_path), "--repo", "mattpackages", "upload", str(package)],
            check=True,
        )
        print("Debian package uploaded through LinuxScripts.")
        return 0
    finally:
        print("Deleting temporary manager script.")
        manager_directory.cleanup()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, OSError, subprocess.CalledProcessError, KeyboardInterrupt) as error:
        print(f"Debian publication failed: {error}", file=sys.stderr)
        raise SystemExit(1)
