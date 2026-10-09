//! JSON persistence of account usage snapshots (the domain type carries no serde).

use serde_json::{Value, json};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

fn window_json(w: &UsageWindow) -> Value {
    json!({"used_percent": w.used_percent, "window_minutes": w.window_minutes, "resets_at": w.resets_at.map(|t| t.0)})
}

fn window_from(v: &Value) -> Option<UsageWindow> {
    if !v.is_object() {
        return None;
    }
    Some(UsageWindow {
        used_percent: v.get("used_percent").and_then(Value::as_f64),
        window_minutes: v
            .get("window_minutes")
            .and_then(Value::as_u64)
            .and_then(|m| u32::try_from(m).ok()),
        resets_at: v.get("resets_at").and_then(Value::as_i64).map(Timestamp),
    })
}

/// Serializes a usage snapshot.
#[must_use]
pub fn to_json(usage: &AccountUsage) -> String {
    json!({
        "primary": usage.primary.as_ref().map(window_json),
        "secondary": usage.secondary.as_ref().map(window_json),
        "limit_reached": usage.limit_reached,
        "limit_resets_at": usage.limit_resets_at.map(|t| t.0),
        "observed_at": usage.observed_at.map(|t| t.0),
    })
    .to_string()
}

/// Parses a usage snapshot; malformed input yields "unknown" usage (never zero).
#[must_use]
pub fn from_json(text: &str) -> AccountUsage {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return AccountUsage::default();
    };
    AccountUsage {
        primary: v.get("primary").and_then(window_from),
        secondary: v.get("secondary").and_then(window_from),
        limit_reached: v
            .get("limit_reached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        limit_resets_at: v
            .get("limit_resets_at")
            .and_then(Value::as_i64)
            .map(Timestamp),
        observed_at: v.get("observed_at").and_then(Value::as_i64).map(Timestamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift generator for randomized tests (no extra dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        /// Text made of `pieces`, random bytes (lossy UTF-8) and multi-byte characters.
        fn text(&mut self, pieces: &[&str], max_parts: usize) -> String {
            let mut out = String::new();
            for _ in 0..self.below(max_parts + 1) {
                match self.below(10) {
                    0 => {
                        let bytes: Vec<u8> = (0..self.below(8))
                            .map(|_| (self.next() & 0xff) as u8)
                            .collect();
                        out.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    1 => out
                        .push_str(["é", "😀", "\u{0}", "\r", "\t", "ﬀ", "\u{200b}"][self.below(7)]),
                    _ => out.push_str(pieces[self.below(pieces.len())]),
                }
            }
            out
        }
    }

    /// Randomized usage JSON (#76): parsing never panics.
    #[test]
    fn random_usage_json_never_panics() {
        let pieces = [
            "{",
            "}",
            ",",
            ":",
            "\"primary\"",
            "\"secondary\"",
            "\"used_percent\"",
            "1e309",
            "-5",
            "\"window_minutes\"",
            "4294967296",
            "\"resets_at\"",
            "null",
            "\"limit_reached\"",
            "true",
        ];
        let mut rng = Rng(0x5eed_1234_abcd_0001);
        for _ in 0..4_000 {
            let _ = from_json(&rng.text(&pieces, 20));
        }
    }

    #[test]
    fn round_trip_and_garbage() {
        let usage = AccountUsage {
            primary: Some(UsageWindow {
                used_percent: Some(42.5),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(9)),
            }),
            secondary: None,
            limit_reached: true,
            limit_resets_at: Some(Timestamp(10)),
            observed_at: Some(Timestamp(11)),
        };
        assert_eq!(from_json(&to_json(&usage)), usage);
        assert_eq!(from_json("not json"), AccountUsage::default());
    }
}
