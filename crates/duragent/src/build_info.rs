use serde::Serialize;

// ============================================================================
// Constants
// ============================================================================

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = match option_env!("BUILD_COMMIT") {
    Some(c) => c,
    None => "unknown",
};
pub const BUILD_DATE: &str = match option_env!("BUILD_DATE") {
    Some(d) => d,
    None => "unknown",
};

/// Version string built at compile time.
///
/// Note: Due to const limitations, this is built dynamically in `version_string()`.
pub const VERSION_STRING: &str = env!("CARGO_PKG_VERSION");

/// Get the full version string including commit and build date.
pub fn version_string() -> String {
    format!("{} (commit: {}, built: {})", VERSION, COMMIT, BUILD_DATE)
}

// ============================================================================
// BuildInfo
// ============================================================================

#[derive(Debug, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
    pub build_date: &'static str,
}

impl BuildInfo {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: VERSION,
            commit: COMMIT,
            build_date: BUILD_DATE,
        }
    }
}

impl Default for BuildInfo {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_info_new() {
        let info = BuildInfo::new();
        assert!(!info.version.is_empty());
        assert!(!info.commit.is_empty());
        assert!(!info.build_date.is_empty());
    }

    #[test]
    fn test_build_info_default() {
        let info = BuildInfo::default();
        assert_eq!(info.version, VERSION);
        assert_eq!(info.commit, COMMIT);
        assert_eq!(info.build_date, BUILD_DATE);
    }

    #[test]
    fn test_version_string_format() {
        let vs = version_string();
        assert!(vs.contains(VERSION));
        assert!(vs.contains("commit:"));
        assert!(vs.contains("built:"));
    }
}
