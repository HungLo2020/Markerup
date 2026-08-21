#!/usr/bin/env python3
"""Run the smoke-tested latest-release pipeline through GitHub Actions."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from datetime import datetime, timezone
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


def runs(repo: str, workflow: str) -> list[dict[str, Any]]:
    output = gh(
        "run",
        "list",
        "--repo",
        repo,
        "--workflow",
        workflow,
        "--branch",
        "main",
        "--limit",
        "20",
        "--json",
        "databaseId,headSha,event,status,conclusion,createdAt,updatedAt",
    )
    return json.loads(output or "[]")


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def wait_for_run(
    repo: str,
    workflow: str,
    started_after: datetime,
    event: str | None = None,
    head_sha: str | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + 30 * 60
    while time.monotonic() < deadline:
        candidates = [
            run
            for run in runs(repo, workflow)
            if parse_time(run["createdAt"]) >= started_after
            and (event is None or run["event"] == event)
            and (head_sha is None or run["headSha"] == head_sha)
        ]
        if candidates:
            return sorted(candidates, key=lambda run: run["createdAt"])[-1]
        time.sleep(5)
    raise RuntimeError(f"Timed out waiting for {workflow} run")


def watch(repo: str, run_id: int) -> None:
    result = subprocess.run(["gh", "run", "watch", str(run_id), "--repo", repo, "--exit-status"])
    if result.returncode != 0:
        raise RuntimeError(f"GitHub Actions run {run_id} failed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", help="GitHub repository, for example HungLo2020/Markerup")
    args = parser.parse_args()
    repo = args.repo or gh("repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner")

    source_sha = gh("api", f"repos/{repo}/git/ref/heads/main", "--jq", ".object.sha")
    started = datetime.now(timezone.utc)
    print("Dispatching the main-branch iOS simulator smoke test...")
    gh("workflow", "run", "ios.yml", "--repo", repo, "--ref", "main")
    smoke = wait_for_run(
        repo,
        "ios.yml",
        started,
        event="workflow_dispatch",
        head_sha=source_sha,
    )
    print(f"Watching smoke test run {smoke['databaseId']}...")
    watch(repo, int(smoke["databaseId"]))

    print("Smoke test passed. Waiting for the gated release workflow...")
    release = wait_for_run(
        repo,
        "release.yml",
        parse_time(smoke["createdAt"]),
        head_sha=smoke["headSha"],
    )
    watch(repo, int(release["databaseId"]))
    print(f"Latest release completed: https://github.com/{repo}/releases/tag/latest")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, KeyboardInterrupt) as error:
        print(f"release pipeline failed: {error}", file=sys.stderr)
        raise SystemExit(1)
