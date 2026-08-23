use crate::workers::LATEST_PREVIEW_GENERATION;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct PendingPreview {
    pub generation: u64,
    pub due: Instant,
    pub source: String,
}

/// UI-thread state for debounced preview requests and stale-result rejection.
#[derive(Debug, Default)]
pub struct PreviewController {
    generation: u64,
    pending: Option<PendingPreview>,
}

impl PreviewController {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn replace_workspace(&mut self, previous_generation: u64) {
        self.generation = previous_generation.wrapping_add(1);
        self.pending = None;
        LATEST_PREVIEW_GENERATION.store(self.generation, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn schedule(&mut self, source: String, delay: Duration) {
        self.generation = self.generation.wrapping_add(1);
        LATEST_PREVIEW_GENERATION.store(self.generation, std::sync::atomic::Ordering::Relaxed);
        self.pending = Some(PendingPreview {
            generation: self.generation,
            due: Instant::now() + delay,
            source,
        });
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        LATEST_PREVIEW_GENERATION.store(self.generation, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn take_due(&mut self, now: Instant) -> Option<PendingPreview> {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.due <= now)
            .then(|| self.pending.take())
            .flatten()
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::PreviewController;
    use std::time::{Duration, Instant};

    #[test]
    fn coalesces_preview_requests_and_rejects_old_generations() {
        let mut controller = PreviewController::default();
        controller.schedule("old".into(), Duration::ZERO);
        let old_generation = controller.generation();
        controller.schedule("new".into(), Duration::ZERO);
        let pending = controller.take_due(Instant::now()).unwrap();

        assert_eq!(pending.source, "new");
        assert!(pending.generation > old_generation);
        assert!(!controller.is_pending());
    }

    #[test]
    fn invalidation_cancels_pending_preview() {
        let mut controller = PreviewController::default();
        controller.schedule("source".into(), Duration::ZERO);
        controller.invalidate();

        assert!(controller.take_due(Instant::now()).is_none());
    }
}
