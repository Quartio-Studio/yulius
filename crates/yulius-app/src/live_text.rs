//! Text operations of the Editor's shared text (SPEC-411 EDT-016): retains, inserts and deletes
//! over UTF-16 units, applied and transformed as in ot.js (the Jupiter model): the server
//! transforms an operation made at an older revision past the steps since, and the browser does
//! the same with the steps it hears while its own operation is on its way. `transform(a, b)` puts
//! `a`'s insert first when both insert at one place; the server always passes the incoming
//! operation as `a`, the browser its own unconfirmed one, so both order such inserts alike.

use yulius_contracts::api::files::OpPart;

/// One part of an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    Retain(usize),
    Insert(Vec<u16>),
    Delete(usize),
}

/// An operation: parts covering the whole text it applies to, normalized (no empty parts, no two
/// of a kind in a row, an insert before an adjacent delete).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Op {
    parts: Vec<Part>,
    /// Length of the text it applies to.
    base: usize,
    /// Length of the text it makes.
    target: usize,
}

/// Why an operation cannot be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpError {
    /// Its length is not the text's.
    Misfit,
    /// It cuts between the halves of a character outside the BMP.
    Split,
}

const fn is_high(unit: u16) -> bool {
    matches!(unit, 0xd800..=0xdbff)
}

const fn is_low(unit: u16) -> bool {
    matches!(unit, 0xdc00..=0xdfff)
}

impl Op {
    pub(crate) fn retain(&mut self, n: usize) -> &mut Self {
        if n == 0 {
            return self;
        }
        self.base += n;
        self.target += n;
        if let Some(Part::Retain(last)) = self.parts.last_mut() {
            *last += n;
        } else {
            self.parts.push(Part::Retain(n));
        }
        self
    }

    pub(crate) fn insert(&mut self, text: &[u16]) -> &mut Self {
        if text.is_empty() {
            return self;
        }
        self.target += text.len();
        let len = self.parts.len();
        match self.parts.as_mut_slice() {
            [.., Part::Insert(last)] => last.extend_from_slice(text),
            // An insert goes before the delete it meets, so equal operations look alike.
            [.., Part::Insert(before), Part::Delete(_)] => before.extend_from_slice(text),
            [.., Part::Delete(_)] => self.parts.insert(len - 1, Part::Insert(text.to_vec())),
            _ => self.parts.push(Part::Insert(text.to_vec())),
        }
        self
    }

    pub(crate) fn delete(&mut self, n: usize) -> &mut Self {
        if n == 0 {
            return self;
        }
        self.base += n;
        if let Some(Part::Delete(last)) = self.parts.last_mut() {
            *last += n;
        } else {
            self.parts.push(Part::Delete(n));
        }
        self
    }

    /// Length of the text it applies to.
    pub(crate) const fn base_len(&self) -> usize {
        self.base
    }

    /// Length of the text it makes.
    pub(crate) const fn target_len(&self) -> usize {
        self.target
    }

    /// Whether it changes nothing.
    pub(crate) fn is_noop(&self) -> bool {
        self.parts.iter().all(|part| matches!(part, Part::Retain(_)))
    }

    /// Units it inserts, a bound on what a kept step costs.
    pub(crate) fn inserted(&self) -> usize {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Insert(text) => text.len(),
                _ => 0,
            })
            .sum()
    }

    /// Reads an operation from the wire; `None` for a count beyond any text.
    pub(crate) fn from_wire(parts: &[OpPart]) -> Option<Self> {
        let mut op = Self::default();
        for part in parts {
            match part {
                OpPart::Count(n) if *n >= 0 => {
                    op.retain(usize::try_from(*n).ok()?);
                }
                OpPart::Count(n) => {
                    op.delete(usize::try_from(n.unsigned_abs()).ok()?);
                }
                OpPart::Insert(text) => {
                    op.insert(&text.encode_utf16().collect::<Vec<_>>());
                }
            }
        }
        Some(op)
    }

    /// The operation on the wire.
    pub(crate) fn to_wire(&self) -> Vec<OpPart> {
        let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        self.parts
            .iter()
            .map(|part| match part {
                Part::Retain(n) => OpPart::Count(count(*n)),
                Part::Delete(n) => OpPart::Count(-count(*n)),
                Part::Insert(text) => OpPart::Insert(String::from_utf16_lossy(text)),
            })
            .collect()
    }

    /// The text after this operation.
    ///
    /// # Errors
    /// `Misfit` when the text is not as long as the operation's base, `Split` when a place where
    /// it inserts or deletes falls between the halves of a character.
    pub(crate) fn apply(&self, text: &[u16]) -> Result<Vec<u16>, OpError> {
        if text.len() != self.base {
            return Err(OpError::Misfit);
        }
        let split = |at: usize| {
            at > 0
                && text.get(at - 1).copied().is_some_and(is_high)
                && text.get(at).copied().is_some_and(is_low)
        };
        let mut out = Vec::with_capacity(self.target);
        let mut at = 0;
        for part in &self.parts {
            match part {
                Part::Retain(n) => {
                    out.extend_from_slice(&text[at..at + n]);
                    at += n;
                }
                Part::Insert(inserted) => {
                    if split(at) {
                        return Err(OpError::Split);
                    }
                    out.extend_from_slice(inserted);
                }
                Part::Delete(n) => {
                    if split(at) || split(at + n) {
                        return Err(OpError::Split);
                    }
                    at += n;
                }
            }
        }
        Ok(out)
    }

    /// `(a', b')` for `a` and `b` made on the same text: `a'` applies after `b`, `b'` after `a`, and
    /// both orders make the same text. At one place `a`'s insert comes first.
    ///
    /// # Errors
    /// `Misfit` when they were not made on texts of one length.
    pub(crate) fn transform(a: &Self, b: &Self) -> Result<(Self, Self), OpError> {
        if a.base != b.base {
            return Err(OpError::Misfit);
        }
        let mut a2 = Self::default();
        let mut b2 = Self::default();
        let mut xs = a.parts.iter().cloned();
        let mut ys = b.parts.iter().cloned();
        let mut x = xs.next();
        let mut y = ys.next();
        loop {
            match (x.take(), y.take()) {
                (None, None) => break,
                (Some(Part::Insert(text)), other) => {
                    a2.insert(&text);
                    b2.retain(text.len());
                    x = xs.next();
                    y = other;
                }
                (other, Some(Part::Insert(text))) => {
                    a2.retain(text.len());
                    b2.insert(&text);
                    x = other;
                    y = ys.next();
                }
                (Some(left), Some(right)) => {
                    let (n, rest_x, rest_y) = overlap(left.clone(), right.clone());
                    match (left, right) {
                        (Part::Retain(_), Part::Retain(_)) => {
                            a2.retain(n);
                            b2.retain(n);
                        }
                        (Part::Delete(_), Part::Retain(_)) => {
                            a2.delete(n);
                        }
                        (Part::Retain(_), Part::Delete(_)) => {
                            b2.delete(n);
                        }
                        // Both delete it: gone already for each.
                        _ => {}
                    }
                    x = rest_x.or_else(|| xs.next());
                    y = rest_y.or_else(|| ys.next());
                }
                _ => return Err(OpError::Misfit),
            }
        }
        Ok((a2, b2))
    }
}

/// The units two retains or deletes share, and what is left of each.
fn overlap(x: Part, y: Part) -> (usize, Option<Part>, Option<Part>) {
    let len = |part: &Part| match part {
        Part::Retain(n) | Part::Delete(n) => *n,
        Part::Insert(_) => 0,
    };
    let rest = |part: &Part, n: usize| match part {
        Part::Retain(_) => Part::Retain(n),
        _ => Part::Delete(n),
    };
    let (lx, ly) = (len(&x), len(&y));
    let n = lx.min(ly);
    (
        n,
        (lx > n).then(|| rest(&x, lx - n)),
        (ly > n).then(|| rest(&y, ly - n)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn text(units: &[u16]) -> String {
        String::from_utf16_lossy(units)
    }

    /// A small deterministic generator: no dependency, the same cases on every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
        }
    }

    const PIECES: [&str; 6] = ["a", "bc", "😀", "\n", "é", "xyz"];

    /// Places in `text` where an operation may cut: never inside a surrogate pair.
    fn cut(text: &[u16], at: usize) -> usize {
        if at > 0 && at < text.len() && is_high(text[at - 1]) && is_low(text[at]) {
            at - 1
        } else {
            at
        }
    }

    fn random_text(rng: &mut Rng) -> Vec<u16> {
        (0..rng.below(8))
            .flat_map(|_| units(PIECES[rng.below(PIECES.len())]))
            .collect()
    }

    fn random_op(rng: &mut Rng, text: &[u16]) -> Op {
        let mut op = Op::default();
        let mut at = 0;
        while at < text.len() {
            let n = cut(text, (at + 1 + rng.below(3)).min(text.len()));
            let n = if n <= at { (at + 2).min(text.len()) } else { n };
            match rng.below(3) {
                0 => op.retain(n - at),
                1 => op.delete(n - at),
                _ => op.insert(&units(PIECES[rng.below(PIECES.len())])).retain(n - at),
            };
            at = n;
        }
        if rng.below(2) == 0 {
            op.insert(&units(PIECES[rng.below(PIECES.len())]));
        }
        op
    }

    /// AC-411-1: concurrent operations converge whichever arrives first.
    #[test]
    fn concurrent_operations_converge_in_either_order() -> Result<(), OpError> {
        let mut rng = Rng(0x5eed_411);
        for _ in 0..2000 {
            let start = random_text(&mut rng);
            let a = random_op(&mut rng, &start);
            let b = random_op(&mut rng, &start);
            let (a2, b2) = Op::transform(&a, &b)?;
            let left = b2.apply(&a.apply(&start)?)?;
            let right = a2.apply(&b.apply(&start)?)?;
            assert_eq!(text(&left), text(&right), "{a:?} / {b:?} on {:?}", text(&start));
            assert_eq!(a2.base_len(), b.target_len());
            assert_eq!(b2.base_len(), a.target_len());
        }
        Ok(())
    }

    #[test]
    fn inserts_at_one_place_keep_the_first_operations_text_first() -> Result<(), OpError> {
        let start = units("ab");
        let mut a = Op::default();
        a.retain(1).insert(&units("X")).retain(1);
        let mut b = Op::default();
        b.retain(1).insert(&units("Y")).retain(1);
        let (a2, b2) = Op::transform(&a, &b)?;
        assert_eq!(text(&b2.apply(&a.apply(&start)?)?), "aXYb");
        assert_eq!(text(&a2.apply(&b.apply(&start)?)?), "aXYb");
        Ok(())
    }

    #[test]
    fn an_operation_of_another_length_or_cutting_a_character_is_refused() {
        let start = units("a😀b");
        let mut short = Op::default();
        short.retain(2);
        assert_eq!(short.apply(&start), Err(OpError::Misfit));
        let mut inside = Op::default();
        inside.retain(2).insert(&units("x")).retain(2);
        assert_eq!(inside.apply(&start), Err(OpError::Split));
        let mut half = Op::default();
        half.retain(1).delete(1).retain(2);
        assert_eq!(half.apply(&start), Err(OpError::Split));
        let mut other = Op::default();
        other.retain(3);
        assert_eq!(Op::transform(&short, &other), Err(OpError::Misfit));
    }

    #[test]
    fn the_wire_form_round_trips_and_normalizes() {
        let wire = vec![
            OpPart::Count(2),
            OpPart::Count(0),
            OpPart::Count(-1),
            OpPart::Insert("é😀".into()),
            OpPart::Count(1),
            OpPart::Count(1),
        ];
        let op = Op::from_wire(&wire).unwrap_or_default();
        assert_eq!(op.base_len(), 5);
        assert_eq!(op.target_len(), 7);
        assert_eq!(op.inserted(), 3);
        assert_eq!(
            op.to_wire(),
            vec![
                OpPart::Count(2),
                OpPart::Insert("é😀".into()),
                OpPart::Count(-1),
                OpPart::Count(2),
            ]
        );
        assert!(!op.is_noop());
        assert!(Op::from_wire(&[OpPart::Count(3)]).is_some_and(|op| op.is_noop()));
        assert!(Op::from_wire(&[OpPart::Count(i64::MIN)]).is_some());
    }
}
