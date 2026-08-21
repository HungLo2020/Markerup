use crate::markdown::{ImageReference, PreviewBlock, image_references, preview_blocks};
use crate::workspace::{EntryId, Workspace, WorkspaceEntry, WorkspaceRef};
use merman::render::HeadlessRenderer;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub static LATEST_SEARCH_GENERATION: AtomicU64 = AtomicU64::new(0);

fn normalize_mermaid_source(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let leading = line
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .count();
            let prefix = &line[..line
                .char_indices()
                .nth(leading)
                .map_or(line.len(), |(index, _)| index)];
            let rest = &line[prefix.len()..];
            let mut normalized = prefix.replace('\t', "  ");
            let mut offset = 0;
            while let Some(relative) = rest[offset..].find('#') {
                let index = offset + relative;
                normalized.push_str(&rest[offset..index]);
                if is_hex_color(rest, index) {
                    normalized.push('#');
                } else {
                    normalized.push_str("Number");
                }
                offset = index + 1;
            }
            normalized.push_str(&rest[offset..]);
            normalized
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_hex_color(line: &str, hash_index: usize) -> bool {
    let hex = line[hash_index + 1..]
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .count();
    matches!(hex, 3 | 4 | 6 | 8)
        && line[hash_index
            + 1
            + line[hash_index + 1..]
                .chars()
                .take(hex)
                .map(char::len_utf8)
                .sum::<usize>()..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

pub enum WorkerRequest {
    Preview {
        generation: u64,
        source: String,
    },
    Search {
        generation: u64,
        workspace: WorkspaceRef,
        query: String,
    },
    Scan {
        generation: u64,
        workspace: WorkspaceRef,
        current_file: Option<EntryId>,
        full_tree: bool,
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
    pub mermaid_svgs: Vec<Option<Result<String, String>>>,
    pub images: Vec<ImageReference>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct SearchResult {
    pub generation: u64,
    pub results: Result<Vec<EntryId>, String>,
    pub cancelled: bool,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct ScanResult {
    pub generation: u64,
    pub entries: Option<Result<Vec<WorkspaceEntry>, String>>,
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
    identity: String,
    notes: Vec<(EntryId, String)>,
    content_cache: HashMap<EntryId, String>,
    cache_order: VecDeque<EntryId>,
    cache_bytes: usize,
}

const SEARCH_CACHE_LIMIT: usize = 4 * 1024 * 1024;

impl SearchIndex {
    fn build(workspace: &dyn Workspace, generation: u64) -> Result<Option<Self>, String> {
        let identity = workspace.identity();
        let entries = workspace
            .entries_with_cancel(&|| LATEST_SEARCH_GENERATION.load(Ordering::Relaxed) != generation)
            .map_err(|error| error.to_string())?;
        let Some(entries) = entries else {
            return Ok(None);
        };
        let mut notes = Vec::with_capacity(entries.len());
        for entry in entries
            .into_iter()
            .filter(|entry| entry.kind == crate::workspace::EntryKind::File)
        {
            if LATEST_SEARCH_GENERATION.load(Ordering::Relaxed) != generation {
                return Ok(None);
            }
            let id = entry.id;
            let path_lower = id.to_lowercase();
            notes.push((id, path_lower));
        }
        Ok(Some(Self {
            identity,
            notes,
            content_cache: HashMap::new(),
            cache_order: VecDeque::new(),
            cache_bytes: 0,
        }))
    }

    fn matches(&self, workspace: &dyn Workspace, query: &str) -> bool {
        self.identity == workspace.identity() && !query.trim().is_empty()
    }

    fn search(
        &mut self,
        workspace: &dyn Workspace,
        query: &str,
        generation: u64,
    ) -> Option<Vec<EntryId>> {
        let query = query.trim().to_lowercase();
        let mut results = Vec::new();
        for index in 0..self.notes.len() {
            if LATEST_SEARCH_GENERATION.load(Ordering::Relaxed) != generation {
                return None;
            }
            let (id, path) = &self.notes[index];
            let id = id.clone();
            let path = path.clone();
            let path_match = path.contains(&query);
            let content_match = if path_match {
                false
            } else if let Some(content) = self.content_cache.get(&id) {
                content.contains(&query)
            } else {
                let content = workspace
                    .read(&id)
                    .map(|text| text.to_lowercase())
                    .unwrap_or_default();
                let matched = content.contains(&query);
                self.cache_content(&id, content);
                matched
            };
            if path_match || content_match {
                results.push(id.clone());
            }
        }
        Some(results)
    }

    fn cache_content(&mut self, id: &str, content: String) {
        if content.len() > SEARCH_CACHE_LIMIT {
            return;
        }
        while self.cache_bytes + content.len() > SEARCH_CACHE_LIMIT {
            let Some(oldest) = self.cache_order.pop_front() else {
                break;
            };
            if let Some(value) = self.content_cache.remove(&oldest) {
                self.cache_bytes = self.cache_bytes.saturating_sub(value.len());
            }
        }
        self.cache_bytes += content.len();
        self.cache_order.push_back(id.to_string());
        self.content_cache.insert(id.to_string(), content);
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
                let mut mermaid_renderer: Option<HeadlessRenderer> = None;
                while let Ok(request) = preview_rx.recv() {
                    let WorkerRequest::Preview { generation, source } = request else {
                        continue;
                    };
                    let started = Instant::now();
                    let blocks = preview_blocks(&source);
                    let mermaid_svgs = blocks
                        .iter()
                        .enumerate()
                        .map(|(index, block)| {
                            if !matches!(block.kind, crate::markdown::PreviewBlockKind::Mermaid) {
                                return None;
                            }
                            let diagram_id = format!("markerup-{generation}-{index}");
                            let renderer =
                                mermaid_renderer.get_or_insert_with(HeadlessRenderer::new);
                            let normalized_source = normalize_mermaid_source(&block.markdown);
                            Some(
                                renderer
                                    .render_svg_resvg_safe_sync_with_diagram_id(
                                        &normalized_source,
                                        &diagram_id,
                                    )
                                    .map_err(|error| error.to_string())
                                    .and_then(|svg| {
                                        svg.ok_or_else(|| "no Mermaid diagram found".to_string())
                                    }),
                            )
                        })
                        .collect();
                    let images = image_references(&source);
                    let result = WorkerResult::Preview(PreviewResult {
                        generation,
                        source_hash: hash_text(&source),
                        blocks,
                        mermaid_svgs,
                        images,
                        elapsed: started.elapsed(),
                    });
                    if result_tx.send(result).is_err() {
                        break;
                    }
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
                    WorkerRequest::Search {
                        generation,
                        workspace,
                        query,
                    } => {
                        let started = Instant::now();
                        if LATEST_SEARCH_GENERATION.load(Ordering::Relaxed) != generation {
                            WorkerResult::Search(SearchResult {
                                generation,
                                results: Ok(Vec::new()),
                                cancelled: true,
                                elapsed: started.elapsed(),
                            })
                        } else {
                            let needs_rebuild = search_index
                                .as_ref()
                                .is_none_or(|index| !index.matches(workspace.as_ref(), &query));
                            if needs_rebuild {
                                search_index = SearchIndex::build(workspace.as_ref(), generation)
                                    .ok()
                                    .flatten();
                            }
                            let (results, cancelled) = match search_index.as_mut() {
                                Some(index) => {
                                    match index.search(workspace.as_ref(), &query, generation) {
                                        Some(results) => (Ok(results), false),
                                        None => (Ok(Vec::new()), true),
                                    }
                                }
                                None => (Err("could not build search index".to_string()), false),
                            };
                            WorkerResult::Search(SearchResult {
                                generation,
                                results,
                                cancelled,
                                elapsed: started.elapsed(),
                            })
                        }
                    }
                    WorkerRequest::Scan {
                        generation,
                        workspace,
                        current_file,
                        full_tree,
                    } => {
                        let started = Instant::now();
                        // A current-file check can still represent an edited
                        // note, so invalidate cached search contents for both
                        // scan modes. The next search rebuilds from disk.
                        search_index = None;
                        let entries = full_tree
                            .then(|| workspace.entries().map_err(|error| error.to_string()));
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
                if result_tx.send(result).is_err() {
                    break;
                }
            }
        })
        .expect("failed to start Markerup I/O worker");

    (
        WorkerSenders {
            preview: preview_tx,
            io: io_tx,
        },
        result_rx,
    )
}

pub fn hash_text(text: &str) -> u64 {
    // This hash is only used to detect whether two versions of the editor
    // contents are equal. It is not security-sensitive, so avoid SipHash's
    // relatively high per-byte cost on every preview/save operation.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{HeadlessRenderer, normalize_mermaid_source};

    #[test]
    fn mermaid_source_normalizes_leading_tabs() {
        let source = "---\nconfig:\n\tlayout: elk\n---\nflowchart TD\n\tCell#([Cell#])\nstyle Cell# fill:#fff";
        assert_eq!(
            normalize_mermaid_source(source),
            "---\nconfig:\n  layout: elk\n---\nflowchart TD\n  CellNumber([CellNumber])\nstyle CellNumber fill:#fff"
        );
    }

    #[test]
    fn merman_renders_a_flowchart_to_svg() {
        let renderer = HeadlessRenderer::new();
        let svg = renderer
            .render_svg_resvg_safe_sync_with_diagram_id(
                "flowchart TD\n    A[Start] --> B[Done]",
                "markerup-test-flowchart",
            )
            .expect("Mermaid flowchart should render")
            .expect("valid Mermaid should produce SVG");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Start"));
        assert!(svg.contains("Done"));
    }
}
