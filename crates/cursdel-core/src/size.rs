//! Byte-size parsing for `--min-size` / `--max-size`.
//!
//! Unlike `--age`, a bare number is accepted here and means bytes -- there
//! is no ambiguity to guard against the way there is with duration units.
//! Suffixes use binary (1024-based) multiples, matching the convention of
//! `du`, `ls -h`, and most sysadmin tooling: `k`/`m`/`g`/`t` mean
//! KiB/MiB/GiB/TiB respectively. This is a deliberate, documented choice
//! (see `docs/COMMAND_REFERENCE.md`), not an accidental deviation from SI.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub const fn bytes(self) -> u64 {
        self.0
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ByteSizeParseError {
    #[error("size value was empty")]
    Empty,
    #[error("'{0}' is not a valid size")]
    InvalidNumber(String),
    #[error("'{value}' uses unknown size unit '{unit}'. Supported units are b, k, m, g, t")]
    UnknownUnit { value: String, unit: String },
    #[error("'{0}' must not be negative")]
    Negative(String),
}

impl FromStr for ByteSize {
    type Err = ByteSizeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(ByteSizeParseError::Empty);
        }

        if trimmed.starts_with('-') {
            return Err(ByteSizeParseError::Negative(trimmed.to_string()));
        }

        let unit_start = trimmed
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
            .map(|(i, _)| i);

        let (number_part, unit_part) = match unit_start {
            Some(i) => trimmed.split_at(i),
            None => (trimmed, ""),
        };

        if number_part.is_empty() {
            return Err(ByteSizeParseError::InvalidNumber(trimmed.to_string()));
        }

        let value: f64 = number_part
            .parse()
            .map_err(|_| ByteSizeParseError::InvalidNumber(trimmed.to_string()))?;

        let multiplier: f64 = match unit_part.to_ascii_lowercase().as_str() {
            "" | "b" => 1.0,
            "k" | "kb" | "kib" => 1024.0,
            "m" | "mb" | "mib" => 1024.0 * 1024.0,
            "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
            "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
            other => {
                return Err(ByteSizeParseError::UnknownUnit {
                    value: trimmed.to_string(),
                    unit: other.to_string(),
                })
            }
        };

        Ok(ByteSize((value * multiplier).round() as u64))
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [(&str, u64); 4] = [
            ("TiB", 1024 * 1024 * 1024 * 1024),
            ("GiB", 1024 * 1024 * 1024),
            ("MiB", 1024 * 1024),
            ("KiB", 1024),
        ];
        for (name, threshold) in UNITS {
            if self.0 >= threshold {
                return write!(f, "{:.2} {name}", self.0 as f64 / threshold as f64);
            }
        }
        write!(f, "{} B", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_bytes() {
        assert_eq!("512".parse::<ByteSize>().unwrap().bytes(), 512);
    }

    #[test]
    fn parses_common_units() {
        assert_eq!(
            "100m".parse::<ByteSize>().unwrap().bytes(),
            100 * 1024 * 1024
        );
        assert_eq!(
            "10g".parse::<ByteSize>().unwrap().bytes(),
            10 * 1024 * 1024 * 1024
        );
        assert_eq!("1k".parse::<ByteSize>().unwrap().bytes(), 1024);
        assert_eq!(
            "1t".parse::<ByteSize>().unwrap().bytes(),
            1024u64 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn rejects_unknown_unit() {
        assert!(matches!(
            "10x".parse::<ByteSize>().unwrap_err(),
            ByteSizeParseError::UnknownUnit { .. }
        ));
    }

    #[test]
    fn rejects_negative() {
        assert!(matches!(
            "-5m".parse::<ByteSize>().unwrap_err(),
            ByteSizeParseError::Negative(_)
        ));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(
            "".parse::<ByteSize>().unwrap_err(),
            ByteSizeParseError::Empty
        );
    }
}
