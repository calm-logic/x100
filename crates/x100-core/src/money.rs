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

/// Tokens a turn or a one-off call used, as its harness reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// A model's listed price at one effort level, per million tokens of each class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Price {
    pub input: Money,
    pub output: Money,
    pub cache_read: Money,
    pub cache_write: Money,
}

impl Price {
    /// What `usage` costs at this price, for a harness that reports tokens but no cost. Rounded up,
    /// so a budget is never undercounted.
    pub fn cost(&self, usage: &Usage) -> Money {
        let part = |tokens: u64, per_million: Money| -> u128 {
            u128::from(tokens) * u128::from(per_million.0)
        };
        let total = part(usage.input, self.input)
            + part(usage.output, self.output)
            + part(usage.cache_read, self.cache_read)
            + part(usage.cache_write, self.cache_write);
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

    #[test]
    fn cost_is_tokens_at_the_listed_price_rounded_up() {
        let price = Price {
            input: Money::from_dollars(3),
            output: Money::from_dollars(15),
            cache_read: Money::from_micros(300_000),
            cache_write: Money::from_micros(3_750_000),
        };
        let usage = Usage {
            input: 1_000,
            output: 2_000,
            cache_read: 10_000,
            cache_write: 0,
        };
        // 1000 * 3 + 2000 * 15 + 10000 * 0.3 dollars per million = 0.036 dollars.
        assert_eq!(price.cost(&usage), Money::from_micros(36_000));
        let one_token = Usage {
            input: 1,
            ..Usage::default()
        };
        assert_eq!(price.cost(&one_token), Money::from_micros(3));
        let tiny = Price {
            input: Money::from_micros(1),
            ..price
        };
        assert_eq!(tiny.cost(&one_token), Money::from_micros(1));
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
