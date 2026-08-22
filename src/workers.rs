use crate::markdown::{ImageReference, PreviewBlock, preview_document};
use crate::workspace::{EntryId, Workspace, WorkspaceEntry, WorkspaceRef};
use merman::MermaidConfig;
use merman::render::HeadlessRenderer;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub static LATEST_SEARCH_GENERATION: AtomicU64 = AtomicU64::new(0);
pub static LATEST_PREVIEW_GENERATION: AtomicU64 = AtomicU64::new(0);

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn safe_workspace_call<T>(operation: impl FnOnce() -> std::io::Result<T>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(payload) => Err(format!(
            "workspace operation panicked: {}",
            panic_text(payload)
        )),
    }
}

// Keep a stuck provider/client operation from permanently wedging the shared
// I/O worker. The SMB backend has shorter per-request timeouts, but this outer
// guard also covers a transport/runtime deadlock.
const SMB_SCAN_WATCHDOG: Duration = Duration::from_secs(60);

fn safe_workspace_scan(workspace: WorkspaceRef) -> Result<Vec<WorkspaceEntry>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("markerup-smb-scan-watchdog".to_string())
        .spawn(move || {
            let result = safe_workspace_call(|| workspace.entries());
            let _ = sender.send(result);
        })
        .map_err(|error| format!("could not start SMB scan watchdog: {error}"))?;

    receiver
        .recv_timeout(SMB_SCAN_WATCHDOG)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => format!(
                "SMB workspace scan exceeded {} seconds",
                SMB_SCAN_WATCHDOG.as_secs()
            ),
            mpsc::RecvTimeoutError::Disconnected => "SMB scan worker disconnected".to_string(),
        })?
}

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

fn mermaid_theme_config(dark: bool) -> MermaidConfig {
    if dark {
        MermaidConfig::from_value(json!({
        "theme": "base",
        "themeVariables": {
            "background": "#1c1c1e",
            "mainBkg": "#252b35",
            "primaryColor": "#273449",
            "primaryTextColor": "#f2f2f7",
            "primaryBorderColor": "#72b7ff",
            "lineColor": "#93c5fd",
            "secondaryColor": "#334155",
            "tertiaryColor": "#1f2937",
            "nodeTextColor": "#f2f2f7",
            "textColor": "#f2f2f7",
            "edgeLabelBackground": "#1c1c1e",
            "clusterBkg": "#202938",
            "clusterBorder": "#64748b",
            "titleColor": "#f8fafc",
            "noteBkgColor": "#3a321f",
            "noteTextColor": "#fef3c7",
            "noteBorderColor": "#fbbf24"
        },
        "themeCSS": "text, .label, .nodeLabel { font-family: -apple-system, BlinkMacSystemFont, sans-serif; }"
        }))
    } else {
        MermaidConfig::from_value(json!({
            "theme": "base",
            "themeVariables": {
                "background": "#ffffff",
                "mainBkg": "#f8fafc",
                "primaryColor": "#dbeafe",
                "primaryTextColor": "#111827",
                "primaryBorderColor": "#2563eb",
                "lineColor": "#2563eb",
                "textColor": "#111827",
                "edgeLabelBackground": "#ffffff",
                "clusterBkg": "#eff6ff",
                "clusterBorder": "#64748b",
                "titleColor": "#111827",
                "noteBkgColor": "#fef3c7",
                "noteTextColor": "#111827",
                "noteBorderColor": "#d97706"
            },
            "themeCSS": "text, .label, .nodeLabel { font-family: -apple-system, BlinkMacSystemFont, sans-serif; }"
        }))
    }
}

fn dark_mermaid_renderer() -> HeadlessRenderer {
    HeadlessRenderer::new().with_site_config(mermaid_theme_config(true))
}

#[allow(dead_code)]
fn light_mermaid_renderer() -> HeadlessRenderer {
    HeadlessRenderer::new().with_site_config(mermaid_theme_config(false))
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
                while let Ok(mut request) = preview_rx.recv() {
                    // Text edits can enqueue several previews while an older
                    // parse or Mermaid render is still running. Only the
                    // newest request can be displayed, so discard queued
                    // superseded work before starting the next parse.
                    while let Ok(next) = preview_rx.try_recv() {
                        request = next;
                    }
                    let WorkerRequest::Preview { generation, source } = request else {
                        continue;
                    };
                    let started = Instant::now();
                    let document = preview_document(&source);
                    let blocks = document.blocks;
                    let images = document.images;
                    let mut mermaid_svgs = Vec::with_capacity(blocks.len());
                    let mut cancelled = false;
                    for (index, block) in blocks.iter().enumerate() {
                        if LATEST_PREVIEW_GENERATION.load(Ordering::Relaxed) != generation {
                            cancelled = true;
                            break;
                        }
                        if !matches!(block.kind, crate::markdown::PreviewBlockKind::Mermaid) {
                            mermaid_svgs.push(None);
                            continue;
                        }
                        let diagram_id = format!("markerup-{generation}-{index}");
                        let renderer = mermaid_renderer.get_or_insert_with(dark_mermaid_renderer);
                        let normalized_source = normalize_mermaid_source(&block.markdown);
                        mermaid_svgs.push(Some(
                            renderer
                                .render_svg_resvg_safe_sync_with_diagram_id(
                                    &normalized_source,
                                    &diagram_id,
                                )
                                .map_err(|error| error.to_string())
                                .and_then(|svg| {
                                    svg.ok_or_else(|| "no Mermaid diagram found".to_string())
                                }),
                        ));
                    }
                    if cancelled {
                        continue;
                    }
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
                        // A current-file check can still represent an edited
                        // note, so invalidate cached search contents for both
                        // scan modes. The next search rebuilds from disk.
                        search_index = None;
                        // Do not let a provider/network scan block the shared
                        // search worker. In particular, an SMB transport can
                        // outlive its request when a server disappears; the
                        // scan watchdog will report that operation while this
                        // worker remains able to process newer requests.
                        let scan_result_tx = result_tx.clone();
                        let scan_start = thread::Builder::new()
                            .name("markerup-workspace-scan".to_string())
                            .spawn(move || {
                                let started = Instant::now();
                                let entries =
                                    full_tree.then(|| safe_workspace_scan(workspace.clone()));
                                let current_text = current_file
                                    .as_deref()
                                    .map(|id| safe_workspace_call(|| workspace.read(id)));
                                let _ = scan_result_tx.send(WorkerResult::Scan(ScanResult {
                                    generation,
                                    entries,
                                    current_file,
                                    current_text,
                                    elapsed: started.elapsed(),
                                }));
                            });
                        if let Err(error) = scan_start {
                            let _ = result_tx.send(WorkerResult::Scan(ScanResult {
                                generation,
                                entries: full_tree.then(|| {
                                    Err(format!("could not start workspace scan: {error}"))
                                }),
                                current_file: None,
                                current_text: None,
                                elapsed: Duration::ZERO,
                            }));
                        }
                        continue;
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
    use super::{HeadlessRenderer, dark_mermaid_renderer, normalize_mermaid_source};

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

    #[test]
    fn dark_mermaid_theme_provides_readable_colors() {
        let svg = dark_mermaid_renderer()
            .render_svg_resvg_safe_sync_with_diagram_id(
                "flowchart TD\n  A[Start] --> B[Done]",
                "markerup-dark-theme",
            )
            .expect("dark Mermaid should render")
            .expect("valid Mermaid should produce SVG");
        assert!(svg.contains("#f2f2f7") || svg.contains("#f8fafc"));
        assert!(svg.contains("#72b7ff") || svg.contains("#93c5fd"));
    }
}
