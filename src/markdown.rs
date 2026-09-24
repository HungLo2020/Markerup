use markdown::{Constructs, ParseOptions, mdast::Node, to_mdast};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageReference {
    pub alt: String,
    pub destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PreviewBlockKind {
    Body,
    Heading(u8),
    Task(bool),
    Mermaid,
    Code,
    List(bool),
    Quote,
    Rule,
    Image,
    Table,
    Footnote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBlock {
    pub kind: PreviewBlockKind,
    pub markdown: String,
    pub image: Option<ImageReference>,
    pub task_offset: Option<usize>,
    pub source_range: Option<SourceRange>,
    pub language: Option<String>,
    pub footnote_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewDocument {
    pub blocks: Vec<PreviewBlock>,
    pub images: Vec<ImageReference>,
    #[serde(rename = "linkDefinitions")]
    pub link_definitions: Vec<String>,
}

fn parse_options() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    }
}

pub fn preview_document(source: &str) -> PreviewDocument {
    let tree = to_mdast(source, &parse_options()).unwrap_or_else(|_| {
        Node::Root(markdown::mdast::Root {
            children: vec![Node::Text(markdown::mdast::Text {
                value: source.to_string(),
                position: None,
            })],
            position: None,
        })
    });
    let mut definitions = std::collections::HashMap::new();
    collect_definitions(&tree, &mut definitions);
    let mut images = Vec::new();
    collect_images(&tree, &definitions, &mut images);
    let mut link_definitions = Vec::new();
    collect_link_definitions(&tree, source, &mut link_definitions);
    let parsed_blocks: Vec<PreviewBlock> = match &tree {
        Node::Root(root) => root
            .children
            .iter()
            .flat_map(|node| block_from_node(node, source, &definitions))
            .collect(),
        node => block_from_node(node, source, &definitions).collect(),
    };
    PreviewDocument {
        blocks: parsed_blocks,
        images,
        link_definitions,
    }
}

fn collect_link_definitions(node: &Node, source: &str, definitions: &mut Vec<String>) {
    if let Node::Definition(_) = node
        && let Some(range) = source_range(node)
        && let Some(markdown) = source.get(range.start..range.end)
    {
        definitions.push(markdown.to_string());
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_link_definitions(child, source, definitions);
        }
    }
}

fn block_from_node(
    node: &Node,
    source: &str,
    definitions: &std::collections::HashMap<String, String>,
) -> std::vec::IntoIter<PreviewBlock> {
    let source_range = source_range(node);
    let original_markdown = source_range
        .as_ref()
        .and_then(|range| source.get(range.start..range.end))
        .unwrap_or_default()
        .to_string();
    let block = match node {
        Node::Heading(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Heading(value.depth),
            markdown: original_markdown.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::Paragraph(value)
            if value.children.len() == 1
                && image_reference_from_node(&value.children[0], definitions).is_some() =>
        {
            let image = image_reference_from_node(&value.children[0], definitions)
                .expect("image guard should provide an image reference");
            Some(PreviewBlock {
                kind: PreviewBlockKind::Image,
                markdown: image.alt.clone(),
                image: Some(image),
                task_offset: None,
                source_range: source_range.clone(),
                language: None,
                footnote_id: None,
            })
        }
        Node::Paragraph(_value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Body,
            markdown: original_markdown.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::Code(value)
            if value
                .lang
                .as_deref()
                .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid")) =>
        {
            Some(PreviewBlock {
                kind: PreviewBlockKind::Mermaid,
                markdown: value.value.clone(),
                image: None,
                task_offset: None,
                source_range: source_range.clone(),
                language: value.lang.clone(),
                footnote_id: None,
            })
        }
        Node::Code(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: value.lang.clone(),
            footnote_id: None,
        }),
        Node::List(value) => {
            if is_task_list(value) {
                return task_blocks_from_list(value, source, definitions).into_iter();
            }
            Some(PreviewBlock {
                kind: PreviewBlockKind::List(value.ordered),
                markdown: original_markdown.clone(),
                image: None,
                task_offset: None,
                source_range: source_range.clone(),
                language: None,
                footnote_id: None,
            })
        }
        Node::Blockquote(_value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Quote,
            markdown: original_markdown.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::ThematicBreak(_) => Some(PreviewBlock {
            kind: PreviewBlockKind::Rule,
            markdown: String::new(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::Image(_) | Node::ImageReference(_) => image_reference_from_node(node, definitions)
            .map(|image| PreviewBlock {
                kind: PreviewBlockKind::Image,
                markdown: image.alt.clone(),
                image: Some(image),
                task_offset: None,
                source_range: source_range.clone(),
                language: None,
                footnote_id: None,
            }),
        Node::Table(_value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Table,
            markdown: original_markdown.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::Yaml(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
            task_offset: None,
            source_range: source_range.clone(),
            language: None,
            footnote_id: None,
        }),
        Node::Toml(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
            task_offset: None,
            source_range,
            language: None,
            footnote_id: None,
        }),
        Node::Html(_value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Body,
            markdown: original_markdown,
            image: None,
            task_offset: None,
            source_range,
            language: None,
            footnote_id: None,
        }),
        Node::FootnoteDefinition(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Footnote,
            markdown: original_markdown,
            image: None,
            task_offset: None,
            source_range,
            language: None,
            footnote_id: Some(value.identifier.clone()),
        }),
        _ => None,
    };
    block.into_iter().collect::<Vec<_>>().into_iter()
}

fn collect_definitions(node: &Node, definitions: &mut std::collections::HashMap<String, String>) {
    if let Node::Definition(definition) = node {
        definitions.insert(definition.identifier.clone(), definition.url.clone());
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_definitions(child, definitions);
        }
    }
}

fn source_range(node: &Node) -> Option<SourceRange> {
    let position = node.position()?;
    Some(SourceRange {
        start: position.start.offset,
        end: position.end.offset,
    })
}

fn collect_images(
    node: &Node,
    definitions: &std::collections::HashMap<String, String>,
    images: &mut Vec<ImageReference>,
) {
    match node {
        Node::Image(image) => images.push(ImageReference {
            alt: image.alt.clone(),
            destination: image.url.clone(),
        }),
        Node::ImageReference(image) => {
            if let Some(destination) = definitions.get(&image.identifier) {
                images.push(ImageReference {
                    alt: image.alt.clone(),
                    destination: destination.clone(),
                });
            }
        }
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_images(child, definitions, images);
        }
    }
}

fn image_reference_from_node(
    node: &Node,
    definitions: &std::collections::HashMap<String, String>,
) -> Option<ImageReference> {
    match node {
        Node::Image(image) => Some(ImageReference {
            alt: image.alt.clone(),
            destination: image.url.clone(),
        }),
        Node::ImageReference(image) => {
            definitions
                .get(&image.identifier)
                .map(|destination| ImageReference {
                    alt: image.alt.clone(),
                    destination: destination.clone(),
                })
        }
        _ => None,
    }
}

fn is_task_list(list: &markdown::mdast::List) -> bool {
    !list.children.is_empty()
        && list
            .children
            .iter()
            .all(|child| matches!(child, Node::ListItem(item) if item.checked.is_some()))
}

fn task_blocks_from_list(
    list: &markdown::mdast::List,
    source: &str,
    definitions: &std::collections::HashMap<String, String>,
) -> Vec<PreviewBlock> {
    let mut blocks = Vec::new();
    for child in &list.children {
        let Node::ListItem(item) = child else {
            continue;
        };

        // A task item's label is its paragraph content. Nested lists are
        // separate tasks and must not be concatenated into that label.
        let label = item
            .children
            .iter()
            .filter(|child| !matches!(child, Node::List(_)))
            .map(|node| node_markdown(node, definitions))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(PreviewBlock {
            kind: PreviewBlockKind::Task(item.checked.unwrap_or(false)),
            markdown: label,
            image: None,
            task_offset: item.position.as_ref().map(|position| position.start.offset),
            source_range: item.position.as_ref().map(|position| {
                let start = position.start.offset;
                let end = source[start..]
                    .find('\n')
                    .map(|offset| start + offset)
                    .unwrap_or(source.len());
                SourceRange { start, end }
            }),
            language: None,
            footnote_id: None,
        });

        for nested in item.children.iter().filter_map(|child| match child {
            Node::List(list) => Some(list),
            _ => None,
        }) {
            if is_task_list(nested) {
                blocks.extend(task_blocks_from_list(nested, source, definitions));
            }
        }
    }
    blocks
}

fn parse_task_line(line: &str) -> Option<(bool, String)> {
    let trimmed = line.trim_start();
    for prefix in ["-", "*", "+"] {
        for (marker, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(text) = trimmed.strip_prefix(&format!("{prefix} {marker}")) {
                return Some((checked, text.to_string()));
            }
        }
    }
    let dot = trimmed.find(". ")?;
    if dot > 0 && trimmed[..dot].chars().all(|ch| ch.is_ascii_digit()) {
        for (marker, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(text) = trimmed[dot + 2..].strip_prefix(marker) {
                return Some((checked, text.to_string()));
            }
        }
    }
    None
}

/// Toggle the task marker for the zero-based task item in source order.
/// Fenced code blocks are ignored so examples containing task syntax remain code.
pub fn toggle_task_at_offset(source: &str, task_offset: usize) -> Option<String> {
    if task_offset >= source.len() {
        return None;
    }
    let line_start = source[..task_offset]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let line_end = source[task_offset..]
        .find('\n')
        .map_or(source.len(), |offset| task_offset + offset);
    let line = &source[line_start..line_end];
    parse_task_line(line)?;
    let trimmed = line.trim_start();
    let leading = line.len() - trimmed.len();
    let (marker_offset, marker) = ["[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|candidate| trimmed.find(candidate).map(|offset| (offset, *candidate)))?;
    let absolute = line_start + leading + marker_offset;
    let replacement = if marker == "[ ]" { "[x]" } else { "[ ]" };
    Some(format!(
        "{}{}{}{}",
        &source[..absolute],
        replacement,
        &source[absolute + marker.len()..line_end],
        &source[line_end..]
    ))
}

fn list_markdown(
    list: &markdown::mdast::List,
    definitions: &std::collections::HashMap<String, String>,
) -> String {
    list.children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            let Node::ListItem(item) = child else {
                return String::new();
            };
            let marker = if list.ordered {
                format!("{}. ", list.start.unwrap_or(1) + index as u32)
            } else {
                "- ".to_string()
            };
            let check = item
                .checked
                .map(|checked| if checked { "[x] " } else { "[ ] " })
                .unwrap_or("");
            format!(
                "{}{}{}",
                marker,
                check,
                item.children
                    .iter()
                    .map(|node| node_markdown(node, definitions))
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_markdown(
    table: &markdown::mdast::Table,
    definitions: &std::collections::HashMap<String, String>,
) -> String {
    let rows = table
        .children
        .iter()
        .map(|row| {
            let Node::TableRow(row) = row else {
                return String::new();
            };
            row.children
                .iter()
                .map(|node| node_markdown(node, definitions))
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return String::new();
    }
    let separator = table
        .align
        .iter()
        .map(|alignment| match alignment {
            markdown::mdast::AlignKind::Left => ":---",
            markdown::mdast::AlignKind::Right => "---:",
            markdown::mdast::AlignKind::Center => ":---:",
            markdown::mdast::AlignKind::None => "---",
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let mut output = Vec::with_capacity(rows.len() + 1);
    output.push(rows[0].clone());
    output.push(separator);
    output.extend(rows.into_iter().skip(1));
    output.join("\n")
}

fn inline_markdown(
    nodes: &[Node],
    definitions: &std::collections::HashMap<String, String>,
) -> String {
    nodes
        .iter()
        .map(|node| node_markdown(node, definitions))
        .collect()
}

fn node_markdown(node: &Node, definitions: &std::collections::HashMap<String, String>) -> String {
    match node {
        Node::Paragraph(value) => inline_markdown(&value.children, definitions),
        Node::Heading(value) => inline_markdown(&value.children, definitions),
        Node::Blockquote(value) => value
            .children
            .iter()
            .map(|node| node_markdown(node, definitions))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Node::List(value) => list_markdown(value, definitions),
        Node::ListItem(value) => value
            .children
            .iter()
            .map(|node| node_markdown(node, definitions))
            .collect::<Vec<_>>()
            .join("\n"),
        Node::Code(value) => value.value.clone(),
        Node::ThematicBreak(_) => "---".to_string(),
        Node::Table(value) => table_markdown(value, definitions),
        Node::TableRow(value) => value
            .children
            .iter()
            .map(|node| node_markdown(node, definitions))
            .collect::<Vec<_>>()
            .join(" | "),
        Node::TableCell(value) => inline_markdown(&value.children, definitions),
        Node::Text(value) => escape_styled_text(&value.value),
        Node::Emphasis(value) => format!("*{}*", inline_markdown(&value.children, definitions)),
        Node::Strong(value) => format!("**{}**", inline_markdown(&value.children, definitions)),
        Node::Delete(value) => format!("~~{}~~", inline_markdown(&value.children, definitions)),
        Node::InlineCode(value) => format!("`{}`", value.value),
        Node::Link(value) => format!(
            "[{}]({})",
            inline_markdown(&value.children, definitions),
            value.url
        ),
        Node::Image(value) => format!("![{}]({})", value.alt, value.url),
        Node::LinkReference(value) => {
            let label = inline_markdown(&value.children, definitions);
            if let Some(url) = definitions.get(&value.identifier) {
                format!("[{label}]({url})")
            } else {
                format!("[{label}][{}]", value.identifier)
            }
        }
        Node::ImageReference(value) => {
            if let Some(url) = definitions.get(&value.identifier) {
                format!("![{}]({url})", value.alt)
            } else {
                format!("![{}][{}]", value.alt, value.identifier)
            }
        }
        Node::Break(_) => "\n".to_string(),
        Node::InlineMath(value) => value.value.clone(),
        Node::FootnoteReference(value) => format!("[^{}]", value.identifier),
        Node::Html(value) => value.value.clone(),
        _ => String::new(),
    }
}

fn escape_styled_text(value: &str) -> String {
    value.replace('<', "&lt;").replace('>', "&gt;")
}

#[allow(dead_code)]
pub fn find_matches(source: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    source
        .match_indices(query)
        .map(|(start, matched)| (start, start + matched.len()))
        .collect()
}

#[allow(dead_code)]
pub fn find_heading_range(source: &str, requested_anchor: &str) -> Option<(usize, usize)> {
    let requested = slugify(requested_anchor.trim_start_matches('#'));
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = without_newline.trim_start();
        let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=6).contains(&hashes) {
            let heading = trimmed[hashes..].trim_start();
            if slugify(heading) == requested {
                let line_start = offset + without_newline.len() - trimmed.len();
                let heading_start = line_start + hashes + (trimmed[hashes..].len() - heading.len());
                return Some((heading_start, heading_start + heading.len()));
            }
        }
        offset += line.len();
    }
    None
}

#[allow(dead_code)]
fn slugify(value: &str) -> String {
    let mut output = String::new();
    let mut previous_dash = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            output.extend(ch.to_lowercase());
            previous_dash = false;
        } else if (ch.is_whitespace() || ch == '-') && !output.is_empty() && !previous_dash {
            output.push('-');
            previous_dash = true;
        }
    }
    output.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewBlockKind, find_heading_range, find_matches, preview_document, toggle_task_at_offset,
    };

    #[test]
    fn parses_commonmark_and_gfm_blocks() {
        let document = preview_document(
            "# Heading\n\ntext **bold** and [link](https://example.com).\n\n- one\n- two\n\n> quote\n\n```rust\nlet x = 1;\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n![alt](image.png)\n\n---",
        );
        assert!(matches!(
            document.blocks[0].kind,
            PreviewBlockKind::Heading(1)
        ));
        assert!(matches!(document.blocks[1].kind, PreviewBlockKind::Body));
        assert!(matches!(
            document.blocks[2].kind,
            PreviewBlockKind::List(false)
        ));
        assert!(matches!(document.blocks[3].kind, PreviewBlockKind::Quote));
        assert!(matches!(document.blocks[4].kind, PreviewBlockKind::Code));
        assert!(matches!(document.blocks[5].kind, PreviewBlockKind::Table));
        assert!(matches!(document.blocks[6].kind, PreviewBlockKind::Image));
        assert!(matches!(document.blocks[7].kind, PreviewBlockKind::Rule));
    }

    #[test]
    fn parses_tasks_and_mermaid_without_line_scanning() {
        let source = "- [ ] todo\n- [x] done\n\n```mermaid\nflowchart TD\n A --> B\n```";
        let blocks = preview_document(source).blocks;
        assert_eq!(blocks[0].kind, PreviewBlockKind::Task(false));
        assert_eq!(blocks[1].kind, PreviewBlockKind::Task(true));
        assert_eq!(blocks[0].task_offset, Some(0));
        assert_eq!(blocks[1].task_offset, source.find("- [x] done"));
        assert_eq!(blocks[2].kind, PreviewBlockKind::Mermaid);
        assert!(blocks[2].markdown.contains("flowchart"));
    }

    #[test]
    fn serializes_task_offsets_for_the_tauri_frontend() {
        let document = preview_document("- [ ] ship the fix\n");
        let value = serde_json::to_value(document).expect("preview document should serialize");
        let task = &value["blocks"][0];

        assert_eq!(task["taskOffset"], 0);
        assert!(task.get("task_offset").is_none());
    }

    #[test]
    fn serializes_source_ranges_as_utf8_offsets() {
        let source = "# A😀B\n\nA paragraph with naïve text.";
        let document = preview_document(source);
        let heading = &document.blocks[0];
        let range = heading
            .source_range
            .as_ref()
            .expect("heading should have a source range");

        assert_eq!(range.start, 0);
        assert_eq!(range.end, "# A😀B".len());
        let value = serde_json::to_value(document).expect("preview document should serialize");
        assert_eq!(value["blocks"][0]["sourceRange"]["start"], 0);
        assert!(
            value["blocks"][0]["sourceRange"]
                .get("source_range")
                .is_none()
        );
    }

    #[test]
    fn renders_nested_tasks_as_individual_checkboxes() {
        let source = "- [x] Parent\n  - [ ] Child one\n  - [x] Child two\n- [ ] Sibling";
        let blocks = preview_document(source).blocks;
        assert_eq!(
            blocks
                .iter()
                .map(|block| (block.kind.clone(), block.markdown.clone()))
                .collect::<Vec<_>>(),
            vec![
                (PreviewBlockKind::Task(true), "Parent".to_string()),
                (PreviewBlockKind::Task(false), "Child one".to_string()),
                (PreviewBlockKind::Task(true), "Child two".to_string()),
                (PreviewBlockKind::Task(false), "Sibling".to_string()),
            ]
        );
        assert_eq!(blocks[1].task_offset, source.find("- [ ] Child one"));
        assert_eq!(blocks[2].task_offset, source.find("- [x] Child two"));
        assert_eq!(
            blocks[0]
                .source_range
                .as_ref()
                .map(|range| (range.start, range.end)),
            Some((0, source.find('\n').unwrap()))
        );
        assert!(
            blocks[1].source_range.as_ref().unwrap().end
                <= blocks[2].source_range.as_ref().unwrap().start
        );
    }

    #[test]
    fn collects_images_in_document_order() {
        let refs = preview_document("![one](one.png)\n\n![two](two.png)").images;
        assert_eq!(
            refs.iter().map(|r| r.alt.as_str()).collect::<Vec<_>>(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn preserves_original_markdown_for_frontend_rendering() {
        let source = "# A \\*literal\\*\n\nA [link][note].\n\n> A quote with \\_literal\\_\n\n[note]: Other.md";
        let blocks = preview_document(source).blocks;
        assert_eq!(blocks[0].markdown, "# A \\*literal\\*");
        assert_eq!(blocks[1].markdown, "A [link][note].");
        assert_eq!(blocks[2].markdown, "> A quote with \\_literal\\_");
    }

    #[test]
    fn preserves_code_language_and_footnote_definitions() {
        let source = "```rust title=demo.rs\nfn main() {}\n```\n\n[^note]: Footnote **body**.\n\nA reference[^note].";
        let document = preview_document(source);
        assert_eq!(document.blocks[0].kind, PreviewBlockKind::Code);
        assert_eq!(document.blocks[0].language.as_deref(), Some("rust"));
        assert_eq!(document.blocks[1].kind, PreviewBlockKind::Footnote);
        assert_eq!(document.blocks[1].footnote_id.as_deref(), Some("note"));
        assert_eq!(document.blocks[1].markdown, "[^note]: Footnote **body**.\n");
        assert_eq!(document.blocks[2].markdown, "A reference[^note].");
    }

    #[test]
    fn preserves_soft_breaks_between_links() {
        let source = "[Index](../undefined)\n[MattMC Main Page](../undefined)";
        let blocks = preview_document(source).blocks;
        assert_eq!(blocks[0].markdown, source);
    }

    #[test]
    fn preserves_reference_source_and_resolves_images_for_rendering() {
        let source =
            "[Index][home]\n\n![Logo][logo]\n\n[home]: ../index.md\n[logo]: images/logo.png";
        let document = preview_document(source);
        assert_eq!(document.blocks[0].markdown, "[Index][home]");
        assert_eq!(
            document.link_definitions,
            vec!["[home]: ../index.md", "[logo]: images/logo.png"]
        );
        let serialized = serde_json::to_value(&document).expect("document should serialize");
        assert!(serialized.get("linkDefinitions").is_some());
        assert!(serialized.get("link_definitions").is_none());
        assert_eq!(document.blocks[1].kind, PreviewBlockKind::Image);
        assert_eq!(
            document.blocks[1]
                .image
                .as_ref()
                .map(|image| image.destination.as_str()),
            Some("images/logo.png")
        );
    }

    #[test]
    fn keeps_html_available_for_frontend_sanitization() {
        let blocks = preview_document("<mark>highlight</mark>").blocks;
        assert_eq!(blocks[0].kind, PreviewBlockKind::Body);
        assert_eq!(blocks[0].markdown, "<mark>highlight</mark>");
    }

    #[test]
    fn preserves_existing_navigation_helpers() {
        assert_eq!(find_matches("one two one", "one"), vec![(0, 3), (8, 11)]);
        assert_eq!(
            find_heading_range("# Hello World\n", "#hello-world"),
            Some((2, 13))
        );
    }

    #[test]
    fn toggles_tasks_without_modifying_fenced_examples() {
        let source = "- [ ] first\n\n```markdown\n- [ ] example\n```\n\n- [x] second\n";
        assert_eq!(
            toggle_task_at_offset(source, 0).as_deref(),
            Some("- [x] first\n\n```markdown\n- [ ] example\n```\n\n- [x] second\n")
        );
        assert_eq!(
            toggle_task_at_offset(source, source.rfind("- [x]").unwrap()).as_deref(),
            Some("- [ ] first\n\n```markdown\n- [ ] example\n```\n\n- [ ] second\n")
        );
    }
}
