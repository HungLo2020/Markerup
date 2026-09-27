# Markdown reminders

Reminders are enabled in every workspace. Open a note and choose **Insert → Reminder** on desktop or iOS. The dialog provides text, date, time, time zone, repeat frequency, interval, weekdays, last day of month, and an optional end date. It inserts ordinary Markdown text and saves through the existing save/conflict protection.

**Settings → Reminders** lists saved reminders, next occurrences, scan errors, notification permission, and the iOS replenish deadline. Use **Refresh** after unusual external edits that preserve file timestamps, or to force reconciliation now. Search by reminder or note, filter by status, and use **Refresh** to rescan. Upcoming, paused, and past reminders are grouped separately. Notification preferences and workspace controls are in expandable sections beneath the list. Choose **Open note** to jump to a reminder or **Edit reminder** to change its schedule while preserving its ID and surrounding Markdown. **Insert → Edit reminder on this line** works directly from a note. Reminder text can be edited in the note.

Each workspace has **Pause reminders**, **Resume reminders**, and **Forget workspace** controls. Pausing cancels scheduling without changing any note. Resuming skips occurrences from the paused period. Forgetting removes the workspace’s cached schedules and prevents automatic rediscovery until you explicitly resume it; it leaves a small workspace identity record and delivery history so previously delivered alerts are not replayed.

## Syntax

```markdown
Call the dentist @remind(2026-10-02 09:30; tz=America/Los_Angeles; id=dentist)

- [ ] Water the plants @remind(2026-10-01 08:00; tz=America/Los_Angeles; repeat=daily; id=plants)

Team check-in @remind(2026-10-05 10:00; tz=America/Los_Angeles; repeat=weekly; every=2; weekdays=Mon,Wed; id=team)

Pay rent @remind(2026-10-01 09:00; tz=America/Los_Angeles; repeat=monthly; id=rent)

Monthly review @remind(2026-10-01 17:00; tz=America/Los_Angeles; repeat=monthly; day=last; id=review)

Anniversary @remind(2027-04-12 09:00; tz=America/Los_Angeles; repeat=yearly; until=2030-12-31; id=anniversary)
```

- `tz` is required: an IANA zone such as `America/Los_Angeles`, `Europe/London`, or `UTC`. The dialog defaults to the device's zone and writes it explicitly. Travel does not silently change the reminder's intended zone.
- `repeat` defaults to `once`; alternatives are `daily`, `weekly`, `monthly`, `yearly`. `every` is an integer from 1 to 100, default 1. Weeks start Monday and intervals are anchored to the start date's week.
- Weekly `weekdays` defaults to the start date's weekday. `day=last` works for monthly or yearly rules (yearly uses the selected month).
- Monthly day 29–31 skips months without that date. February 29 yearly reminders skip non-leap years. `until` is inclusive in the reminder's zone.
- Missing DST times shift forward by the gap (02:30 becomes 03:30 for a one-hour gap). Repeated DST times fire on the first occurrence only.
- `id` is optional for hand-written reminders. The dialog generates a unique ID. Retain it when moving/renaming notes. Duplicate IDs within a workspace are reported and only one copy is scheduled. Without an ID, the source line identifies the reminder; editing that line can re-arm it.
- A time-only recurring rule also works, e.g. `@remind(09:00; tz=UTC; repeat=daily)`. Use `start=YYYY-MM-DD` to anchor its interval; otherwise it has a historical anchor. Prefer the dialog's explicit dates.
- Reminders inside code, HTML comments/blocks, image descriptions, frontmatter, or completed task subtrees are ignored. Escape `\@remind(` to write a literal example.
- Checking a task cancels its reminder after the save succeeds. Unchecking restores the schedule; a previously delivered occurrence is not replayed. Dismissing a notification does not edit the note.
- The marker remains editable Markdown in the editor and preview; there is no separate reminder database that replaces the note's source.

## Notification delivery

### Linux / KDE

Installed release builds start a small headless `markerup --reminder-service` process. Where user systemd is available, Markerup installs `$XDG_CONFIG_HOME/systemd/user/markerup-reminders.service` with automatic restart on failure. Otherwise it installs an XDG Autostart entry at `$XDG_CONFIG_HOME/autostart/markerup-reminders.desktop` (normally `~/.config/autostart/`). It sends native `org.freedesktop.Notifications` messages over the desktop session bus. It continues after the editor closes and starts again at desktop login. Opening an updated release replaces an older running service. Debian installation and upgrades also run a headless setup command in active desktop sessions. A packaged XDG login bootstrap performs setup for other users at their next login. Setup stops the old worker before starting the new one; no editor window or Markdown file is opened. Development builds (`cargo run`) share the installed app’s reminder index, delivery history, and worker. They do not create development login entries. A standalone development worker runs for the current session only. Startup imports the former `markerup-dev/reminders` index, keeps the newest delivery cursors, and holds both old worker locks so legacy services cannot deliver concurrently. The old index is retained for recovery; unchanged legacy state is imported only once.

The desktop session must be running. The service does not wake a sleeping/off computer. Missed reminders are summarized when it runs again; a repeating reminder contributes at most one overdue item. Desktop notification settings / Do Not Disturb can suppress banners. A shared leader lock and the two legacy worker locks remain held throughout error retries and recovery. The worker retries index/disk errors with bounded backoff and records diagnostics in `service.error` and `service.log`. It reconciles local workspaces before delivering cached schedules at startup. Unavailable workspaces retain cached schedules with an error. Failed D-Bus delivery is reported and retried; successful delivery is recorded to avoid duplicates after restart. An abrupt crash between the desktop accepting a notification and recording it can still replay that notification.

The headless service can rescan previously opened local workspaces. SMB schedules already read by the editor remain available, but changed SMB files require the editor to reconnect. It stores no SMB credentials.

### iOS

Markerup uses `UNUserNotificationCenter`, not JavaScript timers. Allow notifications when prompted, or use **Settings → Reminders → Notification permission**. iOS owns the scheduled notifications and can deliver them while Markerup is suspended or closed. A notification tap opens its note when the workspace is accessible (summary and refill notifications open the list); **Snooze 10 minutes** schedules a local follow-up.

No reminder server, cross-device messaging, APNs registration, or network synchronization is involved. Open Markerup and the workspace on each device to read changes made elsewhere. Each device schedules independently, so both can alert. iOS Focus, notification settings, and OS scheduling policy still apply.

There is a finite iOS pending-notification queue. Markerup reserves up to 59 slots for normal reminders, one for a refill warning, one for an overdue summary, and up to three retained snoozes. Simple already-started rules in fixed-offset zones use indefinite native calendar repetition. Named-zone rules use exact UTC occurrences to preserve the same DST behavior as Linux; multiple-weekday/interval/end-date/last-day/future-start rules also use exact occurrences. Slots are allocated by the earliest upcoming occurrence across all notes. The reminder list shows the earliest occurrence that has not fit in the queue: **open Markerup before that deadline to replenish it**. For example, one daily reminder in a named zone normally has about 59 days queued, while many daily reminders have a shorter horizon. The app also shows the deadline in a persistent banner and schedules a local refill warning before coverage runs out. Notifications already scheduled do not require the workspace to remain accessible.

After a successful save, native schedules are reconciled. The iOS background-save completion also flushes reminder scheduling before ending the app's bounded background task. Permission denial and scheduling failures remain visible; they do not discard the Markdown text.

## Index and storage

The first background scan reads the workspace's Markdown files. It does not block window startup. Subsequent scans compare modification time and size and read only new/changed notes; notes with no reminders are cached too. SMB revision checks use directory metadata. External changes are checked about once a minute while accessible. A daily content recheck catches timestamp-preserving changes, and manual rescan bypasses the metadata cache. App saves enqueue confirmed disk contents. A worker coalesces saves and updates the index about every half second, outside the editor’s save lock. Normal exit and iOS background-save completion flush the queue. Unchanged index reads use an in-memory cache invalidated by atomic replacements from another process; unsaved drafts are never scheduled.

A failed directory scan retains the previous schedules and reports an error. An unreadable individual note retains only that note’s previous schedules; readable changes, completed tasks, deletions, and new notes still reconcile. A successful scan reconciles renames and deletions. Hidden backup/trash directories are excluded. The service does not edit Markdown files.

The per-device index and delivery ledger live under `$XDG_DATA_HOME/markerup/reminders` (normally `~/.local/share/markerup/reminders`) on Linux and `Library/Application Support/Markerup/reminders` in the iOS app container. They contain workspace paths, reminder text/schedules, file revision metadata, and delivery state. They use a process lock and atomic replacement. They are not placed in the workspace or synced across devices. Clearing this local state can cause overdue reminders to be shown again when notes are rediscovered. Do not remove it while the app or reminder service is running.

## Verification

```sh
cargo test --lib
cargo clippy --all-targets --locked -- -D warnings
npm --prefix frontend run build
npm --prefix frontend test
cargo build --locked
dbus-run-session -- /usr/bin/python3 DevUtils/test_reminder_notifications.py
cargo tauri build --bundles deb -- --locked
dbus-run-session -- /usr/bin/python3 DevUtils/test_reminder_package.py target/release/bundle/deb/*.deb
```

The D-Bus checks need `python3-dbus` and `python3-gi`. The notification test exercises the real headless binary and native D-Bus transport in temporary directories on a private bus, including delivery failure/retry, restart deduplication, completion, discovering a new reminder from Markdown, competing processes, and handoff from both legacy index locations. The package test extracts the actual Debian archive and runs its install/upgrade/removal hooks with isolated homes and stubbed OS session/systemd management, exercising both managed and autostart fallback workers. It does not install the package.

iOS needs macOS/Xcode and a device or simulator to verify the Objective-C bridge and OS behavior. On a device: create a reminder a few minutes ahead, allow notifications, background and then terminate Markerup, and confirm delivery. Reopen, test snooze, edit/delete/check the source task, and confirm obsolete requests no longer fire. Also deny/re-enable permission and check the displayed queue horizon with many recurring reminders. Rust tests exercise the iOS request generation, queue bounds, DST times, cancellation, native-error/permission-denial handling, and coverage ledger; those tests are not a substitute for an actual iOS delivery test.
