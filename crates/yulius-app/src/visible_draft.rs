//! Detect a draft restored into Claude Code's full-screen prompt box (SPEC-537).

/// The focused conversation input in Claude Code 2.1.323. Its upper rule may carry
/// the session name after /rename. The agents view uses dim rules.
pub(crate) fn shown(screen: &vt100::Screen) -> bool {
    if !screen.alternate_screen() {
        return false;
    }
    let (rows, cols) = screen.size();
    let (row, col) = screen.cursor_position();
    let cell = |r, c| screen.cell(r, c);
    let text = |r, c| cell(r, c).map_or("", vt100::Cell::contents);
    let rule = |r, c| cell(r, c).is_some_and(|v| v.contents() == "─" && !v.dim());
    let Some(first) = (1..=row).rev().find(|&r| {
        text(r, 0) == "❯" && text(r, 1).trim().is_empty() && upper_rule(screen, r - 1, cols)
    }) else {
        return false;
    };
    let Some(bottom) = (first + 1..rows).find(|&r| (0..cols).all(|c| rule(r, c))) else {
        return false;
    };
    if row >= bottom || (row == first && col < 2) {
        return false;
    }
    // Even typed spaces advance the caret. In an empty input the caret is on its
    // first cell, including when Claude draws a dim suggestion there.
    if (row, col) != (first, 2) {
        return true;
    }
    let mut dim_follows = false;
    for r in first..bottom {
        let start = if r == first { 3 } else { 0 };
        for c in start..cols {
            if let Some(v) = cell(r, c)
                && !v.contents().trim().is_empty()
            {
                if !v.dim() {
                    return true;
                }
                dim_follows = true;
            }
        }
    }
    // A suggestion's first glyph is inverse under the caret while focused. A single
    // restored character under that caret has no dim suffix.
    cell(first, 2).is_some_and(|v| {
        !v.contents().trim().is_empty() && !v.dim() && !(v.inverse() && dim_follows)
    })
}

/// Either a solid rule or the same rule ending in " name ─" after /rename.
fn upper_rule(screen: &vt100::Screen, row: u16, cols: u16) -> bool {
    if screen
        .cell(row, cols - 1)
        .is_none_or(|cell| cell.contents() != "─")
        || (0..cols).any(|c| screen.cell(row, c).is_some_and(vt100::Cell::dim))
    {
        return false;
    }
    let Some(line) = screen.rows(0, cols).nth(usize::from(row)) else {
        return false;
    };
    let label = line.trim_start_matches('─');
    line.starts_with('─')
        && (label.is_empty()
            || label
                .strip_prefix(' ')
                .and_then(|s| s.strip_suffix(" ─"))
                .is_some_and(|name| !name.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::shown;

    // The shape and attributes of the input in the Claude Code 2.1.323 PTY probe.
    fn framed(lines: &[&str], cursor: (u16, u16), alt: bool) -> vt100::Parser {
        let mut parser = vt100::Parser::new(12, 40, 0);
        if alt {
            parser.process(b"\x1b[?1049h");
        }
        let rule = "─".repeat(40);
        let mut bytes = format!("\x1b[5;1H{rule}");
        for (offset, line) in lines.iter().enumerate() {
            bytes.push_str(&format!("\x1b[{};1H{line}\x1b[0m", 6 + offset));
        }
        bytes.push_str(&format!(
            "\x1b[{};1H{rule}\x1b[{};{}H",
            6 + lines.len(),
            cursor.0,
            cursor.1
        ));
        parser.process(bytes.as_bytes());
        parser
    }

    #[test]
    fn restored_text_is_visible_at_either_end_or_on_a_continuation() {
        for (lines, cursor) in [
            (vec!["❯\u{a0}my draft"], (6, 11)),
            (vec!["❯ \x1b[7mx"], (6, 3)),
            (vec!["❯ ", "  second line"], (7, 14)),
            (vec!["❯ ", "  second line"], (6, 3)),
            (vec!["❯    "], (6, 6)),
        ] {
            assert!(shown(framed(&lines, cursor, true).screen()), "{lines:?}");
        }
    }

    #[test]
    fn a_named_session_still_shows_its_draft() {
        let mut parser = framed(&["❯ my draft"], (6, 11), true);
        // /rename replaces the right end of the upper rule with the name.
        parser.process(b"\x1b[5;28H named task \x1b[6;11H");
        assert!(shown(parser.screen()));
    }

    #[test]
    fn a_partial_frame_or_an_agents_view_does_not_hold_a_message() {
        for redraw in [
            "\x1b[5;1H \x1b[6;3H",
            "\x1b[7;40H \x1b[6;3H",
            "\x1b[5;20H \x1b[6;3H",
            "\x1b[5;20H\x1b[2m─\x1b[0m\x1b[6;3H",
            "\x1b[5;1H\x1b[2m─\x1b[0m\x1b[6;3H",
        ] {
            let mut parser = framed(&["❯ describe a new session"], (6, 3), true);
            parser.process(redraw.as_bytes());
            assert!(!shown(parser.screen()), "{redraw:?}");
        }
    }

    #[test]
    fn empty_input_suggestions_and_transcript_text_do_not_veto() {
        for (row, cursor, alt) in [
            ("❯ ", (6, 3), true),
            ("❯ \x1b[7mr\x1b[0;2mun the tests", (6, 3), true),
            ("❯ \x1b[2mPress up to edit queued messages", (6, 3), true),
            ("❯ some old prompt", (10, 3), true),
            ("❯ some old prompt", (6, 3), false),
            ("Answer in a conversation", (6, 3), true),
        ] {
            assert!(!shown(framed(&[row], cursor, alt).screen()), "{row:?}");
        }
    }
}
