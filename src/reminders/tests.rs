use super::*;
use crate::workspace::LocalWorkspace;
use schedule::{parse, parse_spec};
fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}
fn next(spec: &str, after: &str) -> String {
    parse_spec(spec)
        .unwrap()
        .0
        .next_after(utc(after))
        .unwrap()
        .to_rfc3339()
}

#[test]
fn recurrence_handles_month_lengths_leap_years_and_intervals() {
    assert_eq!(
        next(
            "2026-01-31 09:00; tz=UTC; repeat=monthly",
            "2026-01-31T09:00:00Z"
        ),
        "2026-03-31T09:00:00+00:00"
    );
    assert_eq!(
        next(
            "2026-01-01 09:00; tz=UTC; repeat=monthly; day=last",
            "2026-01-31T09:00:00Z"
        ),
        "2026-02-28T09:00:00+00:00"
    );
    assert_eq!(
        next(
            "2024-02-29 09:00; tz=UTC; repeat=yearly",
            "2025-01-01T00:00:00Z"
        ),
        "2028-02-29T09:00:00+00:00"
    );
    assert_eq!(
        next(
            "2026-09-28 09:00; tz=UTC; repeat=weekly; every=2; weekdays=Mon,Wed",
            "2026-09-30T09:00:00Z"
        ),
        "2026-10-12T09:00:00+00:00"
    );
    assert_eq!(
        next(
            "2026-01-15 09:00; tz=UTC; repeat=monthly; every=3",
            "2026-01-15T09:00:00Z"
        ),
        "2026-04-15T09:00:00+00:00"
    );
}
#[test]
fn dst_gap_shifts_forward_and_fold_fires_once() {
    assert_eq!(
        next(
            "2026-03-08 02:30; tz=America/Los_Angeles; repeat=daily",
            "2026-03-08T00:00:00Z"
        ),
        "2026-03-08T10:30:00+00:00"
    );
    assert_eq!(
        next(
            "2026-11-01 01:30; tz=America/Los_Angeles; repeat=daily",
            "2026-11-01T00:00:00Z"
        ),
        "2026-11-01T08:30:00+00:00"
    );
    assert_eq!(
        next(
            "2026-11-01 01:30; tz=America/Los_Angeles; repeat=daily",
            "2026-11-01T08:30:00Z"
        ),
        "2026-11-02T09:30:00+00:00"
    );
}
#[test]
fn parser_respects_markdown_context_completion_and_unicode() {
    let marker = "@remind(2026-10-02 09:00; tz=UTC)";
    let source = format!(
        "```md\n{marker}\n```\n\n`{marker}`\n\n\\{marker}\n\n- [x] Done {marker}\n  - Nested {marker}\n\n日本語 😀 Call {marker}\n"
    );
    let parsed = parse(&source);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(parsed.reminders.len(), 1, "{parsed:?}");
    assert_eq!(parsed.reminders[0].offset, source.rfind("@remind").unwrap());
    assert_eq!(parsed.reminders[0].title, "日本語 😀 Call");
}
#[test]
fn rejects_bad_dates_duplicate_fields_and_exhausted_schedules() {
    for spec in [
        "2026-02-30 09:00; tz=UTC",
        "2026-01-01 09:00; tz=NoSuchZone",
        "2026-01-01 09:00; tz=UTC; every=0",
        "2026-01-01 09:00; tz=UTC; tz=UTC",
    ] {
        assert!(parse_spec(spec).is_err());
    }
    assert!(
        parse_spec("2026-01-01 09:00; tz=UTC; repeat=daily; until=2026-01-02")
            .unwrap()
            .0
            .next_after(utc("2026-01-03T00:00:00Z"))
            .is_none()
    );
}
#[test]
fn incremental_index_tracks_empty_notes_changes_deletions_and_preserves_on_failure() {
    let root =
        std::env::temp_dir().join(format!("markerup-reminder-test-{}", uuid::Uuid::new_v4()));
    let notes = root.join("notes");
    let index = root.join("index");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join("a.md"), "No reminders").unwrap();
    let workspace: WorkspaceRef = std::sync::Arc::new(LocalWorkspace::open(&notes).unwrap());
    scan(&workspace, &index, false).unwrap();
    let first = transaction_at(&index, false, |s| {
        Ok(s.workspaces[&workspace.identity()].notes["a.md"].checked_at)
    })
    .unwrap();
    scan(&workspace, &index, false).unwrap();
    assert_eq!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .notes["a.md"]
            .checked_at))
        .unwrap(),
        first
    );
    fs::write(
        notes.join("a.md"),
        "Call @remind(2027-10-02 09:00; tz=UTC; id=call)",
    )
    .unwrap();
    scan(&workspace, &index, false).unwrap();
    assert_eq!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .notes["a.md"]
            .parsed
            .reminders
            .len()))
        .unwrap(),
        1
    );
    fs::rename(&notes, root.join("offline")).unwrap();
    assert!(scan(&workspace, &index, false).is_err());
    assert_eq!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .notes
            .len()))
        .unwrap(),
        1
    );
    fs::rename(root.join("offline"), &notes).unwrap();
    fs::remove_file(notes.join("a.md")).unwrap();
    scan(&workspace, &index, false).unwrap();
    assert!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .notes
            .is_empty()))
        .unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn delivery_ledger_prevents_repeated_alerts_but_rescheduling_rearms() {
    let mut store = Store::default();
    let mut workspace = WorkspaceIndex::default();
    workspace.notes.insert(
        "note.md".into(),
        NoteIndex {
            parsed: parse("Task @remind(2026-10-02 09:00; tz=UTC; repeat=daily; id=task)"),
            ..Default::default()
        },
    );
    store.workspaces.insert("workspace".into(), workspace);
    let now = utc("2026-10-02T09:00:01Z");
    let due = deliveries(&store, now);
    assert_eq!(due.len(), 1);
    store.cursors.insert(due[0].key.clone(), now.timestamp());
    assert!(deliveries(&store, now).is_empty());
    assert_eq!(deliveries(&store, utc("2026-10-03T09:00:01Z")).len(), 1);
    store
        .workspaces
        .get_mut("workspace")
        .unwrap()
        .notes
        .get_mut("note.md")
        .unwrap()
        .parsed = parse("Task @remind(2026-10-02 09:00; tz=UTC; id=task)");
    assert_eq!(deliveries(&store, now).len(), 1);
}

fn indexed(source: &str) -> Store {
    let mut store = Store::default();
    let mut workspace = WorkspaceIndex::default();
    workspace.notes.insert(
        "note.md".into(),
        NoteIndex {
            parsed: parse(source),
            ..Default::default()
        },
    );
    store.workspaces.insert("workspace".into(), workspace);
    store
}

#[test]
fn ios_generates_native_weekday_triggers_for_fixed_zones_and_exact_dst_triggers() {
    let now = utc("2026-03-07T00:00:00Z");
    let store =
        indexed("Call @remind(2026-01-01 09:15; tz=UTC; repeat=weekly; weekdays=Mon; id=call)");
    let (requests, until, _) = platform::ios_requests(&store, now);
    assert_eq!(requests.len(), 1);
    assert!(until.is_none());
    assert_eq!(
        requests
            .iter()
            .map(|r| r["calendar"]["weekday"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(requests[0]["calendar"]["hour"], 9);
    let store =
        indexed("Call @remind(2026-03-07 02:30; tz=America/Los_Angeles; repeat=daily; id=call)");
    let (requests, until, overflow) = platform::ios_requests(&store, now);
    assert_eq!(requests.len(), 59);
    assert_eq!(requests[1]["at"], utc("2026-03-08T10:30:00Z").timestamp());
    assert_eq!(requests[2]["at"], utc("2026-03-09T09:30:00Z").timestamp());
    assert!(overflow);
    assert_eq!(until, Some("2026-05-05T09:30:00+00:00".into()));
}

#[test]
fn ios_queue_is_globally_bounded_and_completion_removes_requests() {
    let source = (0..70)
        .map(|i| format!("- [ ] Task {i} @remind(2026-10-02 09:00; tz=UTC; id=task-{i})\n"))
        .collect::<String>();
    let store = indexed(&source);
    let now = utc("2026-10-01T00:00:00Z");
    let (requests, until, overflow) = platform::ios_requests(&store, now);
    assert_eq!(requests.len(), 59);
    assert!(overflow && until.is_some());
    let done = indexed(&source.replace("[ ]", "[x]"));
    assert!(platform::ios_requests(&done, now).0.is_empty());
    assert!(deliveries(&done, utc("2026-10-02T09:01:00Z")).is_empty());
    // A finite schedule that fits entirely needs no replenish warning.
    let finite =
        indexed("Call @remind(2026-10-01 09:00; tz=UTC; repeat=daily; every=2; until=2026-10-05)");
    let (requests, until, overflow) = platform::ios_requests(&finite, now);
    assert_eq!(requests.len(), 3);
    assert!(until.is_none() && !overflow);
}

#[test]
fn index_skips_unchanged_content_and_force_scan_reconciles_it() {
    let root =
        std::env::temp_dir().join(format!("markerup-reminder-test-{}", uuid::Uuid::new_v4()));
    let notes = root.join("notes");
    let index = root.join("index");
    fs::create_dir_all(&notes).unwrap();
    let path = notes.join("a.md");
    fs::write(&path, "No reminders").unwrap();
    let workspace: WorkspaceRef = std::sync::Arc::new(LocalWorkspace::open(&notes).unwrap());
    scan(&workspace, &index, false).unwrap();
    // Keep metadata identical while replacing contents with invalid UTF-8. An
    // accidental read of unchanged notes would fail this second scan.
    let metadata = fs::metadata(&path).unwrap();
    fs::write(&path, vec![0xff; metadata.len() as usize]).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    scan(&workspace, &index, false).unwrap();
    scan(&workspace, &index, true).unwrap();
    assert!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .error
            .is_some()))
        .unwrap()
    );
    assert_eq!(
        transaction_at(&index, false, |s| Ok(s.workspaces[&workspace.identity()]
            .notes
            .len()))
        .unwrap(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_is_stable_across_moves_and_duplicate_ids_are_not_double_delivered() {
    let mut store = indexed("Call @remind(2026-10-02 09:00; tz=UTC; id=call)");
    let now = utc("2026-10-02T09:00:01Z");
    let first = deliveries(&store, now);
    let workspace = store.workspaces.get_mut("workspace").unwrap();
    let note = workspace.notes.remove("note.md").unwrap();
    workspace.notes.insert("moved.md".into(), note.clone());
    workspace.notes.insert("copy.md".into(), note);
    let moved = deliveries(&store, now);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].key, first[0].key);
    store.cursors.insert(first[0].key.clone(), now.timestamp());
    assert!(deliveries(&store, now).is_empty());
}

#[test]
fn parser_ignores_frontmatter_html_images_and_reports_invalid_markers() {
    let marker = "@remind(2026-10-02 09:00; tz=UTC)";
    let source = format!(
        "---\ntitle: {marker}\n---\n\n<!-- {marker} -->\n\n![{marker}](image.png)\n\nBad @remind(nonsense)\n"
    );
    let parsed = parse(&source);
    assert!(parsed.reminders.is_empty());
    assert_eq!(parsed.errors.len(), 1);
}

#[test]
fn ios_reconciles_coverage_only_after_native_confirmation_and_summarizes_missed() {
    let directory =
        std::env::temp_dir().join(format!("markerup-ios-test-{}", uuid::Uuid::new_v4()));
    let source = "Past @remind(2026-09-30 09:00; tz=UTC; id=past)\n\nFuture @remind(2026-10-02 09:00; tz=UTC; id=future)";
    transaction_at(&directory, true, |s| {
        *s = indexed(source);
        Ok(())
    })
    .unwrap();
    let now = utc("2026-10-01T09:00:00Z");
    let denied = platform::synchronize_ios_at(&directory, now, |_| {
        Ok(platform::PlatformStatus {
            message: "Notifications not allowed".into(),
            limited_until: None,
        })
    })
    .unwrap();
    assert!(denied.message.contains("not allowed"));
    assert!(
        transaction_at(&directory, false, |s| Ok(
            s.ios_scheduled.is_empty() && s.cursors.is_empty()
        ))
        .unwrap()
    );
    assert!(
        platform::synchronize_ios_at(&directory, now, |_| Err("Native failure".into())).is_err()
    );
    platform::synchronize_ios_at(&directory, now, |json| {
        let args: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(args["requests"].as_array().unwrap().len(), 2);
        assert_eq!(args["requests"][1]["id"], "markerup:overdue");
        assert_eq!(args["activeKeys"].as_array().unwrap().len(), 2);
        Ok(platform::PlatformStatus {
            message: "2 iOS notification schedules active".into(),
            limited_until: None,
        })
    })
    .unwrap();
    assert_eq!(
        transaction_at(&directory, false, |s| Ok(s.ios_scheduled.len())).unwrap(),
        1
    );
    // After delivery time, reopening does not replay either scheduled or summarized alerts.
    platform::synchronize_ios_at(&directory, utc("2026-10-03T00:00:00Z"), |json| {
        let args: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(args["requests"].as_array().unwrap().is_empty());
        Ok(platform::PlatformStatus {
            message: "0 iOS notification schedules active".into(),
            limited_until: None,
        })
    })
    .unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_moves_and_deletes_reconcile_only_the_affected_subtree() {
    let mut index = WorkspaceIndex::default();
    for name in ["games/a.md", "games/nested/b.md", "games2/c.md"] {
        index.notes.insert(name.into(), NoteIndex::default());
    }
    update_path(&mut index, "games", Some("archive"));
    assert!(index.notes.contains_key("archive/a.md"));
    assert!(index.notes.contains_key("archive/nested/b.md"));
    assert!(index.notes.contains_key("games2/c.md"));
    assert_eq!(index.generation, 1);
    update_path(&mut index, "archive", None);
    assert_eq!(index.notes.keys().collect::<Vec<_>>(), vec!["games2/c.md"]);
    assert_eq!(index.generation, 2);
}

fn test_dir() -> PathBuf {
    std::env::temp_dir().join(format!("markerup-regression-{}", uuid::Uuid::new_v4()))
}
fn ios_sync(directory: &std::path::Path, now: &str) -> serde_json::Value {
    let mut payload = serde_json::Value::Null;
    platform::synchronize_ios_at(directory, utc(now), |json| {
        payload = serde_json::from_str(json).unwrap();
        Ok(platform::PlatformStatus {
            message: "iOS schedules active".into(),
            limited_until: None,
        })
    })
    .unwrap();
    payload
}
#[test]
fn ios_indefinite_coverage_and_completed_oneoffs_never_replay() {
    let dir = test_dir();
    let source = "- [ ] Daily @remind(2026-10-01 09:00; tz=UTC; repeat=daily; id=daily)\n\n- [ ] Once @remind(2026-10-02 09:00; tz=UTC; id=once)";
    transaction_at(&dir, true, |s| {
        *s = indexed(source);
        Ok(())
    })
    .unwrap();
    ios_sync(&dir, "2026-10-01T10:00:00Z");
    for time in ["2026-10-02T10:00:00Z", "2026-10-03T10:00:00Z"] {
        let payload = ios_sync(&dir, time);
        assert!(
            payload["requests"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["key"] != "overdue")
        );
    }
    transaction_at(&dir, true, |s| {
        s.workspaces = indexed(&source.replace("[ ]", "[x]")).workspaces;
        Ok(())
    })
    .unwrap();
    assert!(
        ios_sync(&dir, "2026-10-03T11:00:00Z")["requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    transaction_at(&dir, true, |s| {
        s.workspaces = indexed(source).workspaces;
        Ok(())
    })
    .unwrap();
    let payload = ios_sync(&dir, "2026-10-03T12:00:00Z");
    assert_eq!(payload["requests"].as_array().unwrap().len(), 1);
    assert!(payload["requests"][0]["calendar"].is_object());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn ios_cancel_before_due_can_rearm_and_refill_warning_is_stable() {
    let dir = test_dir();
    let source = "- [ ] Once @remind(2026-10-02 09:00; tz=UTC; id=once)";
    transaction_at(&dir, true, |s| {
        *s = indexed(source);
        Ok(())
    })
    .unwrap();
    ios_sync(&dir, "2026-10-01T00:00:00Z");
    transaction_at(&dir, true, |s| {
        s.workspaces = indexed("").workspaces;
        Ok(())
    })
    .unwrap();
    ios_sync(&dir, "2026-10-01T01:00:00Z");
    transaction_at(&dir, true, |s| {
        s.workspaces = indexed(source).workspaces;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        ios_sync(&dir, "2026-10-01T02:00:00Z")["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    transaction_at(&dir, true, |s| {
        *s = indexed("Daily @remind(2026-10-01 09:00; tz=America/Los_Angeles; repeat=daily)");
        Ok(())
    })
    .unwrap();
    let first = ios_sync(&dir, "2026-10-01T00:00:00Z");
    let second = ios_sync(&dir, "2026-10-01T00:01:00Z");
    let warning = |v: serde_json::Value| {
        v["requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == "refill")
            .unwrap()
            .clone()
    };
    assert_eq!(warning(first), warning(second));
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn index_migrates_infinite_coverage_and_recovers_after_corruption() {
    let dir = test_dir();
    fs::create_dir_all(&dir).unwrap();
    let mut old = indexed("");
    old.version = 1;
    old.ios_coverage.insert("key".into(), i64::MAX);
    fs::write(dir.join("index.json"), serde_json::to_vec(&old).unwrap()).unwrap();
    transaction_at(&dir, false, |s| {
        assert!(matches!(s.ios_scheduled["key"], Coverage::Indefinite));
        Ok(())
    })
    .unwrap();
    fs::write(dir.join("index.json"), "broken").unwrap();
    assert!(transaction_at(&dir, false, |_| Ok(())).is_err());
    fs::write(dir.join("index.json"), serde_json::to_vec(&old).unwrap()).unwrap();
    transaction_at(&dir, true, |s| {
        assert!(s.ios_coverage.is_empty());
        Ok(())
    })
    .unwrap();
    let result: Store = serde_json::from_slice(&fs::read(dir.join("index.json")).unwrap()).unwrap();
    assert_eq!(result.version, 3);
    assert!(
        transaction_at::<()>(&dir, true, |s| {
            s.workspaces.clear();
            Err(io::Error::other("abort"))
        })
        .is_err()
    );
    assert_eq!(
        transaction_at(&dir, false, |s| Ok(s.workspaces.len())).unwrap(),
        1
    );
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unreadable_note_does_not_block_completion_deletion_or_discovery() {
    let root = test_dir();
    let notes = root.join("notes");
    let dir = root.join("index");
    fs::create_dir_all(&notes).unwrap();
    let source = "- [ ] Call @remind(2026-10-02 09:00; tz=UTC)";
    for file in ["bad.md", "done.md", "deleted.md"] {
        fs::write(notes.join(file), source).unwrap();
    }
    let w: WorkspaceRef = std::sync::Arc::new(LocalWorkspace::open(&notes).unwrap());
    scan(&w, &dir, false).unwrap();
    fs::write(notes.join("bad.md"), [0xff]).unwrap();
    fs::write(notes.join("done.md"), source.replace("[ ]", "[x]")).unwrap();
    fs::remove_file(notes.join("deleted.md")).unwrap();
    fs::write(notes.join("new.md"), source).unwrap();
    scan(&w, &dir, true).unwrap();
    transaction_at(&dir, false, |s| {
        let index = &s.workspaces[&w.identity()];
        assert!(index.error.as_ref().unwrap().contains("bad.md"));
        assert_eq!(index.notes["bad.md"].parsed.reminders.len(), 1);
        assert!(index.notes["done.md"].parsed.reminders.is_empty());
        assert!(!index.notes.contains_key("deleted.md"));
        assert_eq!(index.notes["new.md"].parsed.reminders.len(), 1);
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn queued_saves_coalesce_and_respect_move_delete_and_forget_barriers() {
    let dir = test_dir();
    let source = "Call @remind(2026-10-02 09:00; tz=UTC)";
    let updates = VecDeque::from([
        IndexUpdate::Register("workspace".into()),
        IndexUpdate::Saved("workspace".into(), "old.md".into(), source.into()),
        IndexUpdate::Saved(
            "workspace".into(),
            "old.md".into(),
            source.replace("Call", "Latest"),
        ),
        IndexUpdate::Path("workspace".into(), "old.md".into(), Some("new.md".into())),
    ]);
    apply_updates(&dir, &updates).unwrap();
    transaction_at(&dir, false, |s| {
        let w = &s.workspaces["workspace"];
        assert_eq!(w.generation, 2); // One coalesced save and one move.
        assert!(!w.notes.contains_key("old.md"));
        assert_eq!(w.notes["new.md"].parsed.reminders[0].title, "Latest");
        Ok(())
    })
    .unwrap();
    apply_updates(
        &dir,
        &VecDeque::from([IndexUpdate::Path("workspace".into(), "new.md".into(), None)]),
    )
    .unwrap();
    transaction_at(&dir, true, |s| {
        assert!(s.workspaces["workspace"].notes.is_empty());
        s.workspaces.clear();
        s.forgotten.insert("workspace".into());
        Ok(())
    })
    .unwrap();
    apply_updates(&dir, &updates).unwrap();
    assert!(transaction_at(&dir, false, |s| Ok(s.workspaces.is_empty())).unwrap());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn pause_and_resume_skip_occurrences_during_pause_on_both_platforms() {
    let mut store = indexed("Call @remind(2026-10-02 09:00; tz=UTC; repeat=daily)");
    store.workspaces.get_mut("workspace").unwrap().paused = true;
    let now = utc("2026-10-03T10:00:00Z");
    assert!(deliveries(&store, now).is_empty());
    assert!(platform::ios_requests(&store, now).0.is_empty());
    let w = store.workspaces.get_mut("workspace").unwrap();
    w.paused = false;
    w.resume_after = now.timestamp();
    assert!(deliveries(&store, now).is_empty());
    assert_eq!(deliveries(&store, utc("2026-10-04T09:01:00Z")).len(), 1);
}
#[test]
fn unicode_marker_range_edits_only_marker_and_keeps_identity() {
    let source = "- [ ] 日本語 😀 Call @remind(2026-10-02 09:00; tz=UTC; id=stable) trailing text";
    let reminder = &parse(source).reminders[0];
    assert_eq!(
        &source[reminder.offset..reminder.end],
        "@remind(2026-10-02 09:00; tz=UTC; id=stable)"
    );
    let mut edited = source.to_string();
    edited.replace_range(
        reminder.offset..reminder.end,
        "@remind(2026-10-03 10:00; tz=UTC; id=stable)",
    );
    assert!(edited.starts_with("- [ ] 日本語 😀 Call "));
    assert!(edited.ends_with(" trailing text"));
    assert_eq!(parse(&edited).reminders[0].id, "stable");
}
#[test]
fn benchmark_large_index_cached_polling_and_coalesced_saves() {
    let dir = test_dir();
    let started = Instant::now();
    transaction_at(&dir, true, |s| {
        let w = s.workspaces.entry("workspace".into()).or_default();
        for i in 0..10_000 {
            w.notes.insert(format!("{i}.md"), NoteIndex::default());
        }
        Ok(())
    })
    .unwrap();
    let cold = started.elapsed();
    let started = Instant::now();
    for _ in 0..1000 {
        transaction_at(&dir, false, |s| {
            assert_eq!(s.workspaces["workspace"].notes.len(), 10_000);
            Ok(())
        })
        .unwrap();
    }
    let polling = started.elapsed();
    let started = Instant::now();
    let updates = (0..100)
        .map(|i| {
            IndexUpdate::Saved(
                "workspace".into(),
                "0.md".into(),
                format!("Save {i} @remind(2027-01-01 09:00; tz=UTC)"),
            )
        })
        .collect();
    apply_updates(&dir, &updates).unwrap();
    eprintln!(
        "10k-note index: initial write {cold:?}, 1000 cached reads {polling:?}, 100 coalesced saves {:?}",
        started.elapsed()
    );
    assert_eq!(
        transaction_at(&dir, false, |s| Ok(s.workspaces["workspace"].generation)).unwrap(),
        1
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ios_multiple_weekdays_have_finite_contiguous_coverage() {
    let source =
        "Call @remind(2026-01-01 09:15; tz=UTC; repeat=weekly; weekdays=Mon,Wed,Fri; id=call)";
    let store = indexed(source);
    let now = utc("2026-03-07T00:00:00Z");
    let (requests, deadline, overflow) = platform::ios_requests(&store, now);
    assert_eq!(requests.len(), 59);
    assert!(overflow && deadline.is_some());
    assert_eq!(requests[0]["at"], utc("2026-03-09T09:15:00Z").timestamp());
    assert_eq!(requests[1]["at"], utc("2026-03-11T09:15:00Z").timestamp());
    assert_eq!(requests[2]["at"], utc("2026-03-13T09:15:00Z").timestamp());
    assert!(requests.iter().all(|r| r["calendar"].is_null()));
}
