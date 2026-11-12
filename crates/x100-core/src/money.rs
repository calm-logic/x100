//! Money, tokens and prices, and a bounded percentage.

use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// An amount in millionths of a US dollar. Integers, so sums are exact and the type can be hashed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Money(u64);

impl Money {
    pub const ZERO: Money = Money(0);

    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    pub const fn from_dollars(dollars: u64) -> Self {
        Self(dollars.saturating_mul(1_000_000))
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

impl Add for Money {
    type Output = Money;

    fn add(self, other: Money) -> Money {
        Money(self.0.saturating_add(other.0))
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, other: Money) {
        *self = *self + other;
    }
}

/// Tokens a turn or a one-off call used, as its harness or provider reports them, in the classes
/// providers charge for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input read fresh, neither from nor into the cache.
    pub input: u64,
    pub output: u64,
    /// Input read from the cache.
    pub cache_read: u64,
    /// Input written to the cache with the short lifetime.
    pub cache_write_short: u64,
    /// Input written to the cache with the long lifetime.
    pub cache_write_long: u64,
}

impl Usage {
    /// Every input token of the turn, cached or not. A long-context tier is decided on this.
    pub fn all_input(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write_short)
            .saturating_add(self.cache_write_long)
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write_short = self
            .cache_write_short
            .saturating_add(other.cache_write_short);
        self.cache_write_long = self.cache_write_long.saturating_add(other.cache_write_long);
    }
}

/// What a provider charges per million tokens of each class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rates {
    pub input: Money,
    pub output: Money,
    pub cache_read: Money,
    pub cache_write_short: Money,
    pub cache_write_long: Money,
}

impl Rates {
    /// In millionths of a micro-dollar, so the sum is rounded once.
    fn cost(&self, usage: &Usage) -> u128 {
        let part = |tokens: u64, per_million: Money| u128::from(tokens) * u128::from(per_million.0);
        part(usage.input, self.input)
            + part(usage.output, self.output)
            + part(usage.cache_read, self.cache_read)
            + part(usage.cache_write_short, self.cache_write_short)
            + part(usage.cache_write_long, self.cache_write_long)
    }
}

/// A second set of rates for a turn whose input passes a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LongContext {
    /// A turn with more input tokens than this, cached ones included, is charged at `rates`.
    pub above: u64,
    pub rates: Rates,
}

/// A model's listed price at one effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Price {
    pub rates: Rates,
    pub long_context: Option<LongContext>,
}

impl Price {
    /// The rates a turn with `usage` is charged at. Past the long-context threshold the second
    /// rates apply to the whole turn, not only to the tokens above it.
    pub fn rates_for(&self, usage: &Usage) -> &Rates {
        match &self.long_context {
            Some(long) if usage.all_input() > long.above => &long.rates,
            _ => &self.rates,
        }
    }

    /// What `usage` costs at this price, for a harness that reports tokens but no cost. Rounded up,
    /// so a budget is never undercounted.
    pub fn cost(&self, usage: &Usage) -> Money {
        let total = self.rates_for(usage).cost(usage);
        Money(u64::try_from(total.div_ceil(1_000_000)).unwrap_or(u64::MAX))
    }
}

/// A whole percentage from 0 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Percent(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a percentage from 0 to 100")]
pub struct NotAPercent(pub u8);

impl Percent {
    pub fn new(value: u8) -> Result<Self, NotAPercent> {
        if value <= 100 {
            Ok(Self(value))
        } else {
            Err(NotAPercent(value))
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }

    /// This share of `amount`, rounded down.
    pub fn of(self, amount: Money) -> Money {
        Money(u64::try_from(u128::from(amount.0) * u128::from(self.0) / 100).unwrap_or(u64::MAX))
    }
}

impl TryFrom<u8> for Percent {
    type Error = NotAPercent;

    fn try_from(v: u8) -> Result<Self, NotAPercent> {
        Percent::new(v)
    }
}

impl From<Percent> for u8 {
    fn from(p: Percent) -> u8 {
        p.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates(input: u64, output: u64) -> Rates {
        Rates {
            input: Money::from_dollars(input),
            output: Money::from_dollars(output),
            cache_read: Money::from_micros(300_000),
            cache_write_short: Money::from_micros(3_750_000),
            cache_write_long: Money::from_dollars(6),
        }
    }

    #[test]
    fn cost_is_tokens_at_the_listed_price_rounded_up() {
        let price = Price {
            rates: rates(3, 15),
            long_context: None,
        };
        let usage = Usage {
            input: 1_000,
            output: 2_000,
            cache_read: 10_000,
            cache_write_short: 1_000,
            cache_write_long: 1_000,
        };
        // 1000 * 3 + 2000 * 15 + 10000 * 0.3 + 1000 * 3.75 + 1000 * 6 dollars per million.
        assert_eq!(price.cost(&usage), Money::from_micros(45_750));
        let one_token = Usage {
            input: 1,
            ..Usage::default()
        };
        assert_eq!(price.cost(&one_token), Money::from_micros(3));
        let mut tiny = price;
        tiny.rates.input = Money::from_micros(1);
        assert_eq!(tiny.cost(&one_token), Money::from_micros(1));
    }

    #[test]
    fn past_the_long_context_threshold_the_whole_turn_takes_the_second_rates() {
        let price = Price {
            rates: rates(3, 15),
            long_context: Some(LongContext {
                above: 200_000,
                rates: rates(6, 22),
            }),
        };
        let at_threshold = Usage {
            input: 100_000,
            cache_read: 100_000,
            output: 1_000,
            ..Usage::default()
        };
        // 100000 * 3 + 100000 * 0.3 + 1000 * 15 dollars per million.
        assert_eq!(price.cost(&at_threshold), Money::from_micros(345_000));
        let past = Usage {
            input: 100_001,
            ..at_threshold
        };
        // 100001 * 6 + 100000 * 0.3 + 1000 * 22, rounded up.
        assert_eq!(price.cost(&past), Money::from_micros(652_006));
    }

    #[test]
    fn a_percentage_is_bounded() {
        assert!(Percent::new(101).is_err());
        assert!(serde_json::from_str::<Percent>("150").is_err());
        assert_eq!(
            Percent::new(10).unwrap().of(Money::from_dollars(50)),
            Money::from_dollars(5)
        );
    }
}
