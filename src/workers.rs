use crate::markdown::{image_references, preview_blocks, ImageReference, PreviewBlock};
use crate::workspace::{EntryId, LocalWorkspace, Workspace, WorkspaceEntry};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
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

#[derive(Clone)]
pub struct WorkerSenders {
    pub preview: Sender<WorkerRequest>,
    pub io: Sender<WorkerRequest>,
}

struct SearchIndex {
    root: PathBuf,
    notes: Vec<(EntryId, String, String)>,
}

impl SearchIndex {
    fn build(workspace: &LocalWorkspace) -> Result<Self, String> {
        let root = workspace.root_path().to_path_buf();
        let files = workspace.markdown_files().map_err(|error| error.to_string())?;
        let mut notes = Vec::with_capacity(files.len());
        for id in files {
            let path_lower = id.to_lowercase();
            let content_lower = workspace.read(&id).map(|text| text.to_lowercase()).unwrap_or_default();
            notes.push((id, path_lower, content_lower));
        }
        Ok(Self { root, notes })
    }

    fn matches(&self, workspace: &LocalWorkspace, query: &str) -> bool {
        self.root == workspace.root_path()
            && !query.trim().is_empty()
    }

    fn search(&self, query: &str) -> Vec<EntryId> {
        let query = query.trim().to_lowercase();
        self.notes.iter()
            .filter(|(_, path, content)| path.contains(&query) || content.contains(&query))
            .map(|(id, _, _)| id.clone())
            .collect()
    }
}

pub fn spawn_workers() -> (WorkerSenders, Receiver<WorkerResult>) {
    let (preview_tx, preview_rx) = mpsc::channel();
    let (io_tx, io_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    {
        let result_tx = result_tx.clone();
        thread::Builder::new()
            .name("markerup-preview".into())
            .spawn(move || {
                while let Ok(request) = preview_rx.recv() {
                    let WorkerRequest::Preview { generation, source } = request else { continue };
                    let started = Instant::now();
                    let blocks = preview_blocks(&source);
                    let images = image_references(&source);
                    let result = WorkerResult::Preview(PreviewResult {
                        generation,
                        source_hash: hash_text(&source),
                        blocks,
                        images,
                        elapsed: started.elapsed(),
                    });
                    if result_tx.send(result).is_err() { break; }
                }
            })
            .expect("failed to start Markerup preview worker");
    }

    thread::Builder::new()
        .name("markerup-io".into())
        .spawn(move || {
            let mut search_index: Option<SearchIndex> = None;
            while let Ok(request) = io_rx.recv() {
                let result = match request {
                    WorkerRequest::Search { generation, workspace, query } => {
                        let started = Instant::now();
                        let needs_rebuild = search_index.as_ref().is_none_or(|index| !index.matches(&workspace, &query));
                        if needs_rebuild {
                            search_index = SearchIndex::build(&workspace).ok();
                        }
                        let results = match search_index.as_ref() {
                            Some(index) => Ok(index.search(&query)),
                            None => workspace.search_markdown(&query).map_err(|error| error.to_string()),
                        };
                        WorkerResult::Search(SearchResult {
                            generation,
                            results,
                            elapsed: started.elapsed(),
                        })
                    }
                    WorkerRequest::Scan { generation, workspace, current_file } => {
                        let started = Instant::now();
                        search_index = None;
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
                    WorkerRequest::Preview { .. } => continue,
                };
                if result_tx.send(result).is_err() { break; }
            }
        })
        .expect("failed to start Markerup I/O worker");

    (WorkerSenders { preview: preview_tx, io: io_tx }, result_rx)
}

pub fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
