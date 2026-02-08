//! Skill metadata types and SKILL.md frontmatter parser.
//!
//! Implements the [Agent Skills](https://agentskills.io) open standard:
//! SKILL.md files with YAML frontmatter containing `name`, `description`,
//! `allowed-tools`, and optional `metadata`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Parsed skill metadata from a SKILL.md file.
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    /// Skill name (lowercase, hyphens only, ≤64 chars).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Path to the skill directory containing SKILL.md.
    pub skill_path: PathBuf,
    /// Tool names this skill uses (from `allowed-tools` frontmatter).
    pub allowed_tools: Vec<String>,
    /// Arbitrary key-value metadata from frontmatter.
    pub metadata: HashMap<String, String>,
}

/// Error parsing SKILL.md frontmatter.
#[derive(Debug, Error)]
pub enum SkillParseError {
    #[error("no YAML frontmatter found (expected --- delimiters)")]
    NoFrontmatter,

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_saphyr::Error),

    #[error("validation error: {0}")]
    Validation(String),
}

/// Raw YAML frontmatter from SKILL.md (serde target).
#[derive(Debug, Deserialize)]
struct RawSkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

/// Parse SKILL.md content into `SkillMetadata`.
///
/// Expects YAML frontmatter between `---` delimiters (the first non-blank line
/// must be `---`). The `skill_path` is the directory containing the SKILL.md.
pub fn parse_skill_frontmatter(
    content: &str,
    skill_path: PathBuf,
) -> Result<SkillMetadata, SkillParseError> {
    let yaml = extract_frontmatter(content)?;
    let raw: RawSkillFrontmatter = serde_saphyr::from_str(&yaml)?;

    let name = raw
        .name
        .ok_or_else(|| SkillParseError::Validation("'name' is required".to_string()))?;

    validate_name(&name)?;

    // Validate name matches directory name
    if let Some(dir_name) = skill_path.file_name().and_then(|n| n.to_str())
        && dir_name != name
    {
        return Err(SkillParseError::Validation(format!(
            "name '{}' does not match directory name '{}'",
            name, dir_name
        )));
    }

    let description = raw
        .description
        .ok_or_else(|| SkillParseError::Validation("'description' is required".to_string()))?;

    let allowed_tools = raw
        .allowed_tools
        .map(|s| {
            s.split_whitespace()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(SkillMetadata {
        name,
        description,
        skill_path,
        allowed_tools,
        metadata: raw.metadata,
    })
}

/// Extract YAML frontmatter from content between `---` delimiters.
fn extract_frontmatter(content: &str) -> Result<String, SkillParseError> {
    // Skip leading blank lines and optional H1 title before frontmatter
    let trimmed = content.trim_start();

    // Find the opening --- (may be at start, or after H1 title lines)
    let rest = if let Some(rest) = trimmed.strip_prefix("---") {
        rest.to_string()
    } else {
        // Allow H1 title before frontmatter (common in existing SKILL.md files)
        let after_title: String = trimmed
            .lines()
            .skip_while(|line| line.starts_with('#') || line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let after_title = after_title.trim_start().to_string();
        if let Some(rest) = after_title.strip_prefix("---") {
            rest.to_string()
        } else {
            return Err(SkillParseError::NoFrontmatter);
        }
    };

    // Find the closing ---
    if let Some(end) = rest.find("\n---") {
        Ok(rest[..end].to_string())
    } else {
        Err(SkillParseError::NoFrontmatter)
    }
}

/// Validate skill name: non-empty, ≤64 chars, lowercase + hyphens only.
fn validate_name(name: &str) -> Result<(), SkillParseError> {
    if name.is_empty() {
        return Err(SkillParseError::Validation(
            "name must not be empty".to_string(),
        ));
    }
    if name.len() > 64 {
        return Err(SkillParseError::Validation(format!(
            "name '{}' exceeds 64 characters",
            name
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
    {
        return Err(SkillParseError::Validation(format!(
            "name '{}' must contain only lowercase letters, digits, and hyphens",
            name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_frontmatter() {
        let content = r#"---
name: task-extraction
description: Extract actionable tasks from a message
allowed-tools: bash calculator
metadata:
  version: "1.0.0"
  author: Duragent
---

## Instructions

Do stuff.
"#;
        let skill =
            parse_skill_frontmatter(content, PathBuf::from("/agents/a/skills/task-extraction"))
                .unwrap();

        assert_eq!(skill.name, "task-extraction");
        assert_eq!(skill.description, "Extract actionable tasks from a message");
        assert_eq!(skill.allowed_tools, vec!["bash", "calculator"]);
        assert_eq!(skill.metadata.get("version").unwrap(), "1.0.0");
        assert_eq!(skill.metadata.get("author").unwrap(), "Duragent");
    }

    #[test]
    fn parse_frontmatter_with_h1_title() {
        let content = r#"# Task Extraction

---
name: task-extraction
description: Extract tasks
---

Body text.
"#;
        let skill =
            parse_skill_frontmatter(content, PathBuf::from("/skills/task-extraction")).unwrap();
        assert_eq!(skill.name, "task-extraction");
    }

    #[test]
    fn parse_frontmatter_no_allowed_tools() {
        let content = r#"---
name: simple-skill
description: A simple skill
---
"#;
        let skill =
            parse_skill_frontmatter(content, PathBuf::from("/skills/simple-skill")).unwrap();
        assert!(skill.allowed_tools.is_empty());
        assert!(skill.metadata.is_empty());
    }

    #[test]
    fn parse_no_frontmatter() {
        let content = "# Just a title\n\nSome text.";
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/test")).unwrap_err();
        assert!(matches!(err, SkillParseError::NoFrontmatter));
    }

    #[test]
    fn parse_missing_name() {
        let content = r#"---
description: No name here
---
"#;
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/test")).unwrap_err();
        assert!(matches!(err, SkillParseError::Validation(_)));
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn parse_missing_description() {
        let content = r#"---
name: test
---
"#;
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/test")).unwrap_err();
        assert!(err.to_string().contains("description"));
    }

    #[test]
    fn parse_name_too_long() {
        let long_name = "a".repeat(65);
        let content = format!("---\nname: {}\ndescription: test\n---\n", long_name);
        let err =
            parse_skill_frontmatter(&content, PathBuf::from(format!("/skills/{}", long_name)))
                .unwrap_err();
        assert!(err.to_string().contains("64 characters"));
    }

    #[test]
    fn parse_name_invalid_chars() {
        let content = r#"---
name: Bad_Name
description: test
---
"#;
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/Bad_Name")).unwrap_err();
        assert!(err.to_string().contains("lowercase"));
    }

    #[test]
    fn parse_name_directory_mismatch() {
        let content = r#"---
name: skill-a
description: test
---
"#;
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/skill-b")).unwrap_err();
        assert!(err.to_string().contains("does not match directory"));
    }

    #[test]
    fn parse_invalid_yaml() {
        let content = "---\n: invalid: yaml::\n---\n";
        let err = parse_skill_frontmatter(content, PathBuf::from("/skills/test")).unwrap_err();
        assert!(matches!(err, SkillParseError::Yaml(_)));
    }

    #[test]
    fn parse_allowed_tools_single() {
        let content = r#"---
name: one-tool
description: test
allowed-tools: bash
---
"#;
        let skill = parse_skill_frontmatter(content, PathBuf::from("/skills/one-tool")).unwrap();
        assert_eq!(skill.allowed_tools, vec!["bash"]);
    }

    #[test]
    fn parse_name_with_digits() {
        let content = r#"---
name: skill-v2
description: test
---
"#;
        let skill = parse_skill_frontmatter(content, PathBuf::from("/skills/skill-v2")).unwrap();
        assert_eq!(skill.name, "skill-v2");
    }
}
