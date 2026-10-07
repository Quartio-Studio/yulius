//! Session window parsing and the interactive window prompt (SPEC-001 IMP-003).

use std::io::{self, BufRead, Write};
use std::time::Duration;

/// Window used when the user accepts the prompt's default (two weeks, SPEC-001 §7.5).
pub const DEFAULT_SINCE: &str = "14d";
/// Longest accepted window in days.
pub const MAX_SINCE_DAYS: u64 = 3650;
/// Invalid answers tolerated by [`prompt_since`] before it gives up.
const MAX_PROMPT_ATTEMPTS: usize = 3;

/// An unusable window.
#[derive(Debug, thiserror::Error)]
pub enum SinceError {
    /// Not `<positive integer><h|d|w>`.
    #[error(
        "invalid window {0:?}: use a whole number followed by h, d or w (for example 14d, 36h, 2w)"
    )]
    Invalid(String),
    /// Longer than [`MAX_SINCE_DAYS`].
    #[error("window {0:?} is longer than {MAX_SINCE_DAYS} days")]
    TooLong(String),
    /// [`prompt_since`] received only invalid answers.
    #[error("no valid window entered after {MAX_PROMPT_ATTEMPTS} attempts")]
    TooManyAttempts,
    /// Terminal I/O failed.
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Parses a window such as `14d`, `36h` or `2w` (case-insensitive, surrounding whitespace
/// ignored) into a duration of at least one hour and at most [`MAX_SINCE_DAYS`] days.
///
/// # Errors
/// [`SinceError::Invalid`] or [`SinceError::TooLong`].
pub fn parse_since(text: &str) -> Result<Duration, SinceError> {
    let trimmed = text.trim();
    let invalid = || SinceError::Invalid(trimmed.to_owned());
    let unit = trimmed.chars().last().ok_or_else(invalid)?;
    let number = &trimmed[..trimmed.len() - unit.len_utf8()];
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let value: u64 = number
        .parse()
        .map_err(|_| SinceError::TooLong(trimmed.to_owned()))?;
    let hours_per_unit = match unit.to_ascii_lowercase() {
        'h' => 1,
        'd' => 24,
        'w' => 24 * 7,
        _ => return Err(invalid()),
    };
    if value == 0 {
        return Err(invalid());
    }
    let hours = value
        .checked_mul(hours_per_unit)
        .filter(|hours| *hours <= MAX_SINCE_DAYS * 24)
        .ok_or_else(|| SinceError::TooLong(trimmed.to_owned()))?;
    Ok(Duration::from_secs(hours * 3600))
}

/// Asks how far back to import sessions. An empty answer or end of input selects `default`;
/// invalid answers are explained and asked again, at most three times.
///
/// # Errors
/// [`SinceError::TooManyAttempts`], an invalid `default`, or terminal I/O failure.
pub fn prompt_since(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    default: &str,
) -> Result<Duration, SinceError> {
    for _ in 0..MAX_PROMPT_ATTEMPTS {
        write!(
            output,
            "Import Claude Code sessions from how far back? (e.g. 14d, 36h, 2w) [{default}]: "
        )?;
        output.flush()?;
        let mut answer = String::new();
        let read = input.read_line(&mut answer)?;
        let answer = answer.trim();
        if read == 0 || answer.is_empty() {
            if read == 0 {
                writeln!(output)?;
            }
            return parse_since(default);
        }
        match parse_since(answer) {
            Ok(window) => return Ok(window),
            Err(error) => writeln!(output, "{error}")?,
        }
    }
    Err(SinceError::TooManyAttempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3600;

    #[test]
    fn parses_hours_days_weeks() -> Result<(), SinceError> {
        assert_eq!(parse_since("14d")?, Duration::from_secs(14 * 24 * HOUR));
        assert_eq!(parse_since("36h")?, Duration::from_secs(36 * HOUR));
        assert_eq!(parse_since(" 2W ")?, Duration::from_secs(14 * 24 * HOUR));
        assert_eq!(parse_since("3650d")?, Duration::from_secs(3650 * 24 * HOUR));
        Ok(())
    }

    #[test]
    fn rejects_bad_windows() {
        for bad in [
            "", "d", "14", "14m", "1.5d", "-2d", "0d", "14 d", "x14d", "14dd", "１d",
        ] {
            assert!(
                matches!(parse_since(bad), Err(SinceError::Invalid(_))),
                "{bad:?}"
            );
        }
        for long in [
            "3651d",
            "522w",
            "99999999999999999999999d",
            "18446744073709551615w",
        ] {
            assert!(
                matches!(parse_since(long), Err(SinceError::TooLong(_))),
                "{long:?}"
            );
        }
    }

    #[test]
    fn prompt_uses_default_on_empty_and_eof() -> Result<(), SinceError> {
        let mut output = Vec::new();
        let window = prompt_since(&mut &b"\n"[..], &mut output, "14d")?;
        assert_eq!(window, Duration::from_secs(14 * 24 * HOUR));
        assert!(String::from_utf8_lossy(&output).contains("[14d]"));
        let window = prompt_since(&mut &b""[..], &mut Vec::new(), "36h")?;
        assert_eq!(window, Duration::from_secs(36 * HOUR));
        Ok(())
    }

    #[test]
    fn prompt_reasks_after_invalid_answers_and_gives_up() -> Result<(), SinceError> {
        let mut output = Vec::new();
        let window = prompt_since(&mut &b"soon\n7d\n"[..], &mut output, "14d")?;
        assert_eq!(window, Duration::from_secs(7 * 24 * HOUR));
        assert!(String::from_utf8_lossy(&output).contains("invalid window"));
        let result = prompt_since(&mut &b"a\nb\nc\n7d\n"[..], &mut Vec::new(), "14d");
        assert!(matches!(result, Err(SinceError::TooManyAttempts)));
        Ok(())
    }
}
