---
name: track-workflows
description: Follow Markerup's GitHub Actions workflows until every run finishes, then verify they all succeeded. Use when the user asks to track, watch, monitor, or babysit workflows, CI, or GitHub runs.
---

# Track GitHub workflows

Follow the workflows on GitHub until every relevant run has finished, and verify each one actually succeeded. Do not end the task while any relevant run is queued, in progress, or waiting on a chained workflow.

## Defaults and what changes them

| Behaviour | Default | Only when the user explicitly asks |
|---|---|---|
| Commits | Do not make, amend, or push commits. | Make and push commits (for example "commit and push", "push this to main"). |
| Failed runs | Report the cause of each failure, then stop. Do not edit files. | Fix the cause, then track the new runs again (for example "make sure they pass", "fix it until it's green"). |

Asking to *ensure* the workflows succeed allows fixing, but not committing. If the fix needs a commit and push and the user has not allowed that, make the fix locally, report it, and ask before committing.

## 1. Find the runs to track

1. Get the commit to follow: `git rev-parse HEAD`. If the user is asking about a commit they just pushed, check `git status -sb` to confirm `HEAD` matches the upstream branch.
2. Check what is already running or recently ran upstream before doing anything else:

   ```bash
   gh run list --limit 20 --json databaseId,workflowName,status,conclusion,headSha,event,createdAt
   ```

   Track the runs for the target commit that already exist. Do not push or re-run anything just to have something to watch.
3. Read `.github/workflows/*.yml` to learn what the commit should trigger. Include `workflow_run` chains, which start only after another workflow finishes. In this repository, **Release latest** follows **iOS build and simulator smoke test** on `main`. Also note `workflow_call` workflows that run as jobs inside another run, and `workflow_dispatch`-only workflows, which a push never starts.
4. If no run exists yet for a commit that was just pushed, wait briefly and list again. GitHub can take several seconds to create runs.

## 2. Wait without stopping

- Use a background watcher (the Monitor tool, or Bash with `run_in_background`) that polls `gh run list` or `gh run view <id>` every 30–60 seconds. It should emit a line whenever a run completes, with any conclusion, and exit only when every expected run has completed. That includes chained runs, which start only after their trigger finishes.
- A watcher that only reports success is wrong. It must report `failure`, `cancelled`, `timed_out`, `action_required`, and `startup_failure` too.
- When the watcher times out before the runs finish, re-arm it. Keep the user informed with short updates: which runs are done and which step each running job is on (`gh run view <id> --json jobs`).
- Waiting between notifications is not stopping. Do not describe the task as complete until the final verification in step 4.

## 3. When a run fails

1. Find the failing jobs and steps:

   ```bash
   gh run view <id> --json jobs --jq '.jobs[] | select(.conclusion != "success") | {name, conclusion, steps: [.steps[] | select(.conclusion == "failure") | .name]}'
   gh run view <id> --log-failed
   ```

2. **Default:** report the workflow, job, step, and the relevant error lines, and say what you think caused it. Do not change files, re-run jobs, or commit.
3. **If the user asked you to ensure success:**
   - Reproduce the failure locally where possible, using the same command the workflow step runs.
   - Fix the root cause. Do not skip, weaken, or delete tests or checks to get a green result.
   - If the failure looks flaky or infrastructure-related (runner outage, network timeout, rate limit), you may re-run it once with `gh run rerun <id> --failed` instead of changing code. Say that you did.
   - Commit and push the fix only if the user allowed commits. Otherwise stop and ask.
   - Return to step 1 for the new commit and track every run again.

## 4. Verify before reporting success

Check the final state directly instead of relying on the watcher's output:

```bash
gh run list --limit 20 --json databaseId,workflowName,status,conclusion,headSha --jq '.[] | select(.headSha == "<sha>")'
gh run view <id> --json jobs --jq '.jobs[] | "\(.name): \(.conclusion)"'
```

Report success only when every expected run for the commit, including chained ones, has `status` `completed` and `conclusion` `success`, and no job failed. A `skipped` job counts as fine only if the workflow skips it by design; say which jobs were skipped. Finish with a short table of each workflow and its result, and mention any outward effect the runs had, such as a published release or deployed site.
