# Performance testing

The debug profiling harness is [`scripts/perf_debug.sh`](../scripts/perf_debug.sh). It creates a temporary Markdown workspace, launches the existing debug binary with an isolated session, and records:

- process CPU percentage sampled from `/proc/<pid>/stat`;
- resident memory and thread count from `/proc/<pid>/status`;
- existing scan/search/preview worker timings from `MARKERUP_PERF=1`;
- UI timer cadence, including interval and effective cadence, from the same opt-in performance output;
- supplemental Linux `perf stat` counters when kernel permissions allow them.

The application-side changes keep search content caching bounded to 4 MiB, cancel stale search generations during filesystem walking and content matching, reuse the tree's Slint model, back off UI polling from 50 ms to 250 ms while idle, and initialize Merman only when a Mermaid block is actually rendered.

Build first, then run a scenario:

```text
cargo build
scripts/perf_debug.sh small 15
scripts/perf_debug.sh medium 30
scripts/perf_debug.sh large 30
```

The harness can also exercise search and preview work explicitly. The first generated note contains a Mermaid diagram; set the restored note and search query for a combined workload:

```text
MARKERUP_PERF_RESTORE_FILE=section-0/note-00001.md \
MARKERUP_PERF_SEARCH_QUERY=searchable-token-1 \
scripts/perf_debug.sh medium 30
```

Without those environment variables, the run measures workspace startup and idle behavior only.

Each run writes results under `perf-results/` unless a third argument supplies another output directory. The output includes `process.csv`, `application.log`, `summary.txt`, and optional `perf-stat.txt`.

The scenarios contain 100, 1,000, and 5,000 Markdown notes. They are intended to establish startup/idle baselines and expose scaling behavior in recursive scanning, search-index construction, and UI update handling. Re-run each scenario at least three times and compare medians rather than a single run.

## Interpreting frame data

Markerup currently uses a 50 ms Slint timer to poll filesystem and worker events. The reported `effective_fps` is therefore UI timer cadence, not compositor-presented FPS. It is useful for detecting event-loop stalls, but true rendered FPS should be measured with platform tooling: Instruments/Core Animation on iOS, and a compositor or GPU profiler on Linux. No release-mode settings are used by this harness.

For a fair comparison, close other Markerup instances, keep the display/compositor configuration fixed, and record the OS, device, Rust version, scenario, and whether `perf-stat.txt` was permitted.
