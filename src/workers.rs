use crate::markdown::{image_references, preview_blocks, ImageReference, PreviewBlock};
use crate::workspace::{EntryId, LocalWorkspace, Workspace, WorkspaceEntry};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum WorkerRequest {
    Preview {
        generation: u64,
        source: String,
    },
    Search {
        generation: u64,
        workspace: LocalWorkspace,
        query: String,
    },
    Scan {
        generation: u64,
        workspace: LocalWorkspace,
        current_file: Option<EntryId>,
    },
}

#[derive(Debug)]
pub enum WorkerResult {
    Preview(PreviewResult),
    Search(SearchResult),
    Scan(ScanResult),
}

#[derive(Debug)]
pub struct PreviewResult {
    pub generation: u64,
    pub source_hash: u64,
    pub blocks: Vec<PreviewBlock>,
    pub images: Vec<ImageReference>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct SearchResult {
    pub generation: u64,
    pub results: Result<Vec<EntryId>, String>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct ScanResult {
    pub generation: u64,
    pub entries: Result<Vec<WorkspaceEntry>, String>,
    pub current_file: Option<EntryId>,
    pub current_text: Option<Result<String, String>>,
    pub elapsed: Duration,
}

pub fn spawn_worker() -> (Sender<WorkerRequest>, Receiver<WorkerResult>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    thread::Builder::new()
        .name("markerup-worker".into())
        .spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let result = match request {
                    WorkerRequest::Preview { generation, source } => {
                        let started = Instant::now();
                        let blocks = preview_blocks(&source);
                        let images = image_references(&source);
                        WorkerResult::Preview(PreviewResult {
                            generation,
                            source_hash: hash_text(&source),
                            blocks,
                            images,
                            elapsed: started.elapsed(),
                        })
                    }
                    WorkerRequest::Search { generation, workspace, query } => {
                        let started = Instant::now();
                        let results = workspace.search_markdown(&query).map_err(|error| error.to_string());
                        WorkerResult::Search(SearchResult {
                            generation,
                            results,
                            elapsed: started.elapsed(),
                        })
                    }
                    WorkerRequest::Scan { generation, workspace, current_file } => {
                        let started = Instant::now();
                        let entries = workspace.entries().map_err(|error| error.to_string());
                        let current_text = current_file
                            .as_deref()
                            .map(|id| workspace.read(id).map_err(|error| error.to_string()));
                        WorkerResult::Scan(ScanResult {
                            generation,
                            entries,
                            current_file,
                            current_text,
                            elapsed: started.elapsed(),
                        })
                    }
                };

                if result_tx.send(result).is_err() {
                    break;
                }
            }
        })
        .expect("failed to start Markerup background worker");

    (request_tx, result_rx)
}

pub fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
