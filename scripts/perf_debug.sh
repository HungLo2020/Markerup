#!/usr/bin/env bash
set -euo pipefail

# Run a repeatable debug-build workload and collect process-level CPU, RSS,
# thread-count, worker timing, and UI-cadence data. This script does not alter
# the application or the user's workspace; each run uses a temporary session
# and workspace that are removed on exit.

usage() {
    echo "usage: $0 [small|medium|large] [seconds] [output-directory]" >&2
    exit 2
}

scenario=${1:-medium}
duration=${2:-15}
output_root=${3:-perf-results}

case "$scenario" in
    small)  files=100;  depth=2;  body_lines=20  ;;
    medium) files=1000; depth=4;  body_lines=40  ;;
    large)  files=5000; depth=6;  body_lines=80  ;;
    *) usage ;;
esac

if ! [[ "$duration" =~ ^[0-9]+$ ]] || [ "$duration" -lt 1 ]; then usage; fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
binary="$repo_root/target/debug/markerup"
if [ ! -x "$binary" ]; then
    echo "debug binary not found; run 'cargo build' first" >&2
    exit 1
fi

run_id="$(date +%Y%m%d-%H%M%S)-$$-$scenario"
result_dir="$output_root/$run_id"
fixture_dir=$(mktemp -d "${TMPDIR:-/tmp}/markerup-perf-fixture.XXXXXX")
config_dir=$(mktemp -d "${TMPDIR:-/tmp}/markerup-perf-config.XXXXXX")
mkdir -p "$result_dir" "$config_dir/markerup"

cleanup() {
    if [ -n "${perf_pid:-}" ] && kill -0 "$perf_pid" 2>/dev/null; then
        kill "$perf_pid" 2>/dev/null || true
        wait "$perf_pid" 2>/dev/null || true
    fi
    if [ -n "${app_pid:-}" ] && kill -0 "$app_pid" 2>/dev/null; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    fi
    rm -rf "$fixture_dir" "$config_dir"
}
trap cleanup EXIT INT TERM

for index in $(seq 1 "$files"); do
    branch=$(( (index - 1) % depth ))
    directory="$fixture_dir/section-$branch"
    mkdir -p "$directory"
    note="$directory/note-$(printf '%05d' "$index").md"
    {
        echo "# Performance note $index"
        echo
        for line in $(seq 1 "$body_lines"); do
            echo "This is benchmark content for note $index, line $line. searchable-token-$((index % 17))"
        done
        if [ "$index" -eq 1 ]; then
            echo
            echo '```mermaid'
            echo 'flowchart TD'
            echo '    Start[Start] --> Done[Done]'
            echo '```'
        fi
    } > "$note"
done

restore_file=${MARKERUP_PERF_RESTORE_FILE:-}
printf 'markerup-session-v2\n%s\n%s\n' "$fixture_dir" "$restore_file" > "$config_dir/markerup/session"

{
    echo "scenario=$scenario"
    echo "files=$files"
    echo "depth=$depth"
    echo "body_lines=$body_lines"
    echo "fixture=$fixture_dir"
    echo "duration_seconds=$duration"
} > "$result_dir/metadata.txt"

MARKERUP_PERF=1 \
MARKERUP_PERF_SEARCH_QUERY="${MARKERUP_PERF_SEARCH_QUERY:-}" \
XDG_CONFIG_HOME="$config_dir" "$binary" > "$result_dir/application.log" 2>&1 &
app_pid=$!
echo "$app_pid" > "$result_dir/pid.txt"

perf_pid=
if command -v perf >/dev/null 2>&1; then
    # Attach to the exact process being sampled. This avoids running a second
    # application instance and keeps perf counters aligned with process.csv.
    perf stat -p "$app_pid" \
        -e task-clock,context-switches,cpu-migrations,page-faults \
        > "$result_dir/perf-stat.txt" 2>&1 &
    perf_pid=$!
fi

printf 'timestamp_ms,cpu_percent,rss_kb,threads\n' > "$result_dir/process.csv"
previous_wall=$(date +%s%N)
previous_cpu=0
end_wall=$((previous_wall + duration * 1000000000))
while kill -0 "$app_pid" 2>/dev/null; do
    now_wall=$(date +%s%N)
    if [ "$now_wall" -ge "$end_wall" ]; then break; fi
    stat_line=$(sed -n '1p' "/proc/$app_pid/stat" 2>/dev/null || true)
    status_file="/proc/$app_pid/status"
    if [ -z "$stat_line" ] || [ ! -r "$status_file" ]; then break; fi

    # /proc/<pid>/stat fields 14 and 15 are user/system CPU ticks. The
    # command name is parenthesized, so extract from the closing parenthesis.
    cpu_ticks=$(awk '{sub(/^.*\) /, ""); print $12 + $13}' <<< "$stat_line")
    rss_kb=$(awk '/^VmRSS:/ {print $2}' "$status_file")
    threads=$(awk '/^Threads:/ {print $2}' "$status_file")
    wall_delta=$((now_wall - previous_wall))
    cpu_delta=$((cpu_ticks - previous_cpu))
    cpu_percent=$(awk -v c="$cpu_delta" -v w="$wall_delta" 'BEGIN { if (w > 0) printf "%.2f", (c / 100.0) / (w / 1000000000.0) * 100.0; else print "0.00" }')
    printf '%s,%s,%s,%s\n' "$((now_wall / 1000000))" "$cpu_percent" "${rss_kb:-0}" "${threads:-0}" >> "$result_dir/process.csv"
    previous_wall=$now_wall
    previous_cpu=$cpu_ticks
    sleep 0.10
done

if kill -0 "$app_pid" 2>/dev/null; then
    kill "$app_pid" 2>/dev/null || true
fi
wait "$app_pid" 2>/dev/null || true
app_pid=
if [ -n "${perf_pid:-}" ]; then
    wait "$perf_pid" 2>/dev/null || true
    perf_pid=
fi

awk -F, '
    NR == 1 { next }
    { cpu += $2; if ($2 > max_cpu) max_cpu = $2; rss_sum += $3; if ($3 > max_rss) max_rss = $3; samples++ }
    END {
        if (samples == 0) exit 0;
        printf "samples=%d\navg_cpu_percent=%.2f\nmax_cpu_percent=%.2f\navg_rss_kb=%.0f\nmax_rss_kb=%d\n", samples, cpu / samples, max_cpu, rss_sum / samples, max_rss;
    }
' "$result_dir/process.csv" > "$result_dir/summary.txt"

cat "$result_dir/summary.txt"
echo "results=$result_dir"
