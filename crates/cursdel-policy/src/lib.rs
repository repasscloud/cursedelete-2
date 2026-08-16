//! Maps a verified licence entitlement to a resolved capability set.
//!
//! The deletion engine (`cursdel-core`) never consumes this crate and
//! never will: per the product brief, raw deletion speed and the core
//! technical capabilities (adaptive/manual workers, `--force` remediation,
//! retention, filters, dry-run, JSON output) are identical in every
//! edition and are never gated here. `cursdel-cli` is the only consumer,
//! and uses [`Capabilities`] for exactly two things: deciding whether
//! `--close-remote-locks` may proceed (the one capability the product
//! brief unambiguously reserves for Business/Enterprise), and annotating
//! `license status`/`--json` output. Everything else the edition matrix
//! lists (commercial use, organisational deployment, priority support) is
//! a legal/licensing-terms distinction, not a technical gate -- enforcing
//! those at runtime would make the tool worse per the brief's own
//! guidance ("Commercial restrictions can be legal/licensing terms where
//! technical enforcement would make the tool worse").
//!
//! Absent or invalid licensing always resolves to [`Capabilities::community`]
//! -- fail *open* to the free tier, never fail closed to nothing. See
//! `docs/LICENSING.md` for the full edition matrix and the reasoning
//! behind the one ambiguous call this module makes (`--kill-locks`
//! available in every edition despite the summary matrix's "Optional"
//! wording for Community/Education -- see [`Capabilities`] docs).

use cursdel_license::ProductEntitlement;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Community,
    Education,
    Business,
    Enterprise,
}

impl Edition {
    /// Maps a licence's free-form `edition` string to CurseDelete's four
    /// technical tiers. Unknown/unrecognised values fail closed to
    /// [`Edition::Community`] -- an edition string that doesn't match
    /// anything we know about must never accidentally grant more than the
    /// free tier. `smb`/`corporate` (optional pricing-band SKUs from the
    /// product brief §26) map onto their nearest technical tier
    /// (`business`/`enterprise` respectively) since they carry the same
    /// technical capabilities and differ only in seat/scale terms, which
    /// this crate does not enforce.
    pub fn from_license_str(edition: &str) -> Edition {
        match edition.to_ascii_lowercase().as_str() {
            "business" | "smb" | "project" => Edition::Business,
            "enterprise" | "corporate" => Edition::Enterprise,
            "education" => Edition::Education,
            "community" | "consumer" => Edition::Community,
            _ => Edition::Community,
        }
    }
}

/// The resolved, technical capability set for one edition. See the module
/// docs for what this is -- and is not -- used to gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub edition: Edition,
    /// Windows administrative closure of a remote SMB open
    /// (`--close-remote-locks`). The one capability the product brief
    /// unambiguously reserves: "Remote Windows `--close-remote-locks`" is
    /// listed as `No | No | Yes | Yes` across Community/Education/
    /// Business/Enterprise with no qualifier, unlike every other
    /// capability in that matrix.
    pub close_remote_locks: bool,
    /// Structured audit/reporting beyond basic `--json` (e.g. richer
    /// JSONL event streams, Windows Event Log integration) -- tracked for
    /// future use; not yet gating any implemented output.
    pub advanced_audit: bool,
    /// Informational only: whether this edition's licence terms include
    /// unattended/CI/scheduled automation rights. Not technically
    /// enforced (see module docs).
    pub unattended_automation_licensed: bool,
    pub priority_support: bool,
}

impl Capabilities {
    pub fn for_edition(edition: Edition) -> Self {
        match edition {
            Edition::Community => Capabilities {
                edition,
                close_remote_locks: false,
                advanced_audit: false,
                unattended_automation_licensed: false,
                priority_support: false,
            },
            Edition::Education => Capabilities {
                edition,
                close_remote_locks: false,
                advanced_audit: false,
                unattended_automation_licensed: true,
                priority_support: false,
            },
            Edition::Business => Capabilities {
                edition,
                close_remote_locks: true,
                advanced_audit: true,
                unattended_automation_licensed: true,
                priority_support: false,
            },
            Edition::Enterprise => Capabilities {
                edition,
                close_remote_locks: true,
                advanced_audit: true,
                unattended_automation_licensed: true,
                priority_support: true,
            },
        }
    }

    pub fn community() -> Self {
        Self::for_edition(Edition::Community)
    }

    pub fn from_entitlement(entitlement: &ProductEntitlement) -> Self {
        Self::for_edition(Edition::from_license_str(&entitlement.edition))
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::community()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_edition_string_fails_closed_to_community() {
        assert_eq!(
            Edition::from_license_str("mystery-tier"),
            Edition::Community
        );
    }

    #[test]
    fn recognises_all_four_core_editions_case_insensitively() {
        assert_eq!(Edition::from_license_str("BUSINESS"), Edition::Business);
        assert_eq!(Edition::from_license_str("Enterprise"), Edition::Enterprise);
        assert_eq!(Edition::from_license_str("education"), Edition::Education);
        assert_eq!(Edition::from_license_str("Community"), Edition::Community);
    }

    #[test]
    fn only_business_and_enterprise_get_remote_lock_capability() {
        assert!(!Capabilities::for_edition(Edition::Community).close_remote_locks);
        assert!(!Capabilities::for_edition(Edition::Education).close_remote_locks);
        assert!(Capabilities::for_edition(Edition::Business).close_remote_locks);
        assert!(Capabilities::for_edition(Edition::Enterprise).close_remote_locks);
    }

    #[test]
    fn default_and_community_are_the_same_fail_open_tier() {
        assert_eq!(Capabilities::default(), Capabilities::community());
        assert_eq!(Capabilities::default().edition, Edition::Community);
    }
}
