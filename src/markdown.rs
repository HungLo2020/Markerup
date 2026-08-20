use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Convert CommonMark into the subset currently rendered by Slint's StyledText.
///
/// The source file is never changed. This is only a disposable preview string.
/// Unsupported block constructs are represented conservatively instead of
/// causing the whole preview to fall back to plain text.
pub fn preview_markdown(source: &str) -> String {
    let parser = Parser::new_ext(source, Options::all());
    let mut output = String::new();
    let mut heading: Option<HeadingLevel> = None;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(level);
                output.push_str("**");
            }
            Event::End(TagEnd::Heading(_)) => {
                output.push_str("**\n\n");
                heading = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(_)) => output.push_str("*"),
            Event::End(TagEnd::BlockQuote(_)) => output.push_str("*\n\n"),
            Event::Start(Tag::CodeBlock(_)) => output.push_str("\n"),
            Event::End(TagEnd::CodeBlock) => output.push_str("\n"),
            Event::Start(Tag::List(start)) => {
                if start.is_none() {
                    output.push('\n');
                }
            }
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
            Event::End(TagEnd::Link) => {
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
            Event::Code(text) => {
                output.push('`');
                output.push_str(&text);
                output.push('`');
            }
            Event::Text(text) => output.push_str(&text),
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Rule => output.push_str("\n────────\n"),
            Event::TaskListMarker(checked) => {
                output.push_str(if checked { "[x] " } else { "[ ] " });
            }
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

    if heading.is_some() {
        output.push_str("**");
    }
    output
}

fn escape_marker(value: &str) -> String {
    value.replace('%', "%25").replace('>', "%3E")
}

fn unescape_marker(value: &str) -> String {
    value.replace("%3E", ">").replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use super::preview_markdown;

    #[test]
    fn headings_are_kept_visible() {
        assert!(preview_markdown("# Markerup").contains("**Markerup**"));
    }

    #[test]
    fn links_are_preserved_for_styled_text_navigation() {
        let preview = preview_markdown("See [Redstone](RedstoneComputer.md).");
        assert!(preview.contains("[Redstone](RedstoneComputer.md)"));
    }
}
