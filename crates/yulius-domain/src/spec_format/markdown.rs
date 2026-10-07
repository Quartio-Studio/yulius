//! Minimal Markdown scanning for the format validator: lines, fenced code blocks, HTML
//! comments, ATX headings and checklist items. It is deliberately not a full CommonMark
//! parser; it recognizes exactly the constructs the enforced format relies on.

/// One source line with its classification.
pub(crate) struct Line<'a> {
    /// 1-based line number.
    pub number: usize,
    /// Raw text without the line terminator.
    pub text: &'a str,
    /// Inside a fenced code block, including the fence lines themselves.
    pub code: bool,
    /// An opening or closing fence line.
    pub fence: bool,
    /// For an opening fence: its info string (for example `mermaid`).
    pub fence_info: Option<&'a str>,
    /// Text outside HTML comments (empty for code lines).
    pub content: String,
}

struct Fence {
    marker: u8,
    length: usize,
}

fn fence(text: &str) -> Option<(Fence, &str)> {
    let indent = text.len() - text.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &text[indent..];
    let marker = *rest.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = rest.bytes().take_while(|b| *b == marker).count();
    if length < 3 {
        return None;
    }
    let info = rest[length..].trim();
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some((Fence { marker, length }, info))
}

/// Splits Markdown into classified lines.
pub(crate) fn scan(markdown: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut open: Option<Fence> = None;
    let mut in_comment = false;
    for (index, raw) in markdown.lines().enumerate() {
        let text = if index == 0 {
            raw.strip_prefix('\u{feff}').unwrap_or(raw)
        } else {
            raw
        };
        let number = index + 1;
        if let Some(current) = &open {
            let closes = fence(text).is_some_and(|(candidate, info)| {
                candidate.marker == current.marker
                    && candidate.length >= current.length
                    && info.is_empty()
            });
            if closes {
                open = None;
            }
            lines.push(Line {
                number,
                text,
                code: true,
                fence: closes,
                fence_info: None,
                content: String::new(),
            });
            continue;
        }
        if !in_comment && let Some((opening, info)) = fence(text) {
            open = Some(opening);
            lines.push(Line {
                number,
                text,
                code: true,
                fence: true,
                fence_info: Some(info),
                content: String::new(),
            });
            continue;
        }
        let mut content = String::new();
        let mut rest = text;
        loop {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        rest = &rest[end + 3..];
                        in_comment = false;
                    }
                    None => break,
                }
            } else {
                match rest.find("<!--") {
                    Some(start) => {
                        content.push_str(&rest[..start]);
                        rest = &rest[start + 4..];
                        in_comment = true;
                    }
                    None => {
                        content.push_str(rest);
                        break;
                    }
                }
            }
        }
        lines.push(Line {
            number,
            text,
            code: false,
            fence: false,
            fence_info: None,
            content,
        });
    }
    lines
}

/// An ATX heading: level 1–6 and its title (closing `#`s removed).
pub(crate) fn heading(text: &str) -> Option<(usize, &str)> {
    let indent = text.len() - text.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &text[indent..];
    let level = rest.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &rest[level..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let mut title = after.trim();
    let without_closing = title.trim_end_matches('#');
    if without_closing.is_empty() || without_closing.ends_with([' ', '\t']) {
        title = without_closing.trim_end();
    }
    Some((level, title))
}

/// Normalized heading title for case-insensitive section matching.
pub(crate) fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(':')
        .trim_end()
        .to_lowercase()
}

/// A checklist item: nesting depth (two columns per level, tabs count four), checked flag and
/// item text (trimmed; may be empty).
pub(crate) fn checklist_item(text: &str) -> Option<(usize, bool, &str)> {
    let rest = text.trim_start_matches([' ', '\t']);
    let columns: usize = text[..text.len() - rest.len()]
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let after_marker = if let Some(stripped) = rest.strip_prefix(['-', '*', '+']) {
        stripped
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        rest[digits..].strip_prefix(['.', ')'])?
    };
    let box_text = after_marker.strip_prefix([' ', '\t'])?.trim_start();
    let checked = match box_text.get(..3) {
        Some("[ ]") => false,
        Some("[x]" | "[X]") => true,
        _ => return None,
    };
    let item = &box_text[3..];
    if !item.is_empty() && !item.starts_with([' ', '\t']) {
        return None;
    }
    Some((columns / 2, checked, item.trim()))
}

/// Whether a non-code line carries content (not blank, not only comments, not a heading).
pub(crate) fn is_meaningful(line: &Line<'_>) -> bool {
    if line.code {
        return !line.fence && !line.text.trim().is_empty();
    }
    let content = line.content.trim();
    !content.is_empty() && heading(content).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_and_comments() {
        let text = "a\n```mermaid\n## not a heading\n```\n<!-- hidden\n## hidden -->visible\n~~~~\n~~~\n~~~~\nz";
        let lines = scan(text);
        let code: Vec<bool> = lines.iter().map(|line| line.code).collect();
        assert_eq!(
            code,
            vec![
                false, true, true, true, false, false, true, true, true, false
            ]
        );
        assert_eq!(lines[1].fence_info, Some("mermaid"));
        assert_eq!(lines[4].content, "");
        assert_eq!(lines[5].content, "visible");
        assert_eq!(lines[9].number, 10);
    }

    #[test]
    fn headings() {
        assert_eq!(heading("## Goal"), Some((2, "Goal")));
        assert_eq!(heading("   ### Sub ###"), Some((3, "Sub")));
        assert_eq!(heading("## C# usage"), Some((2, "C# usage")));
        assert_eq!(heading("##"), Some((2, "")));
        assert_eq!(heading("#hashtag"), None);
        assert_eq!(heading("    ## code"), None);
        assert_eq!(heading("####### seven"), None);
        assert_eq!(
            normalize_title("  Acceptance   Criteria: "),
            "acceptance criteria"
        );
    }

    #[test]
    fn checklist_items() {
        assert_eq!(
            checklist_item("- [ ] Write spec"),
            Some((0, false, "Write spec"))
        );
        assert_eq!(checklist_item("  * [x] Done"), Some((1, true, "Done")));
        assert_eq!(checklist_item("\t+ [X] Tab"), Some((2, true, "Tab")));
        assert_eq!(checklist_item("1. [ ] First"), Some((0, false, "First")));
        assert_eq!(
            checklist_item("12) [x] Twelfth"),
            Some((0, true, "Twelfth"))
        );
        assert_eq!(checklist_item("- [ ]"), Some((0, false, "")));
        assert_eq!(checklist_item("- [ ]x"), None);
        assert_eq!(checklist_item("- [y] no"), None);
        assert_eq!(checklist_item("-[ ] no space"), None);
        assert_eq!(checklist_item("- plain bullet"), None);
        assert_eq!(checklist_item("[ ] no marker"), None);
        assert_eq!(
            checklist_item("- [ ] [x] nested box"),
            Some((0, false, "[x] nested box"))
        );
        assert_eq!(checklist_item("- [ ] żółw"), Some((0, false, "żółw")));
    }
}
