use crate::workers::LATEST_SAVE_GENERATION;
use crate::workspace::EntryId;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const RECENT_LOCAL_SAVE_LIMIT: usize = 8;

#[derive(Debug)]
pub struct PendingSave {
    pub generation: u64,
    pub due: Instant,
    pub file: EntryId,
    pub contents: String,
    pub expected_disk_text: String,
}

#[derive(Debug)]
struct LocalSaveRecord {
    generation: u64,
    file: EntryId,
    contents: String,
}

/// UI-thread save state. Workspace I/O remains performed by the I/O worker.
#[derive(Debug, Default)]
pub struct SaveCoordinator {
    generation: u64,
    pending: Option<PendingSave>,
    in_flight: Option<PendingSave>,
    recent_local_saves: VecDeque<LocalSaveRecord>,
}

impl SaveCoordinator {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn replace_workspace(&mut self, previous_generation: u64) {
        self.generation = previous_generation.wrapping_add(1);
        self.pending = None;
        self.in_flight = None;
        self.recent_local_saves.clear();
        LATEST_SAVE_GENERATION.store(self.generation, std::sync::atomic::Ordering::Release);
    }

    pub fn schedule(
        &mut self,
        file: Option<EntryId>,
        workspace_open: bool,
        contents: String,
        expected_disk_text: String,
        delay: Duration,
    ) {
        let Some(file) = file else { return };
        if !workspace_open {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        LATEST_SAVE_GENERATION.store(self.generation, std::sync::atomic::Ordering::Release);
        self.pending = Some(PendingSave {
            generation: self.generation,
            due: Instant::now() + delay,
            file,
            contents,
            expected_disk_text,
        });
    }

    pub fn begin_immediate(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        LATEST_SAVE_GENERATION.store(self.generation, std::sync::atomic::Ordering::Release);
        self.generation
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.in_flight = None;
        self.recent_local_saves.clear();
        LATEST_SAVE_GENERATION.store(self.generation, std::sync::atomic::Ordering::Release);
    }

    pub fn take_due(&mut self, now: Instant) -> Option<PendingSave> {
        if self.in_flight.is_some()
            || !self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.due <= now)
        {
            return None;
        }
        let pending = self.pending.take()?;
        self.in_flight = Some(PendingSave {
            generation: pending.generation,
            due: pending.due,
            file: pending.file.clone(),
            contents: pending.contents.clone(),
            expected_disk_text: pending.expected_disk_text.clone(),
        });
        Some(pending)
    }

    pub fn retry(
        &mut self,
        now: Instant,
        file: EntryId,
        contents: String,
        expected_disk_text: String,
    ) {
        self.in_flight = None;
        self.pending = Some(PendingSave {
            generation: self.generation,
            due: now + Duration::from_secs(1),
            file,
            contents,
            expected_disk_text,
        });
    }

    pub fn clear_pending(&mut self) {
        self.pending = None;
    }

    /// Marks a submitted save as complete. The returned value is present only
    /// when the request is still associated with this workspace/session.
    pub fn complete_in_flight(&mut self, generation: u64) -> Option<PendingSave> {
        (self
            .in_flight
            .as_ref()
            .is_some_and(|pending| pending.generation == generation))
        .then(|| self.in_flight.take())
        .flatten()
    }

    pub fn has_in_flight_for(&self, file: &str) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|pending| pending.file == file)
    }

    /// Records contents written by Markerup so a watcher/provider scan cannot
    /// mistake our own write for an external edit that happened concurrently.
    pub fn record_local_save(&mut self, file: EntryId, generation: u64, contents: String) {
        self.recent_local_saves.push_back(LocalSaveRecord {
            generation,
            file,
            contents,
        });
        while self.recent_local_saves.len() > RECENT_LOCAL_SAVE_LIMIT {
            self.recent_local_saves.pop_front();
        }
    }

    pub fn matches_recent_local_save(&self, file: &str, contents: &str) -> bool {
        self.recent_local_saves
            .iter()
            .rev()
            .any(|save| save.file == file && save.contents == contents)
    }

    pub fn has_newer_local_save(&self, file: &str, generation: u64) -> bool {
        self.recent_local_saves
            .iter()
            .any(|save| save.file == file && save.generation > generation)
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::SaveCoordinator;
    use std::time::{Duration, Instant};

    #[test]
    fn coalesces_debounced_saves() {
        let mut coordinator = SaveCoordinator::default();
        coordinator.schedule(
            Some("note.md".into()),
            true,
            "old".into(),
            "disk".into(),
            Duration::ZERO,
        );
        coordinator.schedule(
            Some("note.md".into()),
            true,
            "new".into(),
            "disk".into(),
            Duration::ZERO,
        );
        let pending = coordinator.take_due(Instant::now()).unwrap();

        assert_eq!(pending.contents, "new");
        assert!(!coordinator.is_pending());
    }

    #[test]
    fn immediate_save_invalidates_pending_work() {
        let mut coordinator = SaveCoordinator::default();
        coordinator.schedule(
            Some("note.md".into()),
            true,
            "contents".into(),
            "disk".into(),
            Duration::from_secs(1),
        );
        let generation = coordinator.begin_immediate();

        assert!(!coordinator.is_pending());
        assert_eq!(generation, coordinator.generation());
    }

    #[test]
    fn does_not_dispatch_a_second_save_until_the_first_finishes() {
        let mut coordinator = SaveCoordinator::default();
        coordinator.schedule(
            Some("note.md".into()),
            true,
            "first".into(),
            "disk".into(),
            Duration::ZERO,
        );
        let first = coordinator.take_due(Instant::now()).unwrap();
        coordinator.schedule(
            Some("note.md".into()),
            true,
            "second".into(),
            "disk".into(),
            Duration::ZERO,
        );

        assert!(coordinator.take_due(Instant::now()).is_none());
        coordinator.complete_in_flight(first.generation);
        assert_eq!(
            coordinator.take_due(Instant::now()).unwrap().contents,
            "second"
        );
    }

    #[test]
    fn recognizes_recent_local_save_contents() {
        let mut coordinator = SaveCoordinator::default();
        coordinator.record_local_save("note.md".into(), 1, "written by markerup".into());

        assert!(coordinator.matches_recent_local_save("note.md", "written by markerup"));
        assert!(!coordinator.matches_recent_local_save("note.md", "external edit"));
    }
}
