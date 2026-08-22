use markdown::{Constructs, ParseOptions, mdast::Node, to_mdast};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageReference {
    pub alt: String,
    pub destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewBlock {
    pub kind: PreviewBlockKind,
    pub markdown: String,
    pub image: Option<ImageReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewDocument {
    pub blocks: Vec<PreviewBlock>,
    pub images: Vec<ImageReference>,
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
    let parsed_blocks: Vec<PreviewBlock> = match &tree {
        Node::Root(root) => root.children.iter().flat_map(block_from_node).collect(),
        node => block_from_node(node).collect(),
    };
    let blocks = parsed_blocks
        .into_iter()
        .flat_map(expand_task_list_block)
        .collect();
    PreviewDocument { blocks, images }
}

pub fn preview_blocks(source: &str) -> Vec<PreviewBlock> {
    preview_document(source).blocks
}

fn block_from_node(node: &Node) -> std::vec::IntoIter<PreviewBlock> {
    let block = match node {
        Node::Heading(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Heading(value.depth),
            markdown: inline_markdown(&value.children),
            image: None,
        }),
        Node::Paragraph(value)
            if value.children.len() == 1 && matches!(value.children[0], Node::Image(_)) =>
        {
            let Node::Image(image) = &value.children[0] else {
                unreachable!()
            };
            Some(PreviewBlock {
                kind: PreviewBlockKind::Image,
                markdown: image.alt.clone(),
                image: Some(ImageReference {
                    alt: image.alt.clone(),
                    destination: image.url.clone(),
                }),
            })
        }
        Node::Paragraph(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Body,
            markdown: inline_markdown(&value.children),
            image: None,
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
            })
        }
        Node::Code(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
        }),
        Node::List(value) => {
            let all_tasks = value
                .children
                .iter()
                .all(|child| matches!(child, Node::ListItem(item) if item.checked.is_some()));
            if all_tasks && value.children.len() == 1 {
                if let Node::ListItem(item) = &value.children[0] {
                    Some(PreviewBlock {
                        kind: PreviewBlockKind::Task(item.checked.unwrap_or(false)),
                        markdown: item.children.iter().map(node_markdown).collect(),
                        image: None,
                    })
                } else {
                    None
                }
            } else {
                Some(PreviewBlock {
                    kind: PreviewBlockKind::List(value.ordered),
                    markdown: list_markdown(value),
                    image: None,
                })
            }
        }
        Node::Blockquote(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Quote,
            markdown: value
                .children
                .iter()
                .map(node_markdown)
                .collect::<Vec<_>>()
                .join("\n"),
            image: None,
        }),
        Node::ThematicBreak(_) => Some(PreviewBlock {
            kind: PreviewBlockKind::Rule,
            markdown: String::new(),
            image: None,
        }),
        Node::Image(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Image,
            markdown: value.alt.clone(),
            image: Some(ImageReference {
                alt: value.alt.clone(),
                destination: value.url.clone(),
            }),
        }),
        Node::Table(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Table,
            markdown: table_markdown(value),
            image: None,
        }),
        Node::Yaml(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
        }),
        Node::Toml(value) => Some(PreviewBlock {
            kind: PreviewBlockKind::Code,
            markdown: value.value.clone(),
            image: None,
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

fn expand_task_list_block(block: PreviewBlock) -> Vec<PreviewBlock> {
    if !matches!(block.kind, PreviewBlockKind::List(_)) {
        return vec![block];
    }

    let tasks = block
        .markdown
        .lines()
        .map(parse_task_line)
        .collect::<Option<Vec<_>>>();
    let Some(tasks) = tasks.filter(|tasks| !tasks.is_empty()) else {
        return vec![block];
    };

    tasks
        .into_iter()
        .map(|(checked, markdown)| PreviewBlock {
            kind: PreviewBlockKind::Task(checked),
            markdown,
            image: None,
        })
        .collect()
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

fn list_markdown(list: &markdown::mdast::List) -> String {
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
                item.children.iter().map(node_markdown).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_markdown(table: &markdown::mdast::Table) -> String {
    table
        .children
        .iter()
        .map(|row| {
            let Node::TableRow(row) = row else {
                return String::new();
            };
            row.children
                .iter()
                .map(node_markdown)
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn inline_markdown(nodes: &[Node]) -> String {
    nodes.iter().map(node_markdown).collect()
}

fn node_markdown(node: &Node) -> String {
    match node {
        Node::Paragraph(value) => inline_markdown(&value.children),
        Node::Heading(value) => inline_markdown(&value.children),
        Node::Blockquote(value) => value
            .children
            .iter()
            .map(node_markdown)
            .collect::<Vec<_>>()
            .join("\n"),
        Node::List(value) => list_markdown(value),
        Node::ListItem(value) => value.children.iter().map(node_markdown).collect(),
        Node::Code(value) => value.value.clone(),
        Node::ThematicBreak(_) => "---".to_string(),
        Node::Table(value) => table_markdown(value),
        Node::TableRow(value) => value
            .children
            .iter()
            .map(node_markdown)
            .collect::<Vec<_>>()
            .join(" | "),
        Node::TableCell(value) => inline_markdown(&value.children),
        Node::Text(value) => escape_styled_text(&value.value),
        Node::Emphasis(value) => format!("*{}*", inline_markdown(&value.children)),
        Node::Strong(value) => format!("**{}**", inline_markdown(&value.children)),
        Node::Delete(value) => format!("~~{}~~", inline_markdown(&value.children)),
        Node::InlineCode(value) => format!("`{}`", value.value),
        Node::Link(value) => format!("[{}]({})", inline_markdown(&value.children), value.url),
        Node::Image(value) => format!("![{}]({})", value.alt, value.url),
        Node::Break(_) => "\n".to_string(),
        Node::InlineMath(value) => value.value.clone(),
        Node::FootnoteReference(value) => format!("[{}]", value.identifier),
        Node::Html(value) => escape_styled_text(&value.value),
        _ => String::new(),
    }
}

fn escape_styled_text(value: &str) -> String {
    value.replace('<', "&lt;").replace('>', "&gt;")
}

pub fn image_references(source: &str) -> Vec<ImageReference> {
    preview_document(source).images
}

pub fn find_matches(source: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    source
        .match_indices(query)
        .map(|(start, matched)| (start, start + matched.len()))
        .collect()
}

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

fn slugify(value: &str) -> String {
    let mut output = String::new();
    let mut previous_dash = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            output.extend(ch.to_lowercase());
            previous_dash = false;
        } else if ch.is_whitespace() || ch == '-' {
            if !output.is_empty() && !previous_dash {
                output.push('-');
                previous_dash = true;
            }
        }
    }
    output.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewBlockKind, find_heading_range, find_matches, image_references, preview_blocks,
        preview_document,
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
        let blocks =
            preview_blocks("- [ ] todo\n- [x] done\n\n```mermaid\nflowchart TD\n A --> B\n```");
        assert_eq!(blocks[0].kind, PreviewBlockKind::Task(false));
        assert_eq!(blocks[1].kind, PreviewBlockKind::Task(true));
        assert_eq!(blocks[2].kind, PreviewBlockKind::Mermaid);
        assert!(blocks[2].markdown.contains("flowchart"));
    }

    #[test]
    fn collects_images_in_document_order() {
        let refs = image_references("![one](one.png)\n\n![two](two.png)");
        assert_eq!(
            refs.iter().map(|r| r.alt.as_str()).collect::<Vec<_>>(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn preserves_existing_navigation_helpers() {
        assert_eq!(find_matches("one two one", "one"), vec![(0, 3), (8, 11)]);
        assert_eq!(
            find_heading_range("# Hello World\n", "#hello-world"),
            Some((2, 13))
        );
    }
}
