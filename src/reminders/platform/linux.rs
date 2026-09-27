use super::*;
use std::collections::HashSet;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;

mod ownership;
pub(super) use ownership::heartbeat as service_heartbeat;
use ownership::{executable_signature, heartbeat};

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
}
fn desktop_exec(path: &str) -> String {
    path.replace('\\', "\\\\\\\\")
        .replace('"', "\\\\\"")
        .replace('$', "\\\\$")
        .replace('`', "\\\\`")
        .replace('%', "%%")
}
fn register_autostart(executable: &std::path::Path) -> io::Result<bool> {
    // Development runs never replace the installed application's startup entry.
    if cfg!(debug_assertions) && executable != std::path::Path::new("/usr/bin/markerup") {
        return Ok(false);
    }
    let path = executable.to_string_lossy();
    if path.contains(['\n', '\r']) {
        return Err(io::Error::other("Unsupported newline in application path"));
    }
    let config = config_dir();
    let managed = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if managed {
        let directory = config.join("systemd/user");
        fs::create_dir_all(&directory)?;
        let escaped = path
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        fs::write(
            directory.join("markerup-reminders.service"),
            format!(
                "[Unit]\nDescription=Markerup reminders\nAfter=graphical-session.target\n[Service]\nExecStart=\"{escaped}\" --reminder-service\nRestart=on-failure\nRestartSec=5\n[Install]\nWantedBy=default.target\n"
            ),
        )?;
        let reload = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()?;
        if reload.success()
            && Command::new("systemctl")
                .args(["--user", "enable", "markerup-reminders.service"])
                .status()?
                .success()
        {
            if !Command::new("systemctl")
                .args(["--user", "restart", "markerup-reminders.service"])
                .status()?
                .success()
            {
                return Err(io::Error::other(
                    "Could not restart the reminder user service",
                ));
            }
            let _ = fs::remove_file(config.join("autostart/markerup-reminders.desktop"));
            return Ok(true);
        }
    }
    fs::create_dir_all(config.join("autostart"))?;
    fs::write(
        config.join("autostart/markerup-reminders.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Markerup reminders\nExec=\"{}\" --reminder-service\nTerminal=false\nNoDisplay=true\n",
            desktop_exec(&path)
        ),
    )?;
    Ok(false)
}
pub(super) fn ensure_service() -> io::Result<()> {
    static LAST_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);
    if heartbeat() {
        return Ok(());
    }
    let mut last = LAST_ATTEMPT.lock().unwrap();
    if last.is_some_and(|t| t.elapsed().as_secs() < 15) {
        return Ok(());
    }
    *last = Some(Instant::now());
    configure(false, false)
}

pub(super) fn setup_service(remove: bool) -> io::Result<()> {
    configure(true, remove)
}

fn configure(force: bool, remove: bool) -> io::Result<()> {
    fs::create_dir_all(data_dir())?;
    let setup = ownership::lock_file(&data_dir().join("setup.lock"))?;
    setup.lock_exclusive()?;
    if !force && heartbeat() {
        return Ok(());
    }
    // Prefer the packaged executable when a Debian installation is present.
    let installed = std::path::Path::new("/usr/share/markerup/reminders/service-version").is_file();
    let executable = if installed && (!force || cfg!(debug_assertions)) {
        PathBuf::from("/usr/bin/markerup")
    } else {
        std::env::current_exe()?
    };
    let config = config_dir();
    // Finish recording in-flight deliveries before systemd can send SIGTERM.
    ownership::stop_and_wait()?;
    if force
        || config
            .join("systemd/user/markerup-reminders.service")
            .exists()
    {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", "markerup-reminders.service"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    if remove {
        let _ = fs::remove_file(data_dir().join("service.desired.json"));
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "markerup-reminders.service"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for path in [
            "systemd/user/markerup-reminders.service",
            "autostart/markerup-reminders.desktop",
        ] {
            match fs::remove_file(config.join(path)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return Ok(());
    }
    // Development binaries never write login entries. A packaged install owns
    // startup; a standalone release can still register itself.
    fs::write(
        data_dir().join("service.desired.json"),
        serde_json::to_vec(&executable).map_err(io::Error::other)?,
    )?;
    let managed = register_autostart(&executable)?;
    if !managed {
        // Retire login entries written by pre-isolation development builds.
        if cfg!(debug_assertions) {
            let path = config.join("autostart/markerup-reminders.desktop");
            if fs::read_to_string(&path).is_ok_and(|s| {
                s.contains("/target/debug/markerup") && s.contains("--reminder-service")
            }) {
                fs::remove_file(path)?;
            }
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir().join("service.log"))?;
        Command::new(executable)
            .process_group(0)
            .arg("--reminder-service")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()?;
    }
    let started = Instant::now();
    while !heartbeat() {
        if started.elapsed().as_secs() >= 45 {
            return Err(io::Error::other(
                "Reminder worker did not become ready; inspect service.log",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
fn diagnostics(error: &dyn std::fmt::Display) {
    let message = format!("{}: {error}", Utc::now());
    eprintln!("{message}");
    let _ = fs::write(data_dir().join("service.error"), &message);
}
// Keep retrying transient disk/index errors even while the editor is closed.
// systemd additionally restarts an installed service after process crashes.
pub(super) fn run_service() -> io::Result<()> {
    let Some(_lease) = ownership::claim()? else {
        return Ok(());
    };
    let signature = executable_signature()?;
    let mut backoff = 1;
    loop {
        match std::panic::catch_unwind(|| session(&signature)) {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => diagnostics(&error),
            Err(_) => diagnostics(&"Reminder worker panicked; restarting"),
        }
        std::thread::sleep(std::time::Duration::from_secs(backoff));
        backoff = (backoff * 2).min(30);
    }
}
fn session(signature: &str) -> io::Result<()> {
    let ready = Arc::new(Mutex::new(HashSet::<String>::new()));
    let scanning = Arc::new(AtomicBool::new(false));
    let mut last_scan = Instant::now() - std::time::Duration::from_secs(60);
    loop {
        if fs::read_to_string(data_dir().join("service.request")).is_ok_and(|s| s != signature) {
            return Ok(());
        }
        fs::write(data_dir().join("service.heartbeat"), signature)?;
        if last_scan.elapsed().as_secs() >= 60 && !scanning.swap(true, Ordering::AcqRel) {
            let paths = transaction(false, |s| {
                Ok(s.workspaces
                    .iter()
                    .filter(|(_, w)| !w.paused)
                    .filter_map(|(identity, w)| w.local_path.clone().map(|p| (identity.clone(), p)))
                    .collect::<Vec<_>>())
            })?;
            let ready = ready.clone();
            let scanning = scanning.clone();
            std::thread::spawn(move || {
                let outcome = std::panic::catch_unwind(|| {
                    for (identity, path) in paths {
                        let result = crate::workspace::LocalWorkspace::open(&path)
                            .and_then(|w| scan(&(Arc::new(w) as WorkspaceRef), &data_dir(), false));
                        if let Err(error) = result {
                            let _ = transaction(true, |s| {
                                if let Some(w) = s.workspaces.get_mut(&identity) {
                                    w.error = Some(format!(
                                        "Using cached schedules; workspace unavailable: {error}"
                                    ));
                                }
                                Ok(())
                            });
                        }
                        // Cached schedules are a fallback only after attempting
                        // reconciliation, never a race against the initial scan.
                        ready.lock().unwrap().insert(identity);
                    }
                });
                if outcome.is_err() {
                    diagnostics(&"Workspace scanner panicked; will retry");
                }
                scanning.store(false, Ordering::Release);
            });
            last_scan = Instant::now();
        }
        let due = transaction(false, |s| {
            let ready = ready.lock().unwrap();
            Ok(deliveries(s, Utc::now())
                .into_iter()
                .filter(|d| {
                    s.workspaces[&d.workspace].local_path.is_none() || ready.contains(&d.workspace)
                })
                .collect::<Vec<_>>())
        })?;
        deliver(due)?;
        let _ = fs::remove_file(data_dir().join("service.error"));
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
fn deliver(due: Vec<Delivery>) -> io::Result<()> {
    let now = Utc::now();
    let mut groups: Vec<Vec<Delivery>> = due
        .iter()
        .filter(|d| (now - d.due).num_seconds() <= 60)
        .cloned()
        .map(|d| vec![d])
        .collect();
    let overdue: Vec<_> = due
        .into_iter()
        .filter(|d| (now - d.due).num_seconds() > 60)
        .collect();
    if !overdue.is_empty() {
        groups.push(overdue);
    }
    for mut group in groups {
        // Recheck indexed cancellation/pause before each native request.
        let active = transaction(false, |s| {
            Ok(deliveries(s, Utc::now())
                .into_iter()
                .map(|d| d.key)
                .collect::<HashSet<_>>())
        })?;
        group.retain(|d| active.contains(&d.key));
        if group.is_empty() {
            continue;
        }
        let title = if group.len() > 1 {
            format!("{} overdue reminders", group.len())
        } else {
            format!(
                "{}{}",
                if (now - group[0].due).num_seconds() > 60 {
                    "Overdue: "
                } else {
                    ""
                },
                group[0].title
            )
        };
        let body = if group.len() > 1 {
            group
                .iter()
                .take(5)
                .map(|d| d.title.clone())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            group[0].body.clone()
        };
        let key = if group.len() == 1 {
            group[0].key.clone()
        } else {
            "overdue".into()
        };
        match notify_rust::Notification::new()
            .appname("Markerup")
            .summary(&title)
            .body(
                &body
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
            )
            .icon("markerup")
            .action("default", "Open note")
            .show()
        {
            Ok(handle) => {
                transaction(true, |s| {
                    for d in &group {
                        s.cursors.insert(d.key.clone(), now.timestamp());
                    }
                    s.notification_error = None;
                    Ok(())
                })?;
                std::thread::spawn(move || {
                    handle.wait_for_action(|action| {
                        if action == "default"
                            && let Ok(exe) = std::env::current_exe()
                        {
                            let _ = Command::new(exe)
                                .arg("--reminder-open")
                                .arg(key)
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                        }
                    })
                });
            }
            Err(error) => {
                transaction(true, |s| {
                    s.notification_error = Some(format!("Desktop notifications failed: {error}"));
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}
