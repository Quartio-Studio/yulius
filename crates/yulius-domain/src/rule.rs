//! Start rules: a logical sentence over the state of other tasks of a sandbox (SPEC-010).
//!
//! `#12 done and (#13 done or #14 ended)`: atoms `#N [state]` joined by `and`, `or`, `not`
//! (`&&`, `||`, `!`) and parentheses; `not` binds tightest, then `and`, then `or`. Evaluation is
//! three-valued so one pass says whether the rule is true (start), unknown (keep waiting) or
//! false for good (needs a person).

use std::fmt;

/// Longest rule text accepted.
pub const MAX_RULE_CHARS: usize = 500;
/// Most atoms in one rule.
pub const MAX_ATOMS: usize = 16;
/// Deepest nesting of parentheses and `not`.
pub const MAX_DEPTH: usize = 8;

/// The state of a task an atom asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The task is Done (its agent finished and the work is evidently complete).
    Done,
    /// The task's pull request is merged.
    Merged,
    /// The task ended without more work coming: stopped, failed, closed or deleted.
    Ended,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Done => "done",
            State::Merged => "merged",
            State::Ended => "ended",
        }
    }
}

/// What is known about one task when a rule is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    /// Done now.
    pub done: bool,
    /// Pull request merged.
    pub merged: bool,
    /// Stopped, failed, closed or deleted (a deleted or unknown task is `ended`).
    pub ended: bool,
}

/// Three-valued truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// The rule holds now.
    True,
    /// The rule cannot hold any more.
    False,
    /// The rule may still become true.
    Unknown,
}

impl Truth {
    fn not(self) -> Truth {
        match self {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown => Truth::Unknown,
        }
    }
}

/// A parsed start rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// Task `number` is in `state`.
    Atom {
        /// Task number in the sandbox.
        number: u32,
        /// The asked state.
        state: State,
    },
    /// Negation.
    Not(Box<Rule>),
    /// All hold.
    And(Vec<Rule>),
    /// At least one holds.
    Or(Vec<Rule>),
}

/// Why a rule text was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError(pub String);

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RuleError {}

fn err<T>(text: impl Into<String>) -> Result<T, RuleError> {
    Err(RuleError(text.into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Not,
    Task(u32),
    Word(String),
}

fn lex(text: &str) -> Result<Vec<Token>, RuleError> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            '!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                tokens.push(Token::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                tokens.push(Token::Or);
                i += 2;
            }
            '#' => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && chars[end].is_ascii_digit() {
                    end += 1;
                }
                let digits: String = chars[start..end].iter().collect();
                match digits.parse::<u32>() {
                    Ok(n) if n > 0 => tokens.push(Token::Task(n)),
                    _ => return err("`#` must be followed by a task number, like #12"),
                }
                i = end;
            }
            _ if c.is_ascii_alphabetic() => {
                let mut end = i;
                while end < chars.len() && chars[end].is_ascii_alphabetic() {
                    end += 1;
                }
                let word: String = chars[i..end].iter().collect::<String>().to_lowercase();
                tokens.push(match word.as_str() {
                    "and" => Token::And,
                    "or" => Token::Or,
                    "not" => Token::Not,
                    _ => Token::Word(word),
                });
                i = end;
            }
            _ => return err(format!("unexpected `{c}` in the rule")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    atoms: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn or(&mut self, depth: usize) -> Result<Rule, RuleError> {
        let mut parts = vec![self.and(depth)?];
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            parts.push(self.and(depth)?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Rule::Or(parts)
        })
    }

    fn and(&mut self, depth: usize) -> Result<Rule, RuleError> {
        let mut parts = vec![self.not(depth)?];
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            parts.push(self.not(depth)?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Rule::And(parts)
        })
    }

    fn not(&mut self, depth: usize) -> Result<Rule, RuleError> {
        if depth > MAX_DEPTH {
            return err(format!("the rule is nested deeper than {MAX_DEPTH} levels"));
        }
        match self.peek().cloned() {
            Some(Token::Not) => {
                self.at += 1;
                Ok(Rule::Not(Box::new(self.not(depth + 1)?)))
            }
            Some(Token::Open) => {
                self.at += 1;
                let inner = self.or(depth + 1)?;
                if self.peek() != Some(&Token::Close) {
                    return err("a `(` is not closed");
                }
                self.at += 1;
                Ok(inner)
            }
            Some(Token::Task(number)) => {
                self.at += 1;
                self.atoms += 1;
                if self.atoms > MAX_ATOMS {
                    return err(format!("the rule names more than {MAX_ATOMS} tasks"));
                }
                let state = match self.peek() {
                    Some(Token::Word(word)) => {
                        let state = match word.as_str() {
                            "done" | "finished" => State::Done,
                            "merged" => State::Merged,
                            "ended" => State::Ended,
                            other => {
                                return err(format!(
                                    "unknown state `{other}`; use done, merged or ended"
                                ));
                            }
                        };
                        self.at += 1;
                        state
                    }
                    _ => State::Done,
                };
                Ok(Rule::Atom { number, state })
            }
            Some(Token::Word(word)) => err(format!("expected a task like #12, found `{word}`")),
            Some(Token::Close) => err("unexpected `)`"),
            Some(Token::And | Token::Or) => err("`and`/`or` needs a condition on both sides"),
            None => err("the rule ends where a condition was expected"),
        }
    }
}

impl Rule {
    /// Parses a rule text.
    ///
    /// # Errors
    /// [`RuleError`] naming the problem for an empty, too long, too deep, too wide or malformed
    /// rule.
    pub fn parse(text: &str) -> Result<Rule, RuleError> {
        let text = text.trim();
        if text.is_empty() {
            return err("the rule is empty");
        }
        if text.chars().count() > MAX_RULE_CHARS {
            return err(format!(
                "the rule is longer than {MAX_RULE_CHARS} characters"
            ));
        }
        let mut parser = Parser {
            tokens: lex(text)?,
            at: 0,
            atoms: 0,
        };
        let rule = parser.or(0)?;
        if let Some(token) = parser.peek() {
            return match token {
                Token::Close => err("unexpected `)`"),
                _ => err("two conditions need `and` or `or` between them"),
            };
        }
        Ok(rule)
    }

    /// The task numbers the rule names, ascending, without repeats.
    #[must_use]
    pub fn numbers(&self) -> Vec<u32> {
        fn walk(rule: &Rule, out: &mut Vec<u32>) {
            match rule {
                Rule::Atom { number, .. } => out.push(*number),
                Rule::Not(inner) => walk(inner, out),
                Rule::And(parts) | Rule::Or(parts) => parts.iter().for_each(|p| walk(p, out)),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Evaluates the rule against what is known about each named task.
    pub fn eval(&self, facts: &dyn Fn(u32) -> Facts) -> Truth {
        match self {
            Rule::Atom { number, state } => {
                let f = facts(*number);
                let holds = match state {
                    State::Done => f.done,
                    State::Merged => f.merged,
                    State::Ended => f.ended,
                };
                if holds {
                    Truth::True
                } else if f.ended {
                    // Nothing more happens to an ended task on its own.
                    Truth::False
                } else {
                    Truth::Unknown
                }
            }
            Rule::Not(inner) => inner.eval(facts).not(),
            Rule::And(parts) => {
                let all: Vec<Truth> = parts.iter().map(|p| p.eval(facts)).collect();
                if all.contains(&Truth::False) {
                    Truth::False
                } else if all.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else {
                    Truth::True
                }
            }
            Rule::Or(parts) => {
                let all: Vec<Truth> = parts.iter().map(|p| p.eval(facts)).collect();
                if all.contains(&Truth::True) {
                    Truth::True
                } else if all.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else {
                    Truth::False
                }
            }
        }
    }
}

impl fmt::Display for Rule {
    /// The canonical text: parentheses only where precedence needs them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn child(rule: &Rule, f: &mut fmt::Formatter<'_>, wrap: bool) -> fmt::Result {
            if wrap {
                write!(f, "({rule})")
            } else {
                write!(f, "{rule}")
            }
        }
        match self {
            Rule::Atom { number, state } => write!(f, "#{number} {}", state.word()),
            Rule::Not(inner) => {
                f.write_str("not ")?;
                child(
                    inner,
                    f,
                    !matches!(**inner, Rule::Atom { .. } | Rule::Not(_)),
                )
            }
            Rule::And(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" and ")?;
                    }
                    child(part, f, matches!(part, Rule::Or(_)))?;
                }
                Ok(())
            }
            Rule::Or(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" or ")?;
                    }
                    child(part, f, false)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: Facts = Facts {
        done: false,
        merged: false,
        ended: false,
    };
    const DONE: Facts = Facts {
        done: true,
        merged: false,
        ended: false,
    };
    const FAILED: Facts = Facts {
        done: false,
        merged: false,
        ended: true,
    };

    fn eval(text: &str, facts: &[(u32, Facts)]) -> Truth {
        let table = facts.to_vec();
        Rule::parse(text).map_or(Truth::Unknown, |rule| {
            rule.eval(&|n| {
                table
                    .iter()
                    .find(|(m, _)| *m == n)
                    .map_or(FAILED, |(_, f)| *f)
            })
        })
    }

    #[test]
    fn both_tasks_must_be_done_for_and() {
        assert_eq!(
            eval("#1 done and #2 done", &[(1, DONE), (2, RUNNING)]),
            Truth::Unknown
        );
        assert_eq!(
            eval("#1 done and #2 done", &[(1, DONE), (2, DONE)]),
            Truth::True
        );
        assert_eq!(eval("#1 and #2", &[(1, DONE), (2, FAILED)]), Truth::False);
    }

    #[test]
    fn either_task_is_enough_for_or() {
        assert_eq!(eval("#1 or #2", &[(1, RUNNING), (2, DONE)]), Truth::True);
        assert_eq!(
            eval("#1 or #2", &[(1, RUNNING), (2, RUNNING)]),
            Truth::Unknown
        );
        assert_eq!(eval("#1 || #2", &[(1, FAILED), (2, FAILED)]), Truth::False);
    }

    #[test]
    fn not_waits_until_the_task_ends() {
        assert_eq!(eval("not #1 done", &[(1, RUNNING)]), Truth::Unknown);
        assert_eq!(eval("!#1 done", &[(1, FAILED)]), Truth::True);
        assert_eq!(eval("not #1 done", &[(1, DONE)]), Truth::False);
        assert_eq!(eval("#1 ended", &[(1, FAILED)]), Truth::True);
    }

    #[test]
    fn and_binds_tighter_than_or_and_parentheses_override() {
        let facts = [(1, DONE), (2, FAILED), (3, FAILED)];
        assert_eq!(eval("#1 or #2 and #3", &facts), Truth::True);
        assert_eq!(eval("(#1 or #2) and #3", &facts), Truth::False);
    }

    #[test]
    fn a_deleted_task_counts_as_ended() {
        assert_eq!(eval("#9 done", &[]), Truth::False);
        assert_eq!(eval("#9 ended", &[]), Truth::True);
    }

    #[test]
    fn merged_is_its_own_state() {
        let merged = Facts {
            done: true,
            merged: true,
            ended: false,
        };
        assert_eq!(eval("#1 merged", &[(1, DONE)]), Truth::Unknown);
        assert_eq!(eval("#1 merged", &[(1, merged)]), Truth::True);
    }

    fn refusal(text: &str) -> String {
        Rule::parse(text)
            .err()
            .map_or_else(|| "accepted".into(), |e| e.0)
    }

    #[test]
    fn numbers_lists_each_task_once() -> Result<(), RuleError> {
        let rule = Rule::parse("#3 and (#1 or not #3) and #2 merged")?;
        assert_eq!(rule.numbers(), vec![1, 2, 3]);
        Ok(())
    }

    #[test]
    fn the_canonical_text_parses_back_to_the_same_rule() -> Result<(), RuleError> {
        for text in [
            "#1 and (#2 or #3)",
            "not (#1 and #2) or #3 merged",
            "!#1 && #2 finished",
            "#1 or #2 and not not #3",
        ] {
            let rule = Rule::parse(text)?;
            assert_eq!(Rule::parse(&rule.to_string())?, rule, "{text}");
        }
        Ok(())
    }

    #[test]
    fn malformed_rules_are_refused_with_a_reason() {
        for (text, reason) in [
            ("", "empty"),
            ("   ", "empty"),
            ("#1 and", "ends"),
            ("and #1", "both sides"),
            ("#1 #2", "between"),
            ("(#1 and #2", "not closed"),
            ("#1)", "unexpected `)`"),
            ("#0", "task number"),
            ("#", "task number"),
            ("#1 soon", "unknown state"),
            ("task", "expected a task"),
            ("1", "unexpected `1`"),
            ("#1 + #2", "unexpected `+`"),
            ("#99999999999", "task number"),
        ] {
            let error = refusal(text);
            assert!(error.contains(reason), "{text:?} gave {error:?}");
        }
    }

    #[test]
    fn the_limits_are_enforced() {
        let long = format!("#1 and {}", " ".repeat(MAX_RULE_CHARS));
        let long = format!("{long}#2");
        assert!(refusal(&long).contains("longer"));
        let wide = (1..=17)
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(" and ");
        assert!(refusal(&wide).contains("more than 16"));
        let sixteen = (1..=16)
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(" and ");
        assert!(Rule::parse(&sixteen).is_ok());
        let deep = format!("{}#1{}", "(".repeat(10), ")".repeat(10));
        assert!(refusal(&deep).contains("deeper"));
        assert!(Rule::parse(&"not ".repeat(10)).is_err());
    }
}
