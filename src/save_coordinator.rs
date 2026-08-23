use crate::workers::LATEST_SAVE_GENERATION;
use crate::workspace::EntryId;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct PendingSave {
    pub generation: u64,
    pub due: Instant,
    pub file: EntryId,
    pub contents: String,
    pub expected_disk_text: String,
}

/// UI-thread save state. Workspace I/O remains performed by the I/O worker.
#[derive(Debug, Default)]
pub struct SaveCoordinator {
    generation: u64,
    pending: Option<PendingSave>,
}

impl SaveCoordinator {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn replace_workspace(&mut self, previous_generation: u64) {
        self.generation = previous_generation.wrapping_add(1);
        self.pending = None;
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
        LATEST_SAVE_GENERATION.store(self.generation, std::sync::atomic::Ordering::Release);
    }

    pub fn take_due(&mut self, now: Instant) -> Option<PendingSave> {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.due <= now)
            .then(|| self.pending.take())
            .flatten()
    }

    pub fn retry(
        &mut self,
        now: Instant,
        file: EntryId,
        contents: String,
        expected_disk_text: String,
    ) {
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
}
