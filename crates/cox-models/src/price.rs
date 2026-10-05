// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! The price table (plan.md §1.7/D5/D6g; moved here from
//! `cox-provider/src/usage.rs` by T30.24 so the model catalog and its
//! prices live in one pure crate). Loads a dated per-model rate table and
//! computes the cost of a `Usage`; `cox_provider::usage::Priced` is where
//! this meets a live call.
//!
//! The price table carries `verified_on` dates; `cox doctor` may warn if a
//! row is older than 90 days. Unknown models are costed as 0 with
//! `estimated = true` and emit a `Notice(Warn)` once per session.

use std::collections::HashSet;
use std::sync::Mutex;

use figment::Figment;
use figment::providers::{Format, Toml};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use cox_protocol::types::{ModelId, Usage};

const DEFAULT_PRICES: &str = include_str!("../../cox-provider/prices.toml");

/// One `[[model]]` row of `prices.toml`: the four per-MTok rates a call is
/// billed at, plus the provenance that lets `cox doctor` flag a stale
/// table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    /// The `ModelId` string this row prices.
    pub id: String,
    /// Input tokens, USD per MTok.
    pub input: f64,
    /// Output tokens, USD per MTok.
    pub output: f64,
    /// Cache write tokens, USD per MTok.
    pub cache_write: f64,
    /// Cache read tokens, USD per MTok.
    pub cache_read: f64,
    /// ISO date these rates were last checked against `source_url`.
    pub verified_on: String,
    /// The official pricing page the rates were read from.
    pub source_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PriceToml {
    model: Vec<Price>,
}

/// Why a price table could not be loaded.
#[derive(Debug, Error)]
pub enum PriceError {
    /// The TOML did not parse, or did not match the `[[model]]` shape.
    #[error("failed to parse price table: {0}")]
    Parse(String),
    /// Kept for a caller that reads a file itself (this crate does no I/O
    /// of its own; `cox_provider::usage::load_price_table` is that
    /// caller) and wants that error folded into the same type.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Thread-safe price table for cost computation, parsed from a
/// `prices.toml`-shaped string, or from the embedded defaults.
pub struct PriceTable {
    prices: Vec<Price>,
    /// Models that have already been warned about (missing/unknown).
    warned_models: Mutex<HashSet<String>>,
}

impl PriceTable {
    /// The table compiled into the binary (`cox-provider/prices.toml`,
    /// written only by `cox-vendor models`, AGENTS.md A48).
    pub fn embedded() -> Result<Self, PriceError> {
        Self::parse(DEFAULT_PRICES)
    }

    /// Parses a `prices.toml`-shaped string. Public so a caller that
    /// already has the contents — from a file it read itself, or from a
    /// `Catalog`'s user-price layer — can hand them straight in.
    pub fn parse(content: &str) -> Result<Self, PriceError> {
        let table: PriceToml = Figment::from(Toml::string(content))
            .extract()
            .map_err(|e| PriceError::Parse(e.to_string()))?;
        Ok(PriceTable {
            prices: table.model,
            warned_models: Mutex::new(HashSet::new()),
        })
    }

    /// All `[[model]]` rows, for staleness checks and diagnostics.
    pub fn prices(&self) -> &[Price] {
        &self.prices
    }

    /// Get the price for a model id, or None if not found.
    pub fn price_for(&self, model: &ModelId) -> Option<&Price> {
        self.prices.iter().find(|p| p.id == model.0)
    }

    /// Compute the cost of a `Usage` row using this price table.
    /// If the model is unknown, returns 0 with `estimated = true` (the usage
    /// will be marked as estimated so the caller can emit a warning).
    pub fn cost(&self, usage: &Usage, price: &Price) -> f64 {
        let input_cost = (usage.input_tokens as f64) * price.input / 1_000_000.0;
        let output_cost = (usage.output_tokens as f64) * price.output / 1_000_000.0;
        let cache_write_cost = (usage.cache_write_tokens as f64) * price.cache_write / 1_000_000.0;
        let cache_read_cost = (usage.cache_read_tokens as f64) * price.cache_read / 1_000_000.0;
        input_cost + output_cost + cache_write_cost + cache_read_cost
    }

    /// Sets `usage.cost_usd` for `model`. An unpriced model is not an error:
    /// it costs 0 and is flagged `estimated`, so it can never make a call
    /// disappear from the ledger.
    pub fn apply(&self, model: &ModelId, usage: &mut Usage) {
        match self.price_for(model) {
            Some(price) => usage.cost_usd = self.cost(usage, price),
            None => {
                usage.cost_usd = 0.0;
                usage.estimated = true;
            }
        }
    }

    /// Returns true the first time this model is seen, false on subsequent calls.
    /// Used to emit one `Notice(Warn)` per session for unknown models.
    pub fn warn_once(&self, model: &ModelId) -> bool {
        // A poisoned lock only means some other thread panicked mid-warn; the
        // warned-set is advisory, so recovering beats propagating.
        let mut warned = self
            .warned_models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        warned.insert(model.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_prices_toml_parses_and_has_all_tier_models() {
        let table = PriceTable::parse(DEFAULT_PRICES).expect("default prices parse");
        assert_eq!(table.prices.len(), 24, "T39.5 added the three gemini rows");
        // `price_for` is a linear find, so row order carries no meaning and
        // is not asserted on.
        // Verify all required tier models are present.
        let ids: Vec<&str> = table.prices.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"claude-haiku-4-5"));
        assert!(ids.contains(&"claude-sonnet-5"));
        assert!(ids.contains(&"claude-opus-5"));
        assert!(ids.contains(&"claude-fable-5-1"));
        assert!(ids.contains(&"jev-latest"));
    }

    #[test]
    fn usage_prices_cover_every_configured_model() {
        // The ledger must price every model a user can route to without
        // touching a price file. A missing row is not fatal at runtime
        // (costed 0, `estimated`), so this test is the sync check between
        // default.toml and prices.toml; `Config::configured_model_ids`
        // (cox-protocol) is the one enumeration of "every configured
        // model", shared with `cox doctor`'s catalog/price row (T30.27) so
        // the two checks can never disagree about what "configured" means.
        use cox_protocol::Config;
        let table = PriceTable::parse(DEFAULT_PRICES).expect("default prices parse");
        let cfg: Config = Figment::from(Toml::string(cox_protocol::config::DEFAULT_CONFIG_TOML))
            .extract()
            .expect("default.toml parses");
        for id in cfg.configured_model_ids() {
            assert!(
                table.price_for(&ModelId(id.clone())).is_some(),
                "prices.toml has no row for configured model `{id}`"
            );
        }
    }

    #[test]
    fn usage_cost_matches_hand_computed() {
        let table = PriceTable::parse(DEFAULT_PRICES).expect("default prices parse");
        let price = table
            .price_for(&ModelId("claude-haiku-4-5".into()))
            .expect("haiku price");
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 50_000,
            cache_write_tokens: 30_000,
            estimated: false,
            cost_usd: 0.0, // placeholder; will be set by cost()
            latency_ms: 100,
        };
        let cost = table.cost(&usage, price);
        // Haiku: input $1/M, output $5/M, cache_write $1.25/M, cache_read $0.10/M
        // = (1M * $1) + (100k * $5/M) + (30k * $1.25/M) + (50k * $0.10/M)
        // = $1 + $0.50 + $0.0375 + $0.005
        // = $1.5425
        assert!((cost - 1.5425).abs() < 0.0001);
    }

    #[test]
    fn usage_unknown_model_is_estimated_and_warns_once() {
        let table = PriceTable::parse(DEFAULT_PRICES).expect("default prices parse");
        let unknown = ModelId("claude-unknown-42".into());
        assert!(table.price_for(&unknown).is_none());
        // First call to warn_once returns true.
        assert!(table.warn_once(&unknown));
        // Second call returns false (already warned).
        assert!(!table.warn_once(&unknown));
        // A different model triggers warning anew.
        let another = ModelId("claude-other-99".into());
        assert!(table.warn_once(&another));
    }
}
