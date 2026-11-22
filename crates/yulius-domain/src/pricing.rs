//! Standard API list prices and the optional Codex Ultrafast rate (SPEC-110, SPEC-602).
//!
//! Subscriptions are not billed per token; this answers "what would the pool's work have cost on
//! the API". Prices are USD per million tokens from
//! <https://platform.claude.com/docs/en/about-claude/pricing>, read 2026-10-14. Cache writes are
//! priced at the 5-minute rate (the API default; runs store one cache-write total), and Haiku 5.5
//! at its rates for prompts up to 100 000 tokens. GPT-6 prices use the short-context and
//! 30-minute cache-write rates at <https://developers.openai.com/api/docs/pricing>, read
//! 2026-11-21. An unknown model has no price.

use crate::time::Timestamp;
use crate::usage::{ModelTokens, TokenUsage};

/// One model's list price in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Base input tokens.
    pub input: f64,
    /// Cache writes at the provider's default retention.
    pub cache_write: f64,
    /// Cache hits and refreshes.
    pub cache_read: f64,
    /// Output tokens.
    pub output: f64,
}

/// The price of `model`, or `None` for a model the table does not know. First-party ids only:
/// cloud-platform ids (Bedrock `anthropic.…-v1:0`, Vertex `…@YYYYMMDD`) count as unpriced.
#[must_use]
pub fn price_of(model: &str) -> Option<Price> {
    model_prices(model).map(|(standard, _)| standard)
}

#[derive(Clone, Copy)]
pub(crate) enum Ultrafast {
    Gpt62,
    Regular,
}

impl Ultrafast {
    /// OpenAI's GPT-6.2 offer ends at midnight Pacific on March 1, 2027 (08:00 UTC).
    pub(crate) fn multiplier(self, now: Timestamp) -> f64 {
        match self {
            Self::Gpt62 if now < Timestamp(1_803_888_000_000) => 3.0,
            Self::Gpt62 | Self::Regular => 6.0,
        }
    }
}

pub(crate) fn model_prices(model: &str) -> Option<(Price, Option<Ultrafast>)> {
    let id = model.trim().to_ascii_lowercase();
    if let Some((_, price, tier)) = CODEX_PRICES.iter().find(|(name, _, _)| *name == id) {
        return Some((*price, *tier));
    }
    let id = id.split('[').next().unwrap_or_default();
    let id = match id.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => id,
    };
    PRICES
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, price)| (*price, None))
}

const fn usd(input: f64, cache_write: f64, cache_read: f64, output: f64) -> Price {
    Price {
        input,
        cache_write,
        cache_read,
        output,
    }
}

const CODEX_PRICES: &[(&str, Price, Option<Ultrafast>)] = &[
    (
        "gpt-6.2-astra",
        usd(8.0, 10.0, 0.4, 40.0),
        Some(Ultrafast::Gpt62),
    ),
    (
        "gpt-6.2-sol",
        usd(2.0, 2.5, 0.1, 10.0),
        Some(Ultrafast::Gpt62),
    ),
    (
        "gpt-6-astra",
        usd(10.0, 12.5, 1.0, 50.0),
        Some(Ultrafast::Regular),
    ),
    (
        "gpt-6.1-sol",
        usd(2.0, 2.5, 0.1, 10.0),
        Some(Ultrafast::Regular),
    ),
    ("gpt-6.1-terra", usd(1.0, 1.25, 0.05, 5.0), None),
    ("gpt-6-sol", usd(2.0, 2.5, 0.2, 10.0), None),
    ("gpt-6-luna", usd(0.1, 0.125, 0.01, 0.5), None),
];

/// Model id → (input, 5-minute cache write, cache read, output), USD per million tokens.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-2", usd(8.0, 10.0, 0.2, 40.0)),
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

/// What a run would cost on the API: at Standard prices, and the extra if its eligible tokens had
/// used Ultrafast at `now`'s rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunCost {
    /// At Standard prices.
    pub standard: f64,
    /// The extra for Ultrafast; zero when no part of it is eligible.
    pub ultrafast_premium: f64,
}

/// What a run whose own thread ran `model` would cost, its `totals` including `parts` that
/// other models answered (SPEC-619 CSA-4): each part at its model's price, the rest at `model`'s.
/// `None` when a part with tokens, or a rest with tokens, has no price.
#[must_use]
pub fn run_cost(
    model: Option<&str>,
    totals: TokenUsage,
    parts: &[ModelTokens],
    now: Timestamp,
) -> Option<RunCost> {
    fn less(total: Option<u64>, part: Option<u64>) -> Option<u64> {
        total.map(|t| t.saturating_sub(part.unwrap_or(0)))
    }
    let rest = parts.iter().fold(totals, |rest, part| TokenUsage {
        input: less(rest.input, part.usage.input),
        output: less(rest.output, part.usage.output),
        cache_read: less(rest.cache_read, part.usage.cache_read),
        cache_write: less(rest.cache_write, part.usage.cache_write),
    });
    let mut cost = RunCost {
        standard: 0.0,
        ultrafast_premium: 0.0,
    };
    for (model, usage) in std::iter::once((model, rest)).chain(
        parts
            .iter()
            .map(|part| (Some(part.model.as_str()), part.usage)),
    ) {
        let spent = [
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
        ]
        .iter()
        .any(|tokens| tokens.is_some_and(|t| t > 0));
        if !spent {
            continue;
        }
        let (price, tier) = model.and_then(model_prices)?;
        let standard = cost_usd(
            price,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
        );
        cost.standard += standard;
        if let Some(tier) = tier {
            cost.ultrafast_premium += standard * (tier.multiplier(now) - 1.0);
        }
    }
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn million(model: &str, each: u64) -> ModelTokens {
        ModelTokens {
            model: model.into(),
            usage: TokenUsage {
                input: Some(each * 1_000_000),
                output: Some(each * 1_000_000),
                cache_read: Some(each * 1_000_000),
                cache_write: Some(each * 1_000_000),
            },
        }
    }

    /// SPEC-619 AC-4: a Codex subagent's tokens are priced at its own model, the rest of the
    /// run at the run's; an unpriced part leaves the run unpriced.
    #[test]
    fn a_runs_parts_are_priced_at_their_own_models() {
        let now = Timestamp(1_795_000_000_000);
        let totals = million("gpt-6.2-sol", 2).usage;
        let astra = [million("gpt-6.2-astra", 1)];
        let cost = run_cost(Some("gpt-6.2-sol"), totals, &astra, now);
        // 14.60 for the rest at Sol, 58.40 for the subagent at Astra; tripled for Ultrafast.
        assert!(
            cost.is_some_and(|c| close(c.standard, 73.0) && close(c.ultrafast_premium, 146.0)),
            "{cost:?}"
        );
        let alone = run_cost(Some("gpt-6.2-sol"), totals, &[], now);
        assert!(alone.is_some_and(|c| close(c.standard, 29.2)), "{alone:?}");
        assert_eq!(
            run_cost(Some("gpt-6.2-sol"), totals, &[million("mystery", 1)], now),
            None
        );
        let luna = run_cost(Some("gpt-6-luna"), million("x", 1).usage, &[], now);
        assert!(luna.is_some_and(|c| c.ultrafast_premium == 0.0), "{luna:?}");
        // Only subagents spent: the run's own unknown model costs nothing.
        let only = run_cost(None, million("x", 1).usage, &astra, now);
        assert!(only.is_some_and(|c| close(c.standard, 58.4)), "{only:?}");
    }

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
        let fable =
            price_of("claude-fable-5-2").map(|p| (p.input, p.cache_write, p.cache_read, p.output));
        assert_eq!(fable, Some((8.0, 10.0, 0.2, 40.0)));
        let fable = price_of("claude-fable-5-1").map(|p| (p.input, p.cache_read, p.output));
        assert_eq!(fable, Some((10.0, 0.25, 50.0)));
        let sonnet = price_of("claude-sonnet-5-5").map(|p| (p.input, p.cache_read, p.output));
        assert_eq!(sonnet, Some((2.0, 0.1, 10.0)));
        let haiku = price_of("claude-haiku-4-5").map(|p| (p.input, p.cache_write, p.output));
        assert_eq!(haiku, Some((1.0, 1.25, 5.0)));
    }

    #[test]
    fn gpt_six_models_use_their_standard_short_context_token_rates() {
        for (model, expected) in [
            ("gpt-6.2-astra", usd(8.0, 10.0, 0.4, 40.0)),
            ("gpt-6.2-sol", usd(2.0, 2.5, 0.1, 10.0)),
            ("gpt-6-astra", usd(10.0, 12.5, 1.0, 50.0)),
            ("gpt-6.1-sol", usd(2.0, 2.5, 0.1, 10.0)),
            ("gpt-6.1-terra", usd(1.0, 1.25, 0.05, 5.0)),
            ("gpt-6-sol", usd(2.0, 2.5, 0.2, 10.0)),
            ("gpt-6-luna", usd(0.1, 0.125, 0.01, 0.5)),
        ] {
            assert_eq!(price_of(model), Some(expected), "{model}");
        }
        assert_eq!(price_of("gpt-6.2-astra-preview"), None);
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
