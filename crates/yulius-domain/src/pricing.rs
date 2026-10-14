//! Claude API list prices and what a run's tokens would cost at them (SPEC-110 TUS-001,
//! TUS-002, ADR-0089).
//!
//! Subscriptions are not billed per token; this answers "what would the pool's work have cost on
//! the API". Prices are USD per million tokens from
//! <https://platform.claude.com/docs/en/about-claude/pricing>, read 2026-10-14. Cache writes are
//! priced at the 5-minute rate (the API default; runs store one cache-write total), and Haiku 5.5
//! at its rates for prompts up to 100 000 tokens. An unknown model has no price, never zero.

/// One model's list price in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Base input tokens.
    pub input: f64,
    /// 5-minute cache writes.
    pub cache_write: f64,
    /// Cache hits and refreshes.
    pub cache_read: f64,
    /// Output tokens.
    pub output: f64,
}

/// The price of `model`, or `None` for a model the table does not know.
#[must_use]
pub fn price_of(model: &str) -> Option<Price> {
    let id = model.trim().to_ascii_lowercase();
    let id = id.split('[').next().unwrap_or_default();
    let id = match id.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => id,
    };
    PRICES
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, price)| *price)
}

const fn usd(input: f64, cache_write: f64, cache_read: f64, output: f64) -> Price {
    Price {
        input,
        cache_write,
        cache_read,
        output,
    }
}

/// Model id → (input, 5-minute cache write, cache read, output), USD per million tokens.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", usd(10.0, 12.5, 0.25, 50.0)),
    ("claude-mythos-5-1", usd(10.0, 12.5, 0.25, 50.0)),
    ("claude-fable-5", usd(10.0, 12.5, 1.0, 50.0)),
    ("claude-mythos-5", usd(10.0, 12.5, 1.0, 50.0)),
    ("claude-opus-5-5", usd(4.0, 5.0, 0.2, 20.0)),
    ("claude-opus-5", usd(5.0, 6.25, 0.5, 25.0)),
    ("claude-opus-4-8", usd(5.0, 6.25, 0.5, 25.0)),
    ("claude-opus-4-7", usd(5.0, 6.25, 0.5, 25.0)),
    ("claude-opus-4-6", usd(5.0, 6.25, 0.5, 25.0)),
    ("claude-opus-4-5", usd(5.0, 6.25, 0.5, 25.0)),
    ("claude-opus-4-1", usd(15.0, 18.75, 1.5, 75.0)),
    ("claude-opus-4", usd(15.0, 18.75, 1.5, 75.0)),
    ("claude-sonnet-5-5", usd(2.0, 2.5, 0.1, 10.0)),
    ("claude-sonnet-5", usd(2.0, 2.5, 0.2, 10.0)),
    ("claude-sonnet-4-6", usd(3.0, 3.75, 0.3, 15.0)),
    ("claude-sonnet-4-5", usd(3.0, 3.75, 0.3, 15.0)),
    ("claude-sonnet-4", usd(3.0, 3.75, 0.3, 15.0)),
    ("claude-haiku-5-5", usd(0.1, 0.125, 0.01, 0.5)),
    ("claude-haiku-4-5", usd(1.0, 1.25, 0.1, 5.0)),
    ("claude-3-5-haiku", usd(0.8, 1.0, 0.08, 4.0)),
];

/// What `input`, `output`, `cache_read` and `cache_write` tokens cost at `price`, in USD; unknown
/// figures add nothing.
#[must_use]
pub fn cost_usd(
    price: Price,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
) -> f64 {
    let part = |tokens: Option<u64>, per_million: f64| {
        tokens.map_or(0.0, |t| t as f64 * per_million / 1_000_000.0)
    };
    part(input, price.input)
        + part(output, price.output)
        + part(cache_read, price.cache_read)
        + part(cache_write, price.cache_write)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn current_models_have_their_list_price() {
        let opus = price_of("claude-opus-5-5");
        assert_eq!(
            opus,
            Some(Price {
                input: 4.0,
                cache_write: 5.0,
                cache_read: 0.2,
                output: 20.0
            })
        );
        let fable = price_of("claude-fable-5-1").map(|p| (p.input, p.cache_read, p.output));
        assert_eq!(fable, Some((10.0, 0.25, 50.0)));
        let sonnet = price_of("claude-sonnet-5-5").map(|p| (p.input, p.cache_read, p.output));
        assert_eq!(sonnet, Some((2.0, 0.1, 10.0)));
        let haiku = price_of("claude-haiku-4-5").map(|p| (p.input, p.cache_write, p.output));
        assert_eq!(haiku, Some((1.0, 1.25, 5.0)));
    }

    #[test]
    fn a_shorter_id_never_takes_a_longer_ones_price() {
        // claude-opus-5 is not claude-opus-5-5, nor claude-opus-4 claude-opus-4-8.
        assert_eq!(price_of("claude-opus-5").map(|p| p.input), Some(5.0));
        assert_eq!(price_of("claude-opus-4").map(|p| p.input), Some(15.0));
        assert_eq!(price_of("claude-opus-4-8").map(|p| p.input), Some(5.0));
        assert_eq!(price_of("claude-sonnet-5").map(|p| p.cache_read), Some(0.2));
    }

    #[test]
    fn date_suffixes_and_variants_are_the_same_model() {
        assert_eq!(
            price_of("claude-haiku-4-5-20251001"),
            price_of("claude-haiku-4-5")
        );
        assert_eq!(
            price_of("claude-opus-4-20250514"),
            price_of("claude-opus-4")
        );
        assert_eq!(price_of("claude-opus-4-8[1m]"), price_of("claude-opus-4-8"));
        assert_eq!(price_of("Claude-Sonnet-4-6"), price_of("claude-sonnet-4-6"));
        assert_eq!(
            price_of("claude-3-5-haiku-20241022").map(|p| p.input),
            Some(0.8)
        );
    }

    #[test]
    fn unknown_models_have_no_price() {
        for model in [
            "",
            "unknown",
            "gpt-5",
            "claude-opus-9",
            "claude-opus-5-5-preview",
        ] {
            assert_eq!(price_of(model), None, "{model}");
        }
    }

    #[test]
    fn the_cost_adds_every_known_figure_at_its_rate() {
        let Some(price) = price_of("claude-opus-5-5") else {
            panic!("no price");
        };
        let m = Some(1_000_000);
        // $4 input + $20 output + $0.20 cache read + $5 cache write.
        assert!(close(cost_usd(price, m, m, m, m), 29.2));
        assert!(close(cost_usd(price, Some(500_000), None, None, None), 2.0));
        assert!(close(cost_usd(price, None, None, None, None), 0.0));
    }
}
