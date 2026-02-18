use std::fmt::Write;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Convert Markdown to Telegram-compatible HTML.
///
/// Telegram supports a limited subset of HTML: `<b>`, `<i>`, `<s>`, `<code>`,
/// `<pre>`, `<a>`, `<blockquote>`. Unsupported elements (headings, lists, tables)
/// are converted to visual equivalents.
pub(crate) fn markdown_to_telegram_html(markdown: &str) -> String {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(markdown, options);

    let mut output = String::with_capacity(markdown.len());
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut code_block_has_lang = false;

    // Table state
    let mut in_table = false;
    let mut in_table_head = false;
    let mut table_header: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();

    for event in parser {
        // Handle table events separately to collect cell data
        if in_table {
            match event {
                Event::Start(Tag::TableHead) => in_table_head = true,
                Event::End(TagEnd::TableHead) => in_table_head = false,
                Event::Start(Tag::TableRow) => {}
                Event::End(TagEnd::TableRow) => {
                    if !in_table_head {
                        table_rows.push(std::mem::take(&mut current_row));
                    }
                }
                Event::Start(Tag::TableCell) => current_cell.clear(),
                Event::End(TagEnd::TableCell) => {
                    if in_table_head {
                        table_header.push(std::mem::take(&mut current_cell));
                    } else {
                        current_row.push(std::mem::take(&mut current_cell));
                    }
                }
                Event::End(TagEnd::Table) => {
                    render_table(&mut output, &table_header, &table_rows);
                    output.push('\n');
                    in_table = false;
                    table_header.clear();
                    table_rows.clear();
                }
                Event::Text(text) => current_cell.push_str(&text),
                Event::Code(code) => current_cell.push_str(&code),
                Event::SoftBreak | Event::HardBreak => current_cell.push(' '),
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { .. } => output.push_str("<b>"),
                Tag::Strong => output.push_str("<b>"),
                Tag::Emphasis => output.push_str("<i>"),
                Tag::Strikethrough => output.push_str("<s>"),
                Tag::CodeBlock(kind) => {
                    if let CodeBlockKind::Fenced(ref lang) = kind
                        && !lang.is_empty()
                    {
                        let _ = write!(
                            output,
                            "<pre><code class=\"language-{}\">",
                            escape_html(lang)
                        );
                        code_block_has_lang = true;
                    } else {
                        output.push_str("<pre>");
                        code_block_has_lang = false;
                    }
                }
                Tag::Link { dest_url, .. } => {
                    let _ = write!(output, "<a href=\"{}\">", escape_html(&dest_url));
                }
                Tag::BlockQuote(_) => output.push_str("<blockquote>"),
                Tag::List(first_item) => {
                    list_stack.push(match first_item {
                        Some(start) => ListKind::Ordered(start),
                        None => ListKind::Unordered,
                    });
                }
                Tag::Item => {
                    for _ in 1..list_stack.len() {
                        output.push_str("  ");
                    }
                    match list_stack.last_mut() {
                        Some(ListKind::Unordered) => output.push_str("• "),
                        Some(ListKind::Ordered(n)) => {
                            let _ = write!(output, "{}. ", n);
                            *n += 1;
                        }
                        None => {}
                    }
                }
                Tag::Table(_) => {
                    in_table = true;
                    table_header.clear();
                    table_rows.clear();
                    current_row.clear();
                }
                _ => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    if list_stack.is_empty() {
                        trim_trailing_whitespace(&mut output);
                        output.push_str("\n\n");
                    }
                }
                TagEnd::Heading(_) => {
                    output.push_str("</b>\n\n");
                }
                TagEnd::Strong => output.push_str("</b>"),
                TagEnd::Emphasis => output.push_str("</i>"),
                TagEnd::Strikethrough => output.push_str("</s>"),
                TagEnd::CodeBlock => {
                    if code_block_has_lang {
                        output.push_str("</code></pre>\n\n");
                    } else {
                        output.push_str("</pre>\n\n");
                    }
                    code_block_has_lang = false;
                }
                TagEnd::Link => output.push_str("</a>"),
                TagEnd::BlockQuote(_) => {
                    trim_trailing_whitespace(&mut output);
                    output.push_str("</blockquote>\n\n");
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    if list_stack.is_empty() {
                        output.push('\n');
                    }
                }
                TagEnd::Item => {
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                }
                _ => {}
            },

            Event::Text(text) => output.push_str(&escape_html(&text)),
            Event::Code(code) => {
                let _ = write!(output, "<code>{}</code>", escape_html(&code));
            }
            Event::SoftBreak => output.push('\n'),
            Event::HardBreak => output.push('\n'),
            Event::Rule => output.push_str("---\n\n"),
            Event::Html(html) | Event::InlineHtml(html) => {
                output.push_str(&escape_html(&html));
            }
            _ => {}
        }
    }

    output.trim_end().to_string()
}

enum ListKind {
    Ordered(u64),
    Unordered,
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn trim_trailing_whitespace(s: &mut String) {
    let trimmed_len = s.trim_end().len();
    s.truncate(trimmed_len);
}

fn render_table(output: &mut String, header: &[String], rows: &[Vec<String>]) {
    if header.is_empty() {
        return;
    }

    let num_cols = header.len();

    // Calculate column widths from unescaped text
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    let mut table = String::new();

    // Header row
    table.push('|');
    for (i, h) in header.iter().enumerate() {
        let _ = write!(table, " {:width$} |", h, width = widths[i]);
    }
    table.push('\n');

    // Separator
    table.push('|');
    for w in &widths {
        table.push_str(&"-".repeat(w + 2));
        table.push('|');
    }
    table.push('\n');

    // Data rows
    for row in rows {
        table.push('|');
        for (i, w) in widths.iter().enumerate().take(num_cols) {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let _ = write!(table, " {:width$} |", cell, width = *w);
        }
        table.push('\n');
    }

    // Escape the whole table and wrap in <pre>
    output.push_str("<pre>");
    output.push_str(&escape_html(&table));
    output.push_str("</pre>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        assert_eq!(markdown_to_telegram_html("hello world"), "hello world");
    }

    #[test]
    fn html_escaping() {
        assert_eq!(
            markdown_to_telegram_html("a < b & c > d"),
            "a &lt; b &amp; c &gt; d"
        );
    }

    #[test]
    fn bold() {
        assert_eq!(
            markdown_to_telegram_html("**bold text**"),
            "<b>bold text</b>"
        );
    }

    #[test]
    fn italic() {
        assert_eq!(
            markdown_to_telegram_html("*italic text*"),
            "<i>italic text</i>"
        );
    }

    #[test]
    fn inline_code() {
        assert_eq!(
            markdown_to_telegram_html("`some code`"),
            "<code>some code</code>"
        );
    }

    #[test]
    fn inline_code_with_special_chars() {
        assert_eq!(
            markdown_to_telegram_html("`a < b`"),
            "<code>a &lt; b</code>"
        );
    }

    #[test]
    fn fenced_code_block() {
        let input = "```\nfn main() {}\n```";
        let result = markdown_to_telegram_html(input);
        assert_eq!(result, "<pre>fn main() {}\n</pre>");
    }

    #[test]
    fn fenced_code_block_with_language() {
        let input = "```rust\nfn main() {}\n```";
        let result = markdown_to_telegram_html(input);
        assert_eq!(
            result,
            "<pre><code class=\"language-rust\">fn main() {}\n</code></pre>"
        );
    }

    #[test]
    fn strikethrough() {
        assert_eq!(markdown_to_telegram_html("~~deleted~~"), "<s>deleted</s>");
    }

    #[test]
    fn link() {
        assert_eq!(
            markdown_to_telegram_html("[click](https://example.com)"),
            "<a href=\"https://example.com\">click</a>"
        );
    }

    #[test]
    fn link_with_special_chars() {
        assert_eq!(
            markdown_to_telegram_html("[text](https://example.com?a=1&b=2)"),
            "<a href=\"https://example.com?a=1&amp;b=2\">text</a>"
        );
    }

    #[test]
    fn blockquote() {
        let result = markdown_to_telegram_html("> quoted text");
        assert_eq!(result, "<blockquote>quoted text</blockquote>");
    }

    #[test]
    fn heading() {
        assert_eq!(markdown_to_telegram_html("# Title"), "<b>Title</b>");
    }

    #[test]
    fn heading_h2() {
        assert_eq!(markdown_to_telegram_html("## Subtitle"), "<b>Subtitle</b>");
    }

    #[test]
    fn unordered_list() {
        let input = "- apple\n- banana\n- cherry";
        let result = markdown_to_telegram_html(input);
        assert_eq!(result, "• apple\n• banana\n• cherry");
    }

    #[test]
    fn ordered_list() {
        let input = "1. first\n2. second\n3. third";
        let result = markdown_to_telegram_html(input);
        assert_eq!(result, "1. first\n2. second\n3. third");
    }

    #[test]
    fn nested_list() {
        let input = "- outer\n  - inner\n- back";
        let result = markdown_to_telegram_html(input);
        assert!(result.contains("• outer"));
        assert!(result.contains("  • inner"));
        assert!(result.contains("• back"));
    }

    #[test]
    fn table() {
        let input = "| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        let result = markdown_to_telegram_html(input);
        assert!(result.starts_with("<pre>"));
        assert!(result.contains("</pre>"));
        assert!(result.contains("| A | B |"));
        assert!(result.contains("| 1 | 2 |"));
    }

    #[test]
    fn mixed_formatting() {
        let result = markdown_to_telegram_html("**bold** and *italic* and `code`");
        assert_eq!(
            result,
            "<b>bold</b> and <i>italic</i> and <code>code</code>"
        );
    }

    #[test]
    fn bold_italic() {
        let result = markdown_to_telegram_html("***bold italic***");
        assert!(result.contains("<b>"));
        assert!(result.contains("<i>"));
        assert!(result.contains("bold italic"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(markdown_to_telegram_html(""), "");
    }

    #[test]
    fn horizontal_rule() {
        let result = markdown_to_telegram_html("above\n\n---\n\nbelow");
        assert!(result.contains("---"));
        assert!(result.contains("above"));
        assert!(result.contains("below"));
    }

    #[test]
    fn code_block_html_escaping() {
        let input = "```\n<div>&amp;</div>\n```";
        let result = markdown_to_telegram_html(input);
        assert!(result.contains("&lt;div&gt;&amp;amp;&lt;/div&gt;"));
    }

    #[test]
    fn paragraphs() {
        let input = "first paragraph\n\nsecond paragraph";
        let result = markdown_to_telegram_html(input);
        assert_eq!(result, "first paragraph\n\nsecond paragraph");
    }
}
