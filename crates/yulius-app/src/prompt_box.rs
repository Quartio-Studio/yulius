//! Claude Code's prompt box, read from a session's screen (SPEC-488, ADR-0329).
//!
//! Yulius sees a person's keys, not what they left in the prompt box, and holds every typed
//! message while a key may have left a draft there (SPEC-480). Most keys leave none. The screen
//! can prove that: [`shown_empty`] says whether it shows the box with nothing typed in it. It
//! accepts one exact picture, the one Claude Code 2.1.323 draws full screen (seen with the real
//! tool in a PTY, through the same `vt100` parser), and everything else is no proof: a wrong
//! "empty" would get a message typed behind a person's text and submitted with it.

/// The prompt character at the start of the box's row.
const PROMPT: &str = "❯";
/// What the rows directly above and below the box consist of, across the whole width.
const RULE: &str = "─";
/// The column of the box's first cell, where Claude Code parks the cursor while the box is
/// empty: after the prompt character and a blank.
const FIRST_CELL: u16 = 2;

/// Whether `screen` shows Claude Code's prompt box with nothing typed in it (SPEC-488 EBX-002):
///
/// - the alternate screen (the full-screen renderer, the only one Yulius runs);
/// - the cursor in the box's first cell, on a row that starts with `❯` and a blank: Claude Code
///   parks it there while the box has the keys and is empty (in a dialog, a list or another
///   view the cursor is elsewhere, and a picture of a box in the conversation has no cursor);
/// - the rows directly above and below are rules across the whole width (the history search
///   labels its upper rule; a draft of several rows has no rule below its first);
/// - from the first cell on, the row holds nothing, or only dim text: what Claude Code draws
///   into an empty box by itself (its prompt suggestion, "Press up to edit queued messages"),
///   which a paste replaces. Typed text is never dim. The cell under the cursor is drawn
///   inverse while the terminal has the focus, so it counts as dim only when dim text follows
///   it: a one-character draft under the cursor looks the same and is followed by none.
#[must_use]
pub fn shown_empty(screen: &vt100::Screen) -> bool {
    if !screen.alternate_screen() {
        return false;
    }
    let (rows, cols) = screen.size();
    let (row, col) = screen.cursor_position();
    if col != FIRST_CELL || row == 0 || row >= rows.saturating_sub(1) || cols <= FIRST_CELL {
        return false;
    }
    let text = |r: u16, c: u16| screen.cell(r, c).map_or("", vt100::Cell::contents);
    let blank = |r: u16, c: u16| text(r, c).trim().is_empty();
    let rule = |r: u16| (0..cols).all(|c| text(r, c) == RULE);
    if text(row, 0) != PROMPT || !blank(row, 1) || !rule(row - 1) || !rule(row + 1) {
        return false;
    }
    let dim = |c: u16| screen.cell(row, c).is_some_and(vt100::Cell::dim);
    let rest_is_dim = (FIRST_CELL + 1..cols).all(|c| blank(row, c) || dim(c));
    let dim_follows = (FIRST_CELL + 1..cols).any(|c| !blank(row, c) && dim(c));
    let under_cursor_inverse = screen
        .cell(row, FIRST_CELL)
        .is_some_and(vt100::Cell::inverse);
    let first_is_nothing_typed =
        blank(row, FIRST_CELL) || dim(FIRST_CELL) || (under_cursor_inverse && dim_follows);
    rest_is_dim && first_is_nothing_typed
}

#[cfg(test)]
mod tests {
    use super::shown_empty;

    const COLS: u16 = 40;
    const ROWS: u16 = 12;
    /// Dim on, inverse on, attributes off.
    const DIM: &str = "\x1b[2m";
    const INV: &str = "\x1b[7m";
    const OFF: &str = "\x1b[0m";

    fn rule() -> String {
        "─".repeat(usize::from(COLS))
    }

    /// The screen after `bytes`, on the alternate screen unless `main`.
    fn drawn(bytes: &str, main: bool) -> vt100::Parser {
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        if !main {
            parser.process(b"\x1b[?1049h");
        }
        parser.process(b"\x1b[H\x1b[2J");
        parser.process(bytes.as_bytes());
        parser
    }

    /// Claude Code's lower screen: `above` (the row over the box), the box's `rows` between
    /// two rules, two status rows, and the cursor at (`row`, `col`), 1-based as in the
    /// sequence. The box's first row is row 9.
    fn boxed(rows: &[&str], cursor: (u16, u16)) -> String {
        let mut out = format!("\x1b[7;1Hsome answer of the agent\x1b[8;1H{}", rule());
        let mut at = 9;
        for row in rows {
            out.push_str(&format!("\x1b[{at};1H{row}{OFF}"));
            at += 1;
        }
        out.push_str(&format!(
            "\x1b[{at};1H{}\x1b[{};1H  Yulius #1 a task\x1b[{};1H  bypass permissions on",
            rule(),
            at + 1,
            at + 2
        ));
        out.push_str(&format!("\x1b[{};{}H", cursor.0, cursor.1));
        out
    }

    fn empty(bytes: &str) -> bool {
        shown_empty(drawn(bytes, false).screen())
    }

    /// SPEC-488 AC4: the pictures of an empty box, as Claude Code 2.1.323 draws them.
    #[test]
    fn an_empty_box_is_shown_empty() {
        // Focused: the caret is an inverse blank at the first cell.
        assert!(empty(&boxed(&[&format!("❯ {INV} ")], (9, 3))));
        // Unfocused: no caret is drawn.
        assert!(empty(&boxed(&["❯ "], (9, 3))));
        // With a hint in the row above the upper rule, and with the shortcut help below.
        let hinted = format!(
            "\x1b[6;10HCtrl+Y to paste deleted text{}",
            boxed(&["❯ "], (9, 3))
        );
        assert!(empty(&hinted));
        // Text that was typed and deleted again leaves blanks.
        assert!(empty(&boxed(&["❯ abc\x08\x08\x08   "], (9, 3))));
    }

    /// SPEC-488 AC4: what Claude Code draws into an empty box by itself is dim, with the cursor
    /// at the first cell: nothing was typed.
    #[test]
    fn a_suggestion_or_hint_is_nothing_typed() {
        // The suggestion, focused: its first character is inverse under the caret.
        let focused = format!("❯ {INV}r{OFF}{DIM}un the tests");
        assert!(empty(&boxed(&[&focused], (9, 3))));
        // Unfocused: dim throughout.
        let unfocused = format!("❯ {DIM}run the tests");
        assert!(empty(&boxed(&[&unfocused], (9, 3))));
        // The hint while prompts wait in Claude Code's queue (its prompt character is grey).
        let queued =
            format!("\x1b[38;5;246m❯{OFF} {INV}P{OFF}{DIM}ress up to edit queued messages");
        assert!(empty(&boxed(&[&queued], (9, 3))));
    }

    /// SPEC-488 AC4: a draft is never shown empty, wherever its cursor is.
    #[test]
    fn a_draft_is_not_shown_empty() {
        // The cursor behind the text.
        assert!(!empty(&boxed(
            &[&format!("❯ half a thought{INV} ")],
            (9, 18)
        )));
        // The cursor moved to the start: the first character inverse, the rest as typed.
        assert!(!empty(&boxed(
            &[&format!("❯ {INV}h{OFF}alf a thought")],
            (9, 3)
        )));
        // The same without the focus: no inverse cell.
        assert!(!empty(&boxed(&["❯ half a thought"], (9, 3))));
        // One character under the cursor: like a suggestion's first cell, with nothing dim
        // behind it.
        assert!(!empty(&boxed(&[&format!("❯ {INV}x")], (9, 3))));
        // A pasted block.
        let pasted = format!("❯ {INV}[{OFF}Pasted text #1 +39 lines]");
        assert!(!empty(&boxed(&[&pasted], (9, 3))));
        // Typed text behind a dim start (never seen; nothing typed is ever taken for dim).
        let mixed = format!("❯ {DIM}run{OFF} the tests");
        assert!(!empty(&boxed(&[&mixed], (9, 3))));
        // A first row left empty and text on the second: no rule below the first row.
        assert!(!empty(&boxed(&["❯ ", "  second line"], (9, 3))));
        assert!(!empty(&boxed(&["❯ ", "  second line"], (10, 14))));
        // A draft wrapped over two rows, the cursor at the start.
        assert!(!empty(&boxed(
            &["❯ a long draft that wraps around the", "  narrow width"],
            (9, 3)
        )));
        // Blanks typed: the cursor is behind them.
        assert!(!empty(&boxed(&["❯    "], (9, 6))));
    }

    /// SPEC-488 AC4: other states of Claude Code show no empty prompt box at the cursor.
    #[test]
    fn other_views_are_not_an_empty_box() {
        // Shell mode: another prompt character.
        assert!(!empty(&boxed(&["! "], (9, 3))));
        // The history search labels the upper rule (and shows the entry).
        let history = format!(
            "\x1b[8;1H─── History 1/1 {}\x1b[9;1H❯ \x1b[10;1H{}\x1b[9;3H",
            "─".repeat(usize::from(COLS) - 16),
            rule()
        );
        assert!(!empty(&history));
        // The rewind list: an indented marker, no rules, the cursor on it.
        let rewind = "\x1b[5;1H   Rewind\x1b[8;1H     hello\x1b[10;1H   ❯ (current)\
                      \x1b[12;1H   Enter to continue\x1b[10;4H";
        assert!(!empty(rewind));
        // A permission dialog over a conversation that shows a picture of an empty box.
        let dialog = format!(
            "\x1b[1;1H{}\x1b[2;1H❯ \x1b[3;1H{}\x1b[5;1H{}\x1b[6;1H Bash command\
             \x1b[8;1H Do you want to proceed?\x1b[9;1H ❯ 1. Yes\x1b[10;1H   2. No\x1b[9;2H",
            rule(),
            rule(),
            rule()
        );
        assert!(!empty(&dialog));
        // The transcript view: a rule and a footer, the cursor in the last row.
        let transcript = format!(
            "\x1b[11;1H{}\x1b[12;1H  Showing detailed transcript\x1b[12;1H",
            rule()
        );
        assert!(!empty(&transcript));
        // The agents view: its box holds a grey placeholder, not a dim one.
        let agents = format!("❯ {INV}d{OFF}\x1b[38;5;246mescribe a task for a new session");
        assert!(!empty(&boxed(&[&agents], (9, 3))));
        // The cursor elsewhere: in the conversation, in a status row, one cell further.
        assert!(!empty(&boxed(&["❯ "], (7, 3))));
        assert!(!empty(&boxed(&["❯ "], (11, 3))));
        assert!(!empty(&boxed(&["❯ "], (9, 4))));
        // No blank after the prompt character.
        assert!(!empty(&boxed(&["❯x"], (9, 3))));
        // A box whose lower rule is missing, or shorter than the screen.
        let open = format!("\x1b[8;1H{}\x1b[9;1H❯ \x1b[9;3H", rule());
        assert!(!empty(&open));
        let short = format!(
            "\x1b[8;1H{}\x1b[9;1H❯ \x1b[10;1H{}\x1b[9;3H",
            rule(),
            "─".repeat(usize::from(COLS) - 1)
        );
        assert!(!empty(&short));
        // A box in the last rows of the screen, with nothing below it.
        let last = format!("\x1b[11;1H{}\x1b[12;1H❯ \x1b[12;3H", rule());
        assert!(!empty(&last));
        // The same picture on the main screen, which was not probed.
        assert!(!shown_empty(drawn(&boxed(&["❯ "], (9, 3)), true).screen()));
        // Nothing drawn at all.
        assert!(!empty(""));
    }
}
