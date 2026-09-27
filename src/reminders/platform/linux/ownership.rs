use super::*;

#[derive(Serialize, Deserialize)]
struct Owner {
    executable: PathBuf,
    signature: String,
}
fn signature(path: &std::path::Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    Ok(format!(
        "{}:{}:{:?}",
        path.display(),
        metadata.len(),
        metadata.modified()?
    ))
}
pub(super) fn executable_signature() -> io::Result<String> {
    signature(&std::env::current_exe()?)
}
pub(in super::super) fn heartbeat() -> bool {
    let dir = data_dir();
    let owner = fs::read(dir.join("service.owner.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Owner>(&bytes).ok());
    owner.is_some_and(|owner| {
        signature(&owner.executable).is_ok_and(|s| s == owner.signature)
            && fs::read_to_string(dir.join("service.request")).is_ok_and(|s| s == owner.signature)
            && fs::read_to_string(dir.join("service.heartbeat")).is_ok_and(|s| s == owner.signature)
            && fs::metadata(dir.join("service.heartbeat"))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() < 15)
            && lock_file(&dir.join("leader.lock")).is_ok_and(|f| f.try_lock_exclusive().is_err())
    })
}
pub(super) fn lock_file(path: &std::path::Path) -> io::Result<fs::File> {
    fs::create_dir_all(path.parent().unwrap())?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}
fn legacy_dir() -> PathBuf {
    data_dir()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("markerup-dev/reminders")
}
fn request_stop() -> io::Result<()> {
    let token = format!("handoff:{}", uuid::Uuid::new_v4());
    for dir in [data_dir(), legacy_dir()] {
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("service.request"), &token)?;
    }
    Ok(())
}
fn legacy_locks() -> io::Result<Vec<fs::File>> {
    let mut held = Vec::new();
    let start = Instant::now();
    for dir in [data_dir(), legacy_dir()] {
        let lock = lock_file(&dir.join("service.lock"))?;
        while let Err(error) = lock.try_lock_exclusive() {
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
            if start.elapsed().as_secs() >= 45 {
                return Err(io::Error::other(
                    "Previous reminder worker has not stopped; refusing concurrent delivery",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        held.push(lock);
    }
    Ok(held)
}
pub(super) fn stop_and_wait() -> io::Result<()> {
    request_stop()?;
    let leader = lock_file(&data_dir().join("leader.lock"))?;
    let start = Instant::now();
    while let Err(error) = leader.try_lock_exclusive() {
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        if start.elapsed().as_secs() >= 45 {
            return Err(io::Error::other("Reminder worker did not stop for handoff"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _locks = legacy_locks()?;
    Ok(())
}
pub(super) fn claim() -> io::Result<Option<Vec<fs::File>>> {
    let leader = lock_file(&data_dir().join("leader.lock"))?;
    match leader.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(e),
    }
    // Setup selects the owner, preventing a simultaneous development/login
    // launch from winning the handoff intended for an installed package.
    if let Ok(bytes) = fs::read(data_dir().join("service.desired.json")) {
        let desired: PathBuf = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if desired.is_file() && desired != std::env::current_exe()? {
            return Ok(None);
        }
    }
    request_stop()?;
    let mut held = legacy_locks()?;
    held.push(leader);
    migrate(&data_dir(), &legacy_dir())?;
    let executable = std::env::current_exe()?;
    let signature = signature(&executable)?;
    fs::write(data_dir().join("service.request"), &signature)?;
    fs::write(
        data_dir().join("service.owner.json"),
        serde_json::to_vec(&Owner {
            executable,
            signature,
        })
        .map_err(io::Error::other)?,
    )?;
    Ok(Some(held))
}
// Both legacy worker locks are held before importing. Preserve the newest
// delivery cursor so switching builds cannot replay an acknowledged occurrence.
fn migrate(current: &std::path::Path, legacy: &std::path::Path) -> io::Result<()> {
    if !legacy.join("index.json").exists() {
        return Ok(());
    }
    let old = transaction_at(legacy, false, |s| Ok(s.clone()))?;
    let fingerprint =
        schedule::stable_hash(&serde_json::to_string(&old).map_err(io::Error::other)?);
    transaction_at(current, true, |store| {
        if store.linux_legacy_import == Some(fingerprint) {
            return Ok(());
        }
        for (key, timestamp) in old.cursors {
            store
                .cursors
                .entry(key)
                .and_modify(|t| *t = (*t).max(timestamp))
                .or_insert(timestamp);
        }
        store.forgotten.extend(old.forgotten);
        for (identity, workspace) in old.workspaces {
            if store.forgotten.contains(&identity) {
                continue;
            }
            if let Some(current) = store.workspaces.get_mut(&identity) {
                current.paused |= workspace.paused;
                current.resume_after = current.resume_after.max(workspace.resume_after);
            }
            if store
                .workspaces
                .get(&identity)
                .is_none_or(|w| w.scanned_at < workspace.scanned_at)
            {
                store.workspaces.insert(identity, workspace);
            }
        }
        store
            .workspaces
            .retain(|id, _| !store.forgotten.contains(id));
        store.linux_legacy_import = Some(fingerprint);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_preserves_history_and_does_not_resurrect_removed_workspaces() {
        let root =
            std::env::temp_dir().join(format!("markerup-migration-{}", uuid::Uuid::new_v4()));
        let current = root.join("current");
        let legacy = root.join("legacy");
        transaction_at(&current, true, |s| {
            s.cursors.insert("newer".into(), 30);
            s.cursors.insert("older".into(), 10);
            s.workspaces.insert(
                "workspace".into(),
                WorkspaceIndex {
                    scanned_at: 20,
                    ..Default::default()
                },
            );
            Ok(())
        })
        .unwrap();
        transaction_at(&legacy, true, |s| {
            s.cursors.insert("newer".into(), 20);
            s.cursors.insert("older".into(), 20);
            s.workspaces.insert(
                "workspace".into(),
                WorkspaceIndex {
                    paused: true,
                    scanned_at: 10,
                    ..Default::default()
                },
            );
            Ok(())
        })
        .unwrap();
        migrate(&current, &legacy).unwrap();
        transaction_at(&current, true, |s| {
            assert_eq!(s.cursors["newer"], 30);
            assert_eq!(s.cursors["older"], 20);
            assert!(s.workspaces["workspace"].paused);
            s.workspaces.clear();
            Ok(())
        })
        .unwrap();
        migrate(&current, &legacy).unwrap();
        assert!(transaction_at(&current, false, |s| Ok(s.workspaces.is_empty())).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
