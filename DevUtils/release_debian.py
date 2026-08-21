#!/usr/bin/env python3
"""Build, download, and install the Debian package through GitHub Actions only."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def gh(*args: str, check: bool = True) -> str:
    result = subprocess.run(
        ["gh", *args],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check and result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "gh command failed")
    return result.stdout.strip()


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def find_run(repo: str, started_after: datetime, source_sha: str) -> dict[str, Any]:
    deadline = time.monotonic() + 30 * 60
    while time.monotonic() < deadline:
        output = gh(
            "run",
            "list",
            "--repo",
            repo,
            "--workflow",
            "debian.yml",
            "--branch",
            "main",
            "--limit",
            "20",
            "--json",
            "databaseId,headSha,event,createdAt",
        )
        candidates = [
            run
            for run in json.loads(output or "[]")
            if run["event"] == "workflow_dispatch"
            and run["headSha"] == source_sha
            and parse_time(run["createdAt"]) >= started_after
        ]
        if candidates:
            return sorted(candidates, key=lambda run: run["createdAt"])[-1]
        time.sleep(5)
    raise RuntimeError("Timed out waiting for the Debian workflow run")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", help="GitHub repository, for example HungLo2020/Markerup")
    args = parser.parse_args()
    repo = args.repo or gh("repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")

    source_sha = gh("api", f"repos/{repo}/git/ref/heads/main", "--jq", ".object.sha")
    started = datetime.now(timezone.utc)
    print("Dispatching the Debian-only package workflow...")
    gh("workflow", "run", "debian.yml", "--repo", repo, "--ref", "main")
    run = find_run(repo, started, source_sha)
    run_id = int(run["databaseId"])
    print(f"Watching Debian workflow run {run_id}...")
    result = subprocess.run(["gh", "run", "watch", str(run_id), "--repo", repo, "--exit-status"])
    if result.returncode != 0:
        raise RuntimeError(f"Debian workflow run {run_id} failed")

    with tempfile.TemporaryDirectory(prefix="markerup-debian-") as download_dir:
        print("Downloading the Debian package artifact from GitHub...")
        gh(
            "run",
            "download",
            str(run_id),
            "--repo",
            repo,
            "--name",
            "markerup-debian",
            "--dir",
            download_dir,
        )
        packages = sorted(Path(download_dir).rglob("*.deb"))
        if len(packages) != 1:
            raise RuntimeError(f"Expected one Debian package, found {len(packages)}")
        package = packages[0]
        print(f"Installing {package} locally...")
        install = subprocess.run(["sudo", "dpkg", "-i", str(package)])
        if install.returncode != 0:
            print("dpkg reported missing dependencies; asking apt to fix them...")
            subprocess.run(["sudo", "apt-get", "install", "-f", "-y"], check=True)
    print("Debian package installed successfully.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, KeyboardInterrupt) as error:
        print(f"Debian release failed: {error}", file=sys.stderr)
        raise SystemExit(1)
