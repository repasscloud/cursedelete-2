//! Mandatory-unit duration parsing for `--age`.
//!
//! `--age` intentionally refuses a bare number: `--age 2` does not say
//! whether the operator meant two minutes or two weeks, and getting this
//! wrong is destructive. A unit is always required.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgeDuration(Duration);

impl AgeDuration {
    pub fn as_duration(self) -> Duration {
        self.0
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgeDurationParseError {
    #[error(
        "'--age {0}' is missing a unit. A duration unit is mandatory: use m (minutes), \
         h (hours), d (days), or w (weeks), e.g. --age 2d"
    )]
    MissingUnit(String),

    #[error(
        "'--age {value}' uses unknown unit '{unit}'. Supported units are m (minutes), \
         h (hours), d (days), and w (weeks)"
    )]
    UnknownUnit { value: String, unit: String },

    #[error("'--age {0}' is not a valid number")]
    InvalidNumber(String),

    #[error("'--age {0}' must be greater than zero")]
    NotPositive(String),

    #[error("'--age {0}' is too large to represent as a duration")]
    TooLarge(String),

    #[error("'--age' value was empty")]
    Empty,
}

impl FromStr for AgeDuration {
    type Err = AgeDurationParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(AgeDurationParseError::Empty);
        }

        let unit_start = trimmed
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
            .map(|(i, _)| i);

        let Some(unit_start) = unit_start else {
            return Err(AgeDurationParseError::MissingUnit(trimmed.to_string()));
        };

        let (number_part, unit_part) = trimmed.split_at(unit_start);
        if number_part.is_empty() {
            return Err(AgeDurationParseError::InvalidNumber(trimmed.to_string()));
        }

        let value: f64 = number_part
            .parse()
            .map_err(|_| AgeDurationParseError::InvalidNumber(trimmed.to_string()))?;

        if value.is_nan() || value <= 0.0 {
            return Err(AgeDurationParseError::NotPositive(trimmed.to_string()));
        }

        let seconds_per_unit: f64 = match unit_part.to_ascii_lowercase().as_str() {
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
            "d" | "day" | "days" => 86_400.0,
            "w" | "wk" | "wks" | "week" | "weeks" => 604_800.0,
            other => {
                return Err(AgeDurationParseError::UnknownUnit {
                    value: trimmed.to_string(),
                    unit: other.to_string(),
                })
            }
        };

        let total_seconds = value * seconds_per_unit;
        // A sufficiently long digit string (or a merely huge but finite
        // value) parses fine as an f64 but overflows what `Duration` can
        // hold; `Duration::from_secs_f64` panics on both a non-finite input
        // and one that overflows, so both must be rejected here first.
        if !total_seconds.is_finite() || total_seconds > Duration::MAX.as_secs_f64() {
            return Err(AgeDurationParseError::TooLarge(trimmed.to_string()));
        }
        Ok(AgeDuration(Duration::from_secs_f64(total_seconds)))
    }
}

impl fmt::Display for AgeDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.as_secs();
        if secs % 604_800 == 0 && secs != 0 {
            write!(f, "{}w", secs / 604_800)
        } else if secs % 86_400 == 0 && secs != 0 {
            write!(f, "{}d", secs / 86_400)
        } else if secs % 3_600 == 0 && secs != 0 {
            write!(f, "{}h", secs / 3_600)
        } else if secs % 60 == 0 {
            write!(f, "{}m", secs / 60)
        } else {
            write!(f, "{secs}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bare_number() {
        let err = "2".parse::<AgeDuration>().unwrap_err();
        assert!(matches!(err, AgeDurationParseError::MissingUnit(_)));
    }

    #[test]
    fn parses_minutes_hours_days_weeks() {
        assert_eq!(
            "30m".parse::<AgeDuration>().unwrap().as_duration(),
            Duration::from_secs(1_800)
        );
        assert_eq!(
            "12h".parse::<AgeDuration>().unwrap().as_duration(),
            Duration::from_secs(43_200)
        );
        assert_eq!(
            "2d".parse::<AgeDuration>().unwrap().as_duration(),
            Duration::from_secs(172_800)
        );
        assert_eq!(
            "4w".parse::<AgeDuration>().unwrap().as_duration(),
            Duration::from_secs(2_419_200)
        );
    }

    #[test]
    fn rejects_unknown_unit() {
        let err = "2x".parse::<AgeDuration>().unwrap_err();
        assert!(matches!(err, AgeDurationParseError::UnknownUnit { .. }));
    }

    #[test]
    fn rejects_zero_and_negative() {
        assert!(matches!(
            "0d".parse::<AgeDuration>().unwrap_err(),
            AgeDurationParseError::NotPositive(_)
        ));
        assert!("-1d".parse::<AgeDuration>().is_err());
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(
            "".parse::<AgeDuration>().unwrap_err(),
            AgeDurationParseError::Empty
        );
    }

    #[test]
    fn accepts_fractional_values() {
        assert_eq!(
            "1.5h".parse::<AgeDuration>().unwrap().as_duration(),
            Duration::from_secs(5_400)
        );
    }

    #[test]
    fn rejects_digit_string_that_overflows_to_infinity() {
        let huge = "9".repeat(400);
        let err = format!("{huge}d").parse::<AgeDuration>().unwrap_err();
        assert!(matches!(err, AgeDurationParseError::TooLarge(_)));
    }

    #[test]
    fn rejects_finite_value_that_overflows_duration() {
        // 25 nines parses as a perfectly finite f64 (~1e25), but that is
        // already far beyond what `Duration` (max ~1.8e19 seconds) can
        // represent -- distinct from the `is_finite()` case above, where
        // the *parse itself* overflows to infinity.
        let big = "9".repeat(25);
        let err = format!("{big}w").parse::<AgeDuration>().unwrap_err();
        assert!(matches!(err, AgeDurationParseError::TooLarge(_)));
    }

    #[test]
    fn display_round_trips_common_units() {
        assert_eq!("2d".parse::<AgeDuration>().unwrap().to_string(), "2d");
        assert_eq!("4w".parse::<AgeDuration>().unwrap().to_string(), "4w");
    }
}
