use tracing::warn;

/// Maximum message length for Telegram messages.
const MAX_MESSAGE_LENGTH: usize = 4096;

/// Initial estimate for tag close/reopen overhead. The retry loop in
/// `fit_chunk` computes the exact overhead, so this just reduces the
/// number of retry iterations needed.
const TAG_OVERHEAD_ESTIMATE: usize = 300;

/// Split an HTML message into chunks that fit within Telegram's message limit.
///
/// Tries to split at newline boundaries for cleaner output. Avoids splitting
/// inside HTML tags and properly closes/reopens tags at chunk boundaries so
/// each chunk is valid HTML that Telegram can parse.
pub(crate) fn chunk_message(content: &str) -> Vec<String> {
    if content.len() <= MAX_MESSAGE_LENGTH {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;
    let mut open_tags: Vec<String> = Vec::new();

    while !remaining.is_empty() {
        let reopen_prefix = build_reopen_tags(&open_tags);

        if reopen_prefix.len() >= MAX_MESSAGE_LENGTH {
            // Extreme edge case: tag state alone exceeds the limit.
            // Drop tag state to guarantee forward progress (HTML will be malformed).
            warn!("HTML reopen tags exceed message limit, dropping tag state");
            open_tags.clear();
            continue;
        }

        if remaining.len() + reopen_prefix.len() <= MAX_MESSAGE_LENGTH {
            let mut chunk = reopen_prefix;
            chunk.push_str(remaining);
            chunks.push(chunk);
            break;
        }

        let available = MAX_MESSAGE_LENGTH
            .saturating_sub(TAG_OVERHEAD_ESTIMATE)
            .saturating_sub(reopen_prefix.len());

        let mut split_at = find_split_point(remaining, available);
        if split_at == 0 {
            split_at = split_after_leading_tag(remaining, available);
        }

        let (chunk, new_tags, consumed) =
            fit_chunk(remaining, split_at, &open_tags, &reopen_prefix);
        chunks.push(chunk);
        open_tags = new_tags;
        remaining = &remaining[consumed..];
    }

    chunks
}

/// Retry-shrink a candidate split until the assembled chunk (reopen + text +
/// close) fits within `MAX_MESSAGE_LENGTH`. Returns the built chunk, the
/// updated open-tag stack, and the number of bytes consumed from `remaining`.
fn fit_chunk(
    remaining: &str,
    initial_split: usize,
    open_tags: &[String],
    reopen_prefix: &str,
) -> (String, Vec<String>, usize) {
    let mut split_at = initial_split;

    loop {
        // Guarantee forward progress: if we've backed up to <=1, force 0
        // so the escape clause fires on this iteration.
        if split_at <= 1 && split_at < initial_split {
            split_at = 0;
        }

        let chunk_text = &remaining[..split_at];
        let mut new_open_tags = open_tags.to_vec();
        update_tag_stack(&mut new_open_tags, chunk_text);
        let close_suffix = build_close_tags(&new_open_tags);

        let total_len = reopen_prefix.len() + chunk_text.len() + close_suffix.len();
        if total_len <= MAX_MESSAGE_LENGTH {
            let mut chunk = String::with_capacity(total_len);
            chunk.push_str(reopen_prefix);
            chunk.push_str(chunk_text);
            chunk.push_str(&close_suffix);

            // Advance past split point, skip newline if we split at one
            let consumed = if remaining[split_at..].starts_with('\n') {
                split_at + 1
            } else {
                split_at
            };
            return (chunk, new_open_tags, consumed);
        }

        if split_at == 0 {
            // Last-resort fallback: emit a raw chunk to guarantee progress.
            warn!("Unable to fit HTML tags within message limit, dropping tag state");
            let text_len = remaining
                .floor_char_boundary(MAX_MESSAGE_LENGTH.min(remaining.len()))
                .max(1);
            let chunk = remaining[..text_len].to_string();
            let consumed = if remaining[text_len..].starts_with('\n') {
                text_len + 1
            } else {
                text_len
            };
            return (chunk, Vec::new(), consumed);
        }

        let next_split = previous_split_point(remaining, split_at);
        split_at = if next_split < split_at {
            next_split
        } else {
            remaining
                .floor_char_boundary(split_at.saturating_sub(1))
                .max(1)
        };
    }
}

fn build_reopen_tags(tags: &[String]) -> String {
    let mut s = String::new();
    for tag in tags {
        s.push('<');
        s.push_str(tag);
        s.push('>');
    }
    s
}

fn build_close_tags(tags: &[String]) -> String {
    let mut s = String::new();
    for tag in tags.iter().rev() {
        s.push_str("</");
        s.push_str(html_tag_name(tag));
        s.push('>');
    }
    s
}

fn find_split_point(s: &str, max_len: usize) -> usize {
    let boundary = s.floor_char_boundary(max_len);
    let split_at = s[..boundary].rfind('\n').unwrap_or(boundary);
    avoid_splitting_html_tag(s, split_at)
}

fn previous_split_point(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let split_at = s[..pos].rfind('\n').unwrap_or(pos.saturating_sub(1));
    let split_at = s.floor_char_boundary(split_at);
    avoid_splitting_html_tag(s, split_at)
}

/// When `find_split_point` returns 0 (e.g. remaining starts with a long tag),
/// skip past the leading tag's `>` so we don't get stuck.
fn split_after_leading_tag(s: &str, max_len: usize) -> usize {
    if s.starts_with('<')
        && let Some(end) = s.find('>')
    {
        let end = end + 1;
        if end <= max_len {
            return end.max(1);
        }
    }
    s.floor_char_boundary(max_len.max(1)).max(1)
}

/// If `pos` falls inside an HTML tag (`<...>`), move it before the `<`.
fn avoid_splitting_html_tag(s: &str, pos: usize) -> usize {
    let before = &s[..pos];
    match (before.rfind('<'), before.rfind('>')) {
        (Some(open), Some(close)) if open > close => open,
        (Some(open), None) => open,
        _ => pos,
    }
}

/// Scan `html` for open/close tags and update the stack accordingly.
fn update_tag_stack(stack: &mut Vec<String>, html: &str) {
    let mut pos = 0;
    while pos < html.len() {
        let Some(rel_start) = html[pos..].find('<') else {
            break;
        };
        let abs_start = pos + rel_start;
        let Some(rel_end) = html[abs_start..].find('>') else {
            break;
        };
        let abs_end = abs_start + rel_end;
        let inner = &html[abs_start + 1..abs_end];

        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim();
            if let Some(idx) = stack.iter().rposition(|t| html_tag_name(t) == name) {
                stack.remove(idx);
            }
        } else if !inner.is_empty() && !inner.ends_with('/') {
            stack.push(inner.to_string());
        }

        pos = abs_end + 1;
    }
}

/// Extract the tag name from full tag content (e.g. `a href="url"` → `a`).
fn html_tag_name(full_tag: &str) -> &str {
    full_tag.split_whitespace().next().unwrap_or(full_tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_message_unchanged() {
        let chunks = chunk_message("hello");
        assert_eq!(chunks, vec!["hello".to_string()]);
    }

    #[test]
    fn does_not_split_inside_html_tag() {
        let mut content = "x".repeat(MAX_MESSAGE_LENGTH - 3);
        content.push_str("<b>bold</b>");
        let chunks = chunk_message(&content);
        for chunk in &chunks {
            let mut depth = 0i32;
            for ch in chunk.chars() {
                if ch == '<' {
                    depth += 1;
                }
                if ch == '>' {
                    depth -= 1;
                }
            }
            assert_eq!(depth, 0, "unbalanced angle brackets in chunk");
        }
    }

    #[test]
    fn balances_html_tags() {
        let filler = "x".repeat(MAX_MESSAGE_LENGTH + 100);
        let content = format!("<b>{}</b>", filler);
        let chunks = chunk_message(&content);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            let opens = chunk.matches("<b>").count();
            let closes = chunk.matches("</b>").count();
            assert_eq!(opens, closes, "unbalanced <b> tags in chunk");
        }
    }

    #[test]
    fn respects_max_length_with_nested_tags() {
        let filler = "x".repeat(MAX_MESSAGE_LENGTH * 2);
        let content = format!("<b><i>{}</i></b>", filler);
        let chunks = chunk_message(&content);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| c.len() <= MAX_MESSAGE_LENGTH));
        assert!(chunks.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn no_empty_chunks() {
        let filler = "x".repeat(MAX_MESSAGE_LENGTH * 3);
        let content = format!("<b><i><s>{}</s></i></b>", filler);
        let chunks = chunk_message(&content);
        assert!(chunks.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn avoid_split_inside_tag() {
        let s = "text <b>bold</b>";
        assert_eq!(avoid_splitting_html_tag(s, 6), 5);
    }

    #[test]
    fn avoid_split_outside_tag() {
        let s = "text <b>bold</b> more";
        assert_eq!(avoid_splitting_html_tag(s, 16), 16);
    }

    #[test]
    fn tag_stack_tracks_opens_and_closes() {
        let mut stack = Vec::new();
        update_tag_stack(&mut stack, "<b>text<i>more</i>");
        assert_eq!(stack, vec!["b".to_string()]);
    }

    #[test]
    fn tag_name_extracts_name() {
        assert_eq!(html_tag_name("b"), "b");
        assert_eq!(html_tag_name("a href=\"url\""), "a");
        assert_eq!(html_tag_name("code class=\"language-rust\""), "code");
    }
}
