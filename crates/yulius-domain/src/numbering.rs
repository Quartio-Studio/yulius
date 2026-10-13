//! Numbered records (SPEC-083 NUM-001): files whose names start with a sequence number, such as
//! decision records (`0066-<slug>.md`), specs (`SPEC-083-<slug>.md`) and migrations
//! (`0020_record_numbers.sql`).
//!
//! Parallel tasks each took the next free number of their own checkout, and the second to merge
//! failed its checks and renumbered, often more than once (81 decision records renamed in 36 of
//! 74 sessions, F 2026-10-13). Yulius gives the numbers out instead. This module reads a
//! directory's convention from its file names and picks the next free number; the application
//! gathers the names (the integration branch, every task's checkout) and the reservations.
//!
//! A numbered name is `<prefix><digits><rest>`:
//! - the prefix has no digits: ASCII letters, `-`, `_` and `.`, not starting with `.`, at most
//!   [`MAX_PREFIX_CHARS`] characters (`""`, `SPEC-`, `V`);
//! - one to [`MAX_DIGITS`] digits: a longer run is a timestamp, which needs no reservation;
//! - the rest is empty or starts with a separator (`-`, `_` or `.`); a date (`2026-10-13-x`) or a
//!   version (`1.2.0`) is not a sequence number.

use std::collections::BTreeMap;

/// Most digits of a sequence number; more is a timestamp (`20231012120000_x.rb`).
pub const MAX_DIGITS: usize = 7;
/// Longest prefix before the number.
pub const MAX_PREFIX_CHARS: usize = 24;

/// A file name that starts with a sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Numbered<'a> {
    /// The text before the number (`""`, `"SPEC-"`).
    pub prefix: &'a str,
    /// The number.
    pub number: u64,
    /// How many digits it was written with (`0066` has 4).
    pub width: usize,
    /// The text after the number (`"-a-task-is-owned-by-one-person.md"`).
    pub rest: &'a str,
}

/// Reads a file name (its last path segment) as a numbered name.
#[must_use]
pub fn parse(name: &str) -> Option<Numbered<'_>> {
    let name = name.rsplit('/').next().unwrap_or(name);
    let start = name.find(|c: char| c.is_ascii_digit())?;
    let prefix = &name[..start];
    let prefix_ok = prefix.len() <= MAX_PREFIX_CHARS
        && !prefix.starts_with('.')
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, '-' | '_' | '.'));
    if !prefix_ok {
        return None;
    }
    let digits = name[start..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(name.len() - start);
    if digits > MAX_DIGITS {
        return None;
    }
    let rest = &name[start + digits..];
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (None, _) => {}
        // A version (`1.2.0`) is no sequence.
        (Some('.'), Some(next)) if next.is_ascii_digit() => return None,
        (Some('-' | '_' | '.'), _) => {}
        _ => return None,
    }
    // A date (`2026-10-13-…`) is no sequence either.
    if digits == 4 && is_month_day(rest) {
        return None;
    }
    let number = name[start..start + digits].parse().ok()?;
    Some(Numbered {
        prefix,
        number,
        width: digits,
        rest,
    })
}

/// Whether `rest` starts like the month and day of a date: `-MM-DD` or `_MM_DD`.
fn is_month_day(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 6
        && matches!(bytes[0], b'-' | b'_')
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == bytes[0]
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_digit()
}

/// How a directory numbers its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Convention {
    /// The text before the number.
    pub prefix: String,
    /// The digits numbers are written with (zero-padded).
    pub width: usize,
    /// What follows the number in the newest file, with its words as `<slug>`
    /// (`"-<slug>.md"`, `"__<slug>.sql"`).
    pub tail: String,
}

impl Convention {
    /// `number` written the way this directory writes it (`0066`, `SPEC-083`).
    #[must_use]
    pub fn format(&self, number: u64) -> String {
        format!("{}{number:0width$}", self.prefix, width = self.width)
    }

    /// A file name for `number` with a placeholder slug (`0066-<slug>.md`).
    #[must_use]
    pub fn example(&self, number: u64) -> String {
        format!("{}{}", self.format(number), self.tail)
    }
}

/// The numbering convention of a directory's file `names`: of the names with `prefix` when given,
/// else of the largest group of names sharing a prefix (ties go to the prefix that sorts first).
/// `None` when no name is numbered that way.
#[must_use]
pub fn convention<'a>(
    names: impl IntoIterator<Item = &'a str>,
    prefix: Option<&str>,
) -> Option<Convention> {
    let mut groups: BTreeMap<&str, Vec<Numbered<'a>>> = BTreeMap::new();
    for numbered in names.into_iter().filter_map(parse) {
        groups.entry(numbered.prefix).or_default().push(numbered);
    }
    let group = match prefix {
        Some(prefix) => groups.remove(prefix)?,
        None => {
            // The largest group; on a tie the first prefix in order (BTreeMap iterates sorted,
            // and `max_by_key` keeps the last maximum, so iterate in reverse).
            let (_, group) = groups.into_iter().rev().max_by_key(|(_, g)| g.len())?;
            group
        }
    };
    let newest = group.iter().max_by_key(|n| n.number)?;
    let mut widths: BTreeMap<usize, usize> = BTreeMap::new();
    for numbered in &group {
        *widths.entry(numbered.width).or_default() += 1;
    }
    // The most common width; on a tie the wider one (`max_by_key` keeps the last maximum).
    let width = widths
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map_or(newest.width, |(width, _)| width);
    Some(Convention {
        prefix: newest.prefix.to_owned(),
        width,
        tail: tail_of(newest.rest),
    })
}

/// `-a-task-is-owned.md` → `-<slug>.md`; `__add_users.sql` → `__<slug>.sql`; `` → ``.
fn tail_of(rest: &str) -> String {
    let separator: String = rest
        .chars()
        .take_while(|c| matches!(c, '-' | '_'))
        .collect();
    let extension = rest.rfind('.').map(|dot| &rest[dot..]).filter(|ext| {
        ext.len() > 1 && ext.len() <= 12 && ext[1..].chars().all(|c| c.is_ascii_alphanumeric())
    });
    match (separator.is_empty(), extension) {
        (true, Some(ext)) if rest == ext => ext.to_owned(),
        (true, None) if rest.is_empty() => String::new(),
        (true, ext) => format!("-<slug>{}", ext.unwrap_or_default()),
        (false, ext) => format!("{separator}<slug>{}", ext.unwrap_or_default()),
    }
}

/// The numbers among `names` written with `prefix`.
#[must_use]
pub fn numbers<'a>(names: impl IntoIterator<Item = &'a str>, prefix: &str) -> Vec<u64> {
    names
        .into_iter()
        .filter_map(parse)
        .filter(|n| n.prefix == prefix)
        .map(|n| n.number)
        .collect()
}

/// The next free number after every taken one (1 when none is taken).
#[must_use]
pub fn next_free(taken: impl IntoIterator<Item = u64>) -> u64 {
    taken
        .into_iter()
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names_are_read_with_their_prefix_and_width() {
        let adr = parse("0066-merge-conflicts.md");
        assert_eq!(
            adr,
            Some(Numbered {
                prefix: "",
                number: 66,
                width: 4,
                rest: "-merge-conflicts.md"
            })
        );
        let spec = parse(".ai/specs/SPEC-083-fix-merge-conflicts.md");
        assert_eq!(
            spec.as_ref().map(|n| (n.prefix, n.number, n.width)),
            Some(("SPEC-", 83, 3))
        );
        let flyway = parse("V12__add_users.sql");
        assert_eq!(flyway.map(|n| (n.prefix, n.number)), Some(("V", 12)));
        assert_eq!(
            parse("0019_automation_start.sql").map(|n| n.number),
            Some(19)
        );
        assert_eq!(parse("42").map(|n| n.number), Some(42));
        // An ADR whose title starts with a digit is still numbered.
        assert_eq!(parse("0042-2fa-for-admins.md").map(|n| n.number), Some(42));
    }

    #[test]
    fn dates_timestamps_versions_and_plain_names_are_not_numbered() {
        for name in [
            "README.md",
            "2026-10-13-checkout-api.md",
            "2026_10_13_checkout.md",
            "20231012120000_create_users.rb",
            "v1.2.0.md",
            "1.2.md",
            ".0001-hidden.md",
            "x9y.md",
            "0001x.md",
            "a b 1-x.md",
        ] {
            assert_eq!(parse(name), None, "{name}");
        }
    }

    #[test]
    fn the_convention_is_the_largest_group_or_the_given_prefix() -> Result<(), &'static str> {
        let names = [
            "README.md",
            "0001-single-rust-binary.md",
            "0064-attachment-reads-have-their-own-bound.md",
            "0063-background-work.md",
        ];
        let adr = convention(names, None).ok_or("numbered")?;
        assert_eq!(adr.prefix, "");
        assert_eq!(adr.width, 4);
        assert_eq!(adr.format(65), "0065");
        assert_eq!(adr.example(65), "0065-<slug>.md");

        let specs = [
            "SPEC-001-yulius-product.md",
            "SPEC-082-fix-test-timeouts.md",
            "2026-10-07-checkout-api.md",
            "notes.md",
        ];
        let spec = convention(specs, None).ok_or("numbered")?;
        assert_eq!(spec.format(83), "SPEC-083");
        assert_eq!(convention(specs, Some("ADR-")), None, "no such group");
        assert_eq!(
            convention(["V1__init.sql", "V2__users.sql"], Some("V")).map(|c| c.example(3)),
            Some("V3__<slug>.sql".to_owned())
        );
        assert_eq!(convention(["README.md", "notes.md"], None), None);
        Ok(())
    }

    #[test]
    fn a_number_grows_past_its_width_and_the_common_width_wins() -> Result<(), &'static str> {
        let names = ["001-a.md", "002-b.md", "1000-c.md"];
        let c = convention(names, None).ok_or("numbered")?;
        assert_eq!(c.width, 3);
        assert_eq!(c.format(next_free(numbers(names, ""))), "1001");
        assert_eq!(c.format(7), "007");
        Ok(())
    }

    #[test]
    fn the_next_free_number_follows_every_taken_one() {
        assert_eq!(next_free([]), 1);
        assert_eq!(next_free([3, 64, 12]), 65);
        assert_eq!(next_free([u64::MAX]), u64::MAX);
        assert_eq!(
            numbers(["0001-a.md", "SPEC-009-b.md", "0007-c.md", "README.md"], ""),
            vec![1, 7]
        );
    }
}
