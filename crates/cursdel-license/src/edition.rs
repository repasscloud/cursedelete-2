//! The four editions every application supports. Which features an
//! edition unlocks is decided by each application, never here.

use std::fmt;

/// Declaration order is the precedence used when several valid licences
/// are installed at once (the highest wins); it says nothing about which
/// features an edition has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edition {
    Community,
    Education,
    Business,
    Enterprise,
}

impl Edition {
    pub const ALL: [Edition; 4] = [
        Edition::Community,
        Edition::Education,
        Edition::Business,
        Edition::Enterprise,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Edition::Community => "community",
            Edition::Education => "education",
            Edition::Business => "business",
            Edition::Enterprise => "enterprise",
        }
    }

    /// Exact lowercase match. An unrecognised edition is a malformed
    /// licence, never a silent downgrade or upgrade.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id() == id)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Edition::Community => "Community",
            Edition::Education => "Education",
            Edition::Business => "Business",
            Edition::Enterprise => "Enterprise",
        }
    }

    /// Business is the only edition with per-device activation.
    pub fn requires_activation(self) -> bool {
        self == Edition::Business
    }
}

impl fmt::Display for Edition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_exact() {
        for e in Edition::ALL {
            assert_eq!(Edition::from_id(e.id()), Some(e));
        }
        assert_eq!(Edition::from_id("Business"), None);
        assert_eq!(Edition::from_id("smb"), None);
    }

    #[test]
    fn only_business_requires_activation() {
        let requiring: Vec<_> = Edition::ALL
            .into_iter()
            .filter(|e| e.requires_activation())
            .collect();
        assert_eq!(requiring, vec![Edition::Business]);
    }
}
