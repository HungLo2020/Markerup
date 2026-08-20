use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewBlock {
    pub kind: PreviewBlockKind,
    pub markdown: String,
}

pub fn preview_blocks(source: &str) -> Vec<PreviewBlock> {
    let mut blocks = Vec::new();
    let mut body = String::new();
    let mut fence: Option<(char, bool)> = None;

    for line in source.lines() {
        let trimmed = line.trim_start();
        let fence_info = fenced_info(trimmed);

        if let Some((marker, mermaid)) = fence {
            if mermaid {
                if fence_info.is_some_and(|(closing, _)| closing == marker) {
                    fence = None;
                    let mermaid_source = body.trim_end().to_string();
                    body.clear();
                    if !mermaid_source.is_empty() {
                        blocks.push(PreviewBlock {
                            kind: PreviewBlockKind::Mermaid,
                            markdown: mermaid_source,
                        });
                    }
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            } else {
                body.push_str(line);
                body.push('\n');
                if fence_info.is_some_and(|(closing, _)| closing == marker) {
                    fence = None;
                }
            }
            continue;
        }

        if let Some((marker, is_mermaid)) = fence_info {
            flush_body(&mut body, &mut blocks);
            fence = Some((marker, is_mermaid));
            if !is_mermaid {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }

        if let Some((level, text)) = parse_atx_heading(line) {
            flush_body(&mut body, &mut blocks);
            if !text.is_empty() {
                blocks.push(PreviewBlock {
                    kind: PreviewBlockKind::Heading(level),
                    markdown: text.to_string(),
                });
            }
            continue;
        }

        if let Some((checked, text)) = parse_task_item(line) {
            flush_body(&mut body, &mut blocks);
            blocks.push(PreviewBlock {
                kind: PreviewBlockKind::Task(checked),
                markdown: text.to_string(),
            });
            continue;
        }

        if line.trim().is_empty() {
            flush_body(&mut body, &mut blocks);
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }

    if let Some((_, true)) = fence {
        let mermaid_source = body.trim_end().to_string();
        body.clear();
        if !mermaid_source.is_empty() {
            blocks.push(PreviewBlock {
                kind: PreviewBlockKind::Mermaid,
                markdown: mermaid_source,
            });
        }
    }

    flush_body(&mut body, &mut blocks);
    blocks
}

fn flush_body(body: &mut String, blocks: &mut Vec<PreviewBlock>) {
    let markdown = body.trim_end().to_string();
    if !markdown.is_empty() {
        blocks.push(PreviewBlock { kind: PreviewBlockKind::Body, markdown });
    }
    body.clear();
}

fn fenced_info(trimmed: &str) -> Option<(char, bool)> {
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let marker_count = trimmed.chars().take_while(|ch| *ch == marker).count();
    if marker_count < 3 {
        return None;
    }
    let info = trimmed[marker.len_utf8() * marker_count..].trim();
    let language = info.split_whitespace().next().unwrap_or_default();
    Some((marker, language.eq_ignore_ascii_case("mermaid")))
}

fn parse_atx_heading(line: &str) -> Option<(u8, &str)> {
    let leading_spaces = line.len() - line.trim_start_matches(' ').len();
    if leading_spaces > 3 {
        return None;
    }
    let trimmed = line.trim_start_matches(' ');
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let remainder = &trimmed[hashes..];
    if !remainder.is_empty() && !remainder.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some((hashes as u8, remainder.trim()))
}

fn parse_task_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;

    if let Some(text) = rest.strip_prefix("[ ] ") {
        return Some((false, text));
    }
    if let Some(text) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
        return Some((true, text));
    }
    None
}


pub fn preview_markdown(source: &str) -> String {
    let parser = Parser::new_ext(source, Options::all());
    let mut output = String::new();
    let mut image: Option<(String, String)> = None;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let hashes = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                output.push_str(&"#".repeat(hashes));
                output.push(' ');
            }
            Event::End(TagEnd::Heading(_)) => output.push_str("\n\n"),
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(_)) => output.push_str("> "),
            Event::End(TagEnd::BlockQuote(_)) => output.push_str("\n\n"),
            Event::Start(Tag::CodeBlock(_)) => output.push_str("```\n"),
            Event::End(TagEnd::CodeBlock) => output.push_str("```\n\n"),
            Event::Start(Tag::List(_)) => {}
            Event::End(TagEnd::List(_)) => output.push('\n'),
            Event::Start(Tag::Item) => output.push_str("- "),
            Event::End(TagEnd::Item) => output.push('\n'),
            Event::Start(Tag::Emphasis) => output.push('*'),
            Event::End(TagEnd::Emphasis) => output.push('*'),
            Event::Start(Tag::Strong) => output.push_str("**"),
            Event::End(TagEnd::Strong) => output.push_str("**"),
            Event::Start(Tag::Strikethrough) => output.push_str("~~"),
            Event::End(TagEnd::Strikethrough) => output.push_str("~~"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                output.push('[');
                output.push_str(&format!("<markerup-link:{}>", escape_marker(&dest_url)));
            }
            Event::End(TagEnd::Link) => finish_link_marker(&mut output),
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some((String::new(), dest_url.into_string()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((alt, destination)) = image.take() {
                    output.push_str("[🖼 ");
                    output.push_str(if alt.is_empty() { "image" } else { &alt });
                    output.push_str("](");
                    output.push_str(&destination);
                    output.push(')');
                }
            }
            Event::Code(text) => {
                output.push('`');
                output.push_str(&text);
                output.push('`');
            }
            Event::Text(text) => {
                if let Some((alt, _)) = image.as_mut() {
                    alt.push_str(&text);
                } else {
                    output.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Rule => output.push_str("\n---\n"),
            Event::TaskListMarker(checked) => output.push_str(if checked { "[x] " } else { "[ ] " }),
            Event::Html(html) | Event::InlineHtml(html) => {
                output.push_str(&html.replace('<', "&lt;").replace('>', "&gt;"));
            }
            Event::FootnoteReference(name) => {
                output.push('[');
                output.push_str(&name);
                output.push(']');
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => output.push_str(&math),
            _ => {}
        }
    }
    output
}

fn finish_link_marker(output: &mut String) {
    if let Some(marker_start) = output.rfind("[<markerup-link:") {
        if let Some(marker_end_offset) = output[marker_start..].find('>') {
            let marker_end = marker_start + marker_end_offset;
            let marker = output[marker_start + 16..marker_end].to_string();
            output.replace_range(marker_start + 1..=marker_end, "");
            output.push_str("](");
            output.push_str(&unescape_marker(&marker));
            output.push(')');
        }
    }
}

pub fn image_references(source: &str) -> Vec<ImageReference> {
    let mut references = Vec::new();
    let mut current: Option<ImageReference> = None;
    for event in Parser::new_ext(source, Options::all()) {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                current = Some(ImageReference { alt: String::new(), destination: dest_url.into_string() });
            }
            Event::Text(text) => {
                if let Some(reference) = current.as_mut() {
                    reference.alt.push_str(&text);
                }
            }
            Event::End(TagEnd::Image) => {
                if let Some(reference) = current.take() {
                    references.push(reference);
                }
            }
            _ => {}
        }
    }
    references
}

pub fn find_matches(source: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    source.match_indices(query).map(|(start, matched)| (start, start + matched.len())).collect()
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

fn escape_marker(value: &str) -> String {
    value.replace('%', "%25").replace('>', "%3E")
}
fn unescape_marker(value: &str) -> String {
    value.replace("%3E", ">").replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use super::{find_heading_range, find_matches, image_references, preview_blocks, PreviewBlockKind};

    #[test]
    fn headings_keep_levels() {
        let blocks = preview_blocks("# Markerup\n\n### Smaller");
        assert_eq!(blocks[0].kind, PreviewBlockKind::Heading(1));
        assert_eq!(blocks[0].markdown, "Markerup");
        assert_eq!(blocks[1].kind, PreviewBlockKind::Heading(3));
    }

    #[test]
    fn task_items_keep_checked_state() {
        let blocks = preview_blocks("- [ ] todo\n- [x] done");
        assert_eq!(blocks[0].kind, PreviewBlockKind::Task(false));
        assert_eq!(blocks[0].markdown, "todo");
        assert_eq!(blocks[1].kind, PreviewBlockKind::Task(true));
        assert_eq!(blocks[1].markdown, "done");
    }

    #[test]
    fn headings_inside_code_fences_are_not_blocks() {
        let blocks = preview_blocks("```md\n# not a heading\n```");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, PreviewBlockKind::Body);
    }

    #[test]
    fn mermaid_fences_become_diagram_blocks_without_the_fence() {
        let blocks = preview_blocks("Intro\n\n```mermaid\nflowchart TD\n    A --> B\n```\n\nAfter");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1].kind, PreviewBlockKind::Mermaid);
        assert_eq!(blocks[1].markdown, "flowchart TD\n    A --> B");
    }

    #[test]
    fn mermaid_language_name_is_case_insensitive_and_supports_tildes() {
        let blocks = preview_blocks("~~~MERMAID\ngraph LR\n    A --> B\n~~~");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, PreviewBlockKind::Mermaid);
        assert_eq!(blocks[0].markdown, "graph LR\n    A --> B");
    }

    #[test]
    fn extracts_images() {
        let refs = image_references("![Diagram](images/test.png)");
        assert_eq!(refs[0].destination, "images/test.png");
    }

    #[test]
    fn finds_offsets() {
        assert_eq!(find_matches("one two one", "one"), vec![(0, 3), (8, 11)]);
    }

    #[test]
    fn finds_heading_anchor() {
        assert_eq!(find_heading_range("# Hello World\ntext\n", "hello-world"), Some((2, 13)));
    }
}
