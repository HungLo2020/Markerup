use super::*;

#[derive(Clone, Serialize)]
pub struct PlatformStatus {
    pub message: String,
    pub limited_until: Option<String>,
}

#[cfg(target_os = "linux")]
mod linux;

pub fn status() -> PlatformStatus {
    #[cfg(target_os = "linux")]
    return PlatformStatus {
        message: if linux::service_heartbeat() {
            if cfg!(debug_assertions) {
                "Shared Linux reminder service running (development and installed apps)"
            } else {
                "Linux reminder service running (including when the editor is closed)"
            }
        } else {
            "Linux reminder service is starting or unavailable"
        }
        .into(),
        limited_until: None,
    };
    #[cfg(target_os = "ios")]
    return IOS_STATUS
        .get_or_init(|| {
            Mutex::new(PlatformStatus {
                message: "Checking iOS notification permission…".into(),
                limited_until: None,
            })
        })
        .lock()
        .unwrap()
        .clone();
    #[cfg(not(any(target_os = "linux", target_os = "ios")))]
    PlatformStatus {
        message: "Reminder delivery is not supported on this platform yet".into(),
        limited_until: None,
    }
}

pub fn request_permission() -> Result<String, String> {
    #[cfg(target_os = "ios")]
    {
        let result = ios_call("{\"permission\":true}")?;
        IOS_SYNC.store(true, Ordering::Release);
        synchronize().map_err(|e| e.to_string())?;
        return Ok(result.message);
    }
    #[cfg(target_os = "linux")]
    {
        linux::ensure_service().map_err(|e| e.to_string())?;
        Ok("Notifications use your desktop notification settings".into())
    }
    #[cfg(not(any(target_os = "linux", target_os = "ios")))]
    Err("Reminder notifications are not implemented for this platform".into())
}

pub fn take_notification() -> Result<Option<String>, String> {
    #[cfg(target_os = "ios")]
    {
        return ios_call("{\"take\":true}").map(|s| (!s.message.is_empty()).then_some(s.message));
    }
    #[cfg(not(target_os = "ios"))]
    {
        Ok(NAVIGATION.lock().unwrap().take())
    }
}

pub fn synchronize() -> io::Result<()> {
    // Serialize native updates; saves and lifecycle flushes may arrive together.
    #[cfg(target_os = "ios")]
    static SYNC_LOCK: Mutex<()> = Mutex::new(());
    #[cfg(target_os = "ios")]
    let _guard = SYNC_LOCK
        .lock()
        .map_err(|_| io::Error::other("Reminder scheduler lock failed"))?;
    #[cfg(target_os = "linux")]
    {
        if transaction(false, |s| Ok(!s.workspaces.is_empty()))? {
            linux::ensure_service()?;
        }
    }
    #[cfg(target_os = "ios")]
    if IOS_SYNC.swap(false, Ordering::AcqRel) {
        match synchronize_ios_at(&data_dir(), Utc::now(), ios_call) {
            Ok(status) => {
                publish_health(&status);
                let state = IOS_STATUS.get_or_init(|| Mutex::new(status.clone()));
                *state.lock().unwrap() = status;
            }
            Err(error) => {
                IOS_SYNC.store(true, Ordering::Release);
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "ios", test))]
pub(super) fn synchronize_ios_at(
    directory: &std::path::Path,
    now: DateTime<Utc>,
    send: impl FnOnce(&str) -> Result<PlatformStatus, String>,
) -> io::Result<PlatformStatus> {
    let (requests, active_keys, overdue) = transaction_at(directory, false, |s| {
        let mut overdue_store = s.clone();
        for (key, covered) in &s.ios_scheduled {
            overdue_store
                .cursors
                .entry(key.clone())
                .and_modify(|c| *c = (*c).max(covered.through(now.timestamp())))
                .or_insert(covered.through(now.timestamp()));
        }
        let overdue = deliveries(&overdue_store, now);
        let keys = s
            .workspaces
            .iter()
            .filter(|(_, i)| !i.paused)
            .flat_map(|(w, i)| {
                i.notes
                    .values()
                    .flat_map(move |n| n.parsed.reminders.iter().map(move |r| delivery_key(w, r)))
            })
            .collect::<Vec<_>>();
        Ok((ios_requests(s, now), keys, overdue))
    })?;
    let deadline = requests
        .1
        .as_ref()
        .and_then(|t| t.parse::<DateTime<Utc>>().ok())
        .map(|t| t.timestamp());
    let refill = transaction_at(directory, false, |s| {
        Ok(deadline.map(|deadline| {
            s.ios_refill.filter(|(old, _)| *old == deadline).unwrap_or((
                deadline,
                (deadline - 3600).max(now.timestamp() + 5).min(deadline - 1),
            ))
        }))
    })?;
    let mut queue = requests.0.clone();
    if let Some((deadline, at)) = refill
        && at > now.timestamp()
    {
        queue.push(serde_json::json!({"id":"markerup:refill","key":"refill","title":"Open Markerup to refresh reminders","body":"Your queued reminders are running low. Open the app to schedule the next occurrences.","at":at,"deadline":deadline}));
    }
    if !overdue.is_empty() {
        queue.push(serde_json::json!({"id": "markerup:overdue", "key": "overdue", "title": format!("{} overdue reminder(s)", overdue.len()),
                "keys": overdue.iter().map(|d| d.key.clone()).collect::<Vec<_>>(),
                "body": overdue.iter().take(5).map(|d| d.title.clone()).collect::<Vec<_>>().join("\n"), "at": (now + Duration::seconds(5)).timestamp()}));
    }
    let json =
        serde_json::to_string(&serde_json::json!({"requests": queue, "activeKeys": active_keys}))
            .map_err(io::Error::other)?;
    match send(&json) {
        Ok(mut status) => {
            if status.message.contains("schedules active") {
                transaction_at(directory, true, |s| {
                    // Move elapsed scheduled coverage into durable history before
                    // replacing/cancelling schedules. Never turn infinity into a date.
                    for (key, coverage) in std::mem::take(&mut s.ios_scheduled) {
                        let through = coverage.through(now.timestamp());
                        s.cursors
                            .entry(key)
                            .and_modify(|t| *t = (*t).max(through))
                            .or_insert(through);
                    }
                    for request in &requests.0 {
                        if let Some(key) = request["key"].as_str() {
                            let coverage = request["at"]
                                .as_i64()
                                .map(Coverage::Through)
                                .unwrap_or(Coverage::Indefinite);
                            s.ios_scheduled
                                .entry(key.to_string())
                                .and_modify(|old| match (&*old, &coverage) {
                                    (Coverage::Through(a), Coverage::Through(b)) if a >= b => {}
                                    (Coverage::Indefinite, _) => {}
                                    _ => *old = coverage.clone(),
                                })
                                .or_insert(coverage);
                        }
                    }
                    s.ios_refill = refill;
                    for d in &overdue {
                        s.cursors.insert(d.key.clone(), now.timestamp());
                    }
                    Ok(())
                })?;
            }
            status.limited_until = requests.1;
            if requests.2 {
                status.message.push_str(" · iOS schedule capacity reached; open Markerup to replenish upcoming reminders");
            }
            Ok(status)
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run_service() -> io::Result<()> {
    linux::run_service()
}

#[cfg(target_os = "ios")]
static IOS_STATUS: OnceLock<Mutex<PlatformStatus>> = OnceLock::new();

#[cfg(target_os = "ios")]
fn ios_call(json: &str) -> Result<PlatformStatus, String> {
    use std::ffi::{CStr, CString, c_char};
    unsafe extern "C" {
        fn markerup_ios_reminders(json: *const c_char) -> *mut c_char;
        fn markerup_ios_free_string(value: *mut c_char);
    }
    let json = CString::new(json).map_err(|e| e.to_string())?;
    let result = unsafe { markerup_ios_reminders(json.as_ptr()) };
    if result.is_null() {
        return Err("iOS notification scheduler did not return a result".into());
    }
    let text = unsafe { CStr::from_ptr(result) }
        .to_string_lossy()
        .into_owned();
    unsafe { markerup_ios_free_string(result) };
    let response: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(error) = response.get("error").and_then(|v| v.as_str()) {
        return Err(error.into());
    }
    Ok(PlatformStatus {
        message: response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("iOS reminders updated")
            .into(),
        limited_until: None,
    })
}

// The native queue is bounded. Simple unbounded patterns use repeating calendar
// triggers; all other schedules use globally ordered upcoming occurrences.
#[cfg(any(target_os = "ios", test))]
pub(super) fn ios_requests(
    store: &Store,
    now: DateTime<Utc>,
) -> (Vec<serde_json::Value>, Option<String>, bool) {
    use chrono::Datelike;
    let mut requests: Vec<(DateTime<Utc>, serde_json::Value)> = Vec::new();
    for (workspace, index) in &store.workspaces {
        if index.paused {
            continue;
        }
        let mut ids = std::collections::HashSet::new();
        for (file, note) in &index.notes {
            for reminder in &note.parsed.reminders {
                if !ids.insert(reminder.id.clone()) {
                    continue;
                }
                let spec = &reminder.schedule;
                let Some(next) = spec.next_after(now) else {
                    continue;
                };
                let key = delivery_key(workspace, reminder);
                let zone: chrono_tz::Tz = spec.tz.parse().unwrap();
                // Exact UTC occurrences preserve our DST gap/fold rules. Native
                // indefinite calendar recurrence is safe in fixed-offset zones.
                let fixed_zone = spec.tz == "UTC" || spec.tz.starts_with("Etc/");
                let native = fixed_zone
                    && spec.repeat != Repeat::Once
                    && spec.every == 1
                    && (spec.repeat != Repeat::Weekly || spec.weekdays.len() == 1)
                    && spec.until.is_none()
                    && !spec.last_day
                    && spec.start <= now.with_timezone(&zone).naive_local();
                if native {
                    let days = if spec.repeat == Repeat::Weekly {
                        spec.weekdays.clone()
                    } else {
                        vec![0]
                    };
                    for weekday in days {
                        let mut components = serde_json::json!({"hour": spec.hour(), "minute": spec.minute(), "timeZone": spec.tz});
                        if weekday > 0 {
                            components["weekday"] = serde_json::json!(weekday % 7 + 1);
                        }
                        if matches!(spec.repeat, Repeat::Monthly | Repeat::Yearly) {
                            components["day"] = serde_json::json!(spec.start.day());
                        }
                        if spec.repeat == Repeat::Yearly {
                            components["month"] = serde_json::json!(spec.start.month());
                        }
                        let mut weekly = spec.clone();
                        if weekday > 0 {
                            weekly.weekdays = vec![weekday];
                        }
                        let next_day = weekly.next_after(now).unwrap_or(next);
                        requests.push((next_day, serde_json::json!({"id": format!("markerup:{key}:{weekday}"), "key": key, "title": reminder.title, "body": file, "calendar": components})));
                    }
                } else {
                    let mut occurrence = Some(next);
                    for _ in 0..61 {
                        let Some(at) = occurrence else {
                            break;
                        };
                        requests.push((at, serde_json::json!({"id": format!("markerup:{key}:{}", at.timestamp()), "key": key, "title": reminder.title, "body": file, "at": at.timestamp()})));
                        occurrence = spec.next_after(at);
                    }
                }
            }
        }
    }
    requests.sort_by_key(|r| r.0);
    let overflow = requests.len() > 59;
    // The earliest omitted occurrence is the deadline, across all series.
    // Generating 61 per series ensures even a lone daily reminder has a horizon.
    let until = requests.get(59).map(|r| r.0.to_rfc3339());
    requests.truncate(59);
    (requests.into_iter().map(|r| r.1).collect(), until, overflow)
}

static NAVIGATION: Mutex<Option<String>> = Mutex::new(None);
pub fn queue_navigation(args: &[String]) {
    if let Some(pair) = args.windows(2).find(|p| p[0] == "--reminder-open")
        && pair[1].len() <= 300
    {
        *NAVIGATION.lock().unwrap() = Some(pair[1].clone());
    }
}

#[cfg(target_os = "linux")]
pub(super) fn setup_service(remove: bool) -> io::Result<()> {
    linux::setup_service(remove)
}
