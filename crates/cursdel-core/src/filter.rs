//! Composable file filters: `--include`, `--exclude`, `--min-size`,
//! `--max-size`, and `--age`/`--age-by`.
//!
//! Filters apply to files only (directories are never matched directly by
//! include/exclude/size/age -- they are removed when retention processing
//! leaves them empty; see `docs/RETENTION.md`). This keeps the filter
//! language small by design, per the product brief: CurseDelete is not a
//! general filesystem query engine.

use std::time::SystemTime;

use globset::{Glob, GlobMatcher};

use crate::entry::DiscoveredEntry;
use crate::size::ByteSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgeBasis {
    #[default]
    Modified,
    Created,
    Accessed,
}

impl AgeBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            AgeBasis::Modified => "modified",
            AgeBasis::Created => "created",
            AgeBasis::Accessed => "accessed",
        }
    }
}

impl std::str::FromStr for AgeBasis {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "modified" => Ok(AgeBasis::Modified),
            "created" => Ok(AgeBasis::Created),
            "accessed" => Ok(AgeBasis::Accessed),
            other => Err(format!(
                "unknown --age-by value '{other}': expected modified, created, or accessed"
            )),
        }
    }
}

/// Why a file was retained instead of deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainReason {
    TooYoung,
    ExcludedByPattern,
    NotIncludedByPattern,
    TooSmall,
    TooLarge,
    TimestampUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDecision {
    Delete,
    Retain(RetainReason),
}

pub struct FilterSet {
    include: Option<GlobMatcher>,
    exclude: Option<GlobMatcher>,
    min_size: Option<ByteSize>,
    max_size: Option<ByteSize>,
    age: Option<std::time::Duration>,
    age_basis: AgeBasis,
    /// Injectable "now" so age-boundary tests are deterministic.
    reference_time: SystemTime,
}

#[derive(Debug, Default, Clone)]
pub struct FilterSpec {
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub min_size: Option<ByteSize>,
    pub max_size: Option<ByteSize>,
    pub age: Option<std::time::Duration>,
    pub age_basis: AgeBasis,
}

impl FilterSpec {
    pub fn is_noop(&self) -> bool {
        self.include.is_none()
            && self.exclude.is_none()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.age.is_none()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FilterBuildError {
    #[error("invalid --include pattern '{pattern}': {source}")]
    InvalidInclude {
        pattern: String,
        #[source]
        source: globset::Error,
    },
    #[error("invalid --exclude pattern '{pattern}': {source}")]
    InvalidExclude {
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

impl FilterSet {
    pub fn build(spec: &FilterSpec) -> Result<Self, FilterBuildError> {
        Self::build_with_reference_time(spec, SystemTime::now())
    }

    pub fn build_with_reference_time(
        spec: &FilterSpec,
        reference_time: SystemTime,
    ) -> Result<Self, FilterBuildError> {
        let include = spec
            .include
            .as_ref()
            .map(|p| {
                Glob::new(p).map(|g| g.compile_matcher()).map_err(|source| {
                    FilterBuildError::InvalidInclude {
                        pattern: p.clone(),
                        source,
                    }
                })
            })
            .transpose()?;

        let exclude = spec
            .exclude
            .as_ref()
            .map(|p| {
                Glob::new(p).map(|g| g.compile_matcher()).map_err(|source| {
                    FilterBuildError::InvalidExclude {
                        pattern: p.clone(),
                        source,
                    }
                })
            })
            .transpose()?;

        Ok(FilterSet {
            include,
            exclude,
            min_size: spec.min_size,
            max_size: spec.max_size,
            age: spec.age,
            age_basis: spec.age_basis,
            reference_time,
        })
    }

    pub fn is_noop(&self) -> bool {
        self.include.is_none()
            && self.exclude.is_none()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.age.is_none()
    }

    /// Evaluate a *file* entry. Directories are never passed here -- callers
    /// route directories directly to the directory-completion tracker.
    pub fn evaluate_file(&self, entry: &DiscoveredEntry) -> FilterDecision {
        let file_name = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        if let Some(exclude) = &self.exclude {
            if exclude.is_match(&file_name) {
                return FilterDecision::Retain(RetainReason::ExcludedByPattern);
            }
        }

        if let Some(include) = &self.include {
            if !include.is_match(&file_name) {
                return FilterDecision::Retain(RetainReason::NotIncludedByPattern);
            }
        }

        if let Some(min) = self.min_size {
            if entry.size < min.bytes() {
                return FilterDecision::Retain(RetainReason::TooSmall);
            }
        }

        if let Some(max) = self.max_size {
            if entry.size > max.bytes() {
                return FilterDecision::Retain(RetainReason::TooLarge);
            }
        }

        if let Some(age) = self.age {
            let timestamp = match self.age_basis {
                AgeBasis::Modified => entry.times.modified,
                AgeBasis::Created => entry.times.created,
                AgeBasis::Accessed => entry.times.accessed,
            };

            match timestamp {
                None => return FilterDecision::Retain(RetainReason::TimestampUnavailable),
                Some(ts) => {
                    let elapsed = self
                        .reference_time
                        .duration_since(ts)
                        .unwrap_or(std::time::Duration::ZERO);
                    if elapsed < age {
                        return FilterDecision::Retain(RetainReason::TooYoung);
                    }
                }
            }
        }

        FilterDecision::Delete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryKind, EntryTimes};
    use std::path::PathBuf;
    use std::time::Duration;

    fn make_entry(name: &str, size: u64, modified_secs_ago: Option<u64>) -> DiscoveredEntry {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        DiscoveredEntry {
            path: PathBuf::from(name),
            dir_id: None,
            parent_dir_id: None,
            kind: EntryKind::File,
            size,
            times: EntryTimes {
                modified: modified_secs_ago.map(|s| now - Duration::from_secs(s)),
                created: None,
                accessed: None,
            },
            readonly: false,
        }
    }

    fn reference_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn no_filters_deletes_everything() {
        let filters =
            FilterSet::build_with_reference_time(&FilterSpec::default(), reference_time()).unwrap();
        assert_eq!(
            filters.evaluate_file(&make_entry("a.txt", 10, None)),
            FilterDecision::Delete
        );
    }

    #[test]
    fn age_boundary_exact_threshold_is_deleted() {
        let spec = FilterSpec {
            age: Some(Duration::from_secs(172_800)), // 2d
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        // Exactly 2 days old: elapsed >= age -> qualifies for deletion.
        let entry = make_entry("old.log", 10, Some(172_800));
        assert_eq!(filters.evaluate_file(&entry), FilterDecision::Delete);
    }

    #[test]
    fn age_boundary_one_second_younger_is_retained() {
        let spec = FilterSpec {
            age: Some(Duration::from_secs(172_800)),
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        let entry = make_entry("young.log", 10, Some(172_799));
        assert_eq!(
            filters.evaluate_file(&entry),
            FilterDecision::Retain(RetainReason::TooYoung)
        );
    }

    #[test]
    fn missing_timestamp_is_retained_not_deleted() {
        let spec = FilterSpec {
            age: Some(Duration::from_secs(60)),
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        let entry = make_entry("unknown.log", 10, None);
        assert_eq!(
            filters.evaluate_file(&entry),
            FilterDecision::Retain(RetainReason::TimestampUnavailable)
        );
    }

    #[test]
    fn include_pattern_matches_file_name_only() {
        let spec = FilterSpec {
            include: Some("*.log".to_string()),
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        assert_eq!(
            filters.evaluate_file(&make_entry("a.log", 10, None)),
            FilterDecision::Delete
        );
        assert_eq!(
            filters.evaluate_file(&make_entry("a.keep", 10, None)),
            FilterDecision::Retain(RetainReason::NotIncludedByPattern)
        );
    }

    #[test]
    fn exclude_wins_over_include() {
        let spec = FilterSpec {
            include: Some("*.log".to_string()),
            exclude: Some("*.keep.log".to_string()),
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        assert_eq!(
            filters.evaluate_file(&make_entry("a.keep.log", 10, None)),
            FilterDecision::Retain(RetainReason::ExcludedByPattern)
        );
    }

    #[test]
    fn size_bounds() {
        let spec = FilterSpec {
            min_size: Some(ByteSize(100)),
            max_size: Some(ByteSize(1000)),
            ..Default::default()
        };
        let filters = FilterSet::build_with_reference_time(&spec, reference_time()).unwrap();
        assert_eq!(
            filters.evaluate_file(&make_entry("tiny", 10, None)),
            FilterDecision::Retain(RetainReason::TooSmall)
        );
        assert_eq!(
            filters.evaluate_file(&make_entry("huge", 10_000, None)),
            FilterDecision::Retain(RetainReason::TooLarge)
        );
        assert_eq!(
            filters.evaluate_file(&make_entry("juuust right", 500, None)),
            FilterDecision::Delete
        );
    }
}
