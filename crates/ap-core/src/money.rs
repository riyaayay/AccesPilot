//! Pure Money and Currency types. No floats, minor units only, no `as` casts.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum MoneyError {
    #[error("Currency mismatch: {0} vs {1}")]
    CurrencyMismatch(Currency, Currency),
    #[error("Arithmetic overflow in money operation")]
    Overflow,
    #[error("Negative amount not allowed in this context: {0}")]
    NegativeAmount(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    INR,
    USD,
    EUR,
    GBP,
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::INR => write!(f, "INR"),
            Self::USD => write!(f, "USD"),
            Self::EUR => write!(f, "EUR"),
            Self::GBP => write!(f, "GBP"),
        }
    }
}

impl std::str::FromStr for Currency {
    type Err = MoneyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "INR" => Ok(Self::INR),
            "USD" => Ok(Self::USD),
            "EUR" => Ok(Self::EUR),
            "GBP" => Ok(Self::GBP),
            _ => Err(MoneyError::CurrencyMismatch(Self::INR, Self::USD)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub amount_minor: i64,
    pub currency: Currency,
}

impl Money {
    pub const fn new_raw(amount_minor: i64, currency: Currency) -> Self {
        Self {
            amount_minor,
            currency,
        }
    }

    pub fn new(amount_minor: i64, currency: Currency) -> Result<Self, MoneyError> {
        if amount_minor < 0 {
            return Err(MoneyError::NegativeAmount(amount_minor));
        }
        Ok(Self {
            amount_minor,
            currency,
        })
    }

    pub const fn zero(currency: Currency) -> Self {
        Self {
            amount_minor: 0,
            currency,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount_minor == 0
    }

    pub fn is_positive(&self) -> bool {
        self.amount_minor > 0
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        let sum = self
            .amount_minor
            .checked_add(other.amount_minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            amount_minor: sum,
            currency: self.currency,
        })
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        let diff = self
            .amount_minor
            .checked_sub(other.amount_minor)
            .ok_or(MoneyError::Overflow)?;
        if diff < 0 {
            return Err(MoneyError::NegativeAmount(diff));
        }
        Ok(Self {
            amount_minor: diff,
            currency: self.currency,
        })
    }

    pub fn checked_cmp(&self, other: &Self) -> Result<std::cmp::Ordering, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        Ok(self.amount_minor.cmp(&other.amount_minor))
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_neg = self.amount_minor < 0;
        let abs_minor = if is_neg {
            self.amount_minor.checked_abs().unwrap_or(i64::MAX)
        } else {
            self.amount_minor
        };
        let major = abs_minor / 100;
        let minor = abs_minor % 100;
        let prefix = if is_neg { "-" } else { "" };
        match self.currency {
            Currency::INR => write!(f, "{prefix}₹{major}.{minor:02}"),
            Currency::USD => write!(f, "{prefix}${major}.{minor:02}"),
            Currency::EUR => write!(f, "{prefix}€{major}.{minor:02}"),
            Currency::GBP => write!(f, "{prefix}£{major}.{minor:02}"),
        }
    }
}
