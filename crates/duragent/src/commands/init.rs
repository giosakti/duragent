use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::Result;
use rand::Rng;
use tokio::fs;

use duragent::config::{
    DEFAULT_AGENTS_DIR, DEFAULT_ARTIFACTS_DIR, DEFAULT_DIRECTIVES_DIR, DEFAULT_SCHEDULES_DIR,
    DEFAULT_SESSIONS_DIR, DEFAULT_WORKSPACE, DEFAULT_WORLD_MEMORY_DIR,
};

// ============================================================================
// Templates (compiled into binary)
// ============================================================================

const TEMPLATE_DURAGENT_YAML: &str = include_str!("../../templates/duragent.yaml");
const TEMPLATE_AGENT_YAML: &str = include_str!("../../templates/agent.yaml");
const TEMPLATE_SOUL: &str = include_str!("../../templates/SOUL.md");
const TEMPLATE_SYSTEM_PROMPT: &str = include_str!("../../templates/SYSTEM_PROMPT.md");

// ============================================================================
// Defaults
// ============================================================================

const DEFAULT_AGENT_NAME: &str = "my-assistant";
const DEFAULT_PROVIDER: &str = "openrouter";
const DEFAULT_MODEL: &str = "moonshotai/kimi-k2.5";

// ============================================================================
// Public Entry Point
// ============================================================================

pub async fn run(
    path: &Path,
    agent_name: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    no_interactive: bool,
) -> Result<()> {
    let agent_name = match agent_name {
        Some(name) => name,
        None if no_interactive => generate_random_name(),
        None => prompt_with_default("Agent name", DEFAULT_AGENT_NAME)?,
    };

    let provider = match provider {
        Some(p) => p,
        None if no_interactive => DEFAULT_PROVIDER.to_string(),
        None => prompt_with_default(
            "LLM provider (anthropic, openrouter, openai, ollama)",
            DEFAULT_PROVIDER,
        )?,
    };

    let model = match model {
        Some(m) => m,
        None if no_interactive => DEFAULT_MODEL.to_string(),
        None => prompt_with_default("Model name", DEFAULT_MODEL)?,
    };

    init_at(path, &agent_name, &provider, &model).await
}

// ============================================================================
// Core Logic (testable)
// ============================================================================

async fn init_at(root: &Path, agent_name: &str, provider: &str, model: &str) -> Result<()> {
    let workspace = root.join(DEFAULT_WORKSPACE);

    // Create directories
    let dirs = [
        workspace.join(DEFAULT_AGENTS_DIR),
        workspace.join(DEFAULT_SESSIONS_DIR),
        workspace.join(DEFAULT_WORLD_MEMORY_DIR),
        workspace.join(DEFAULT_DIRECTIVES_DIR),
        workspace.join(DEFAULT_SCHEDULES_DIR),
        workspace.join(DEFAULT_ARTIFACTS_DIR),
    ];

    for dir in &dirs {
        fs::create_dir_all(dir).await?;
    }

    // Prepare agent.yaml content with placeholder replacement
    let agent_yaml = TEMPLATE_AGENT_YAML
        .replace("{name}", agent_name)
        .replace("{provider}", provider)
        .replace("{model}", model);

    // Write files
    let agent_dir = workspace.join(DEFAULT_AGENTS_DIR).join(agent_name);
    fs::create_dir_all(&agent_dir).await?;

    let files: &[(&Path, &str)] = &[
        (&root.join("duragent.yaml"), TEMPLATE_DURAGENT_YAML),
        (&agent_dir.join("agent.yaml"), &agent_yaml),
        (&agent_dir.join("SOUL.md"), TEMPLATE_SOUL),
        (&agent_dir.join("SYSTEM_PROMPT.md"), TEMPLATE_SYSTEM_PROMPT),
    ];

    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for (path, content) in files {
        if write_if_not_exists(path, content).await? {
            created.push(path.strip_prefix(root).unwrap_or(path));
        } else {
            skipped.push(path.strip_prefix(root).unwrap_or(path));
        }
    }

    // Print summary
    println!();
    if !created.is_empty() {
        println!("Created:");
        for path in &created {
            println!("  {}", path.display());
        }
    }
    if !skipped.is_empty() {
        println!("Skipped (already exists):");
        for path in &skipped {
            println!("  {}", path.display());
        }
    }

    println!();
    println!("Workspace initialized! Next steps:");
    println!("  1. Edit .duragent/agents/{agent_name}/SOUL.md to define agent personality");
    println!("  2. Edit .duragent/agents/{agent_name}/SYSTEM_PROMPT.md to define agent task");
    println!("  3. Run: duragent chat --agent {agent_name}");

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

/// Writes content to path only if the file does not already exist.
/// Returns `true` if the file was written, `false` if skipped.
async fn write_if_not_exists(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    fs::write(path, content).await?;
    Ok(true)
}

/// Prompts the user for input with a default value.
/// Returns the default if the user presses Enter without typing anything.
fn prompt_with_default(prompt: &str, default: &str) -> Result<String> {
    print!("{prompt} [{default}]: ");
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();

    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Generates a random agent name in the format `agent-{adjective}-{noun}`.
fn generate_random_name() -> String {
    const ADJECTIVES: &[&str] = &[
        "swift", "bright", "calm", "keen", "bold", "warm", "crisp", "sharp", "quick", "steady",
    ];
    const NOUNS: &[&str] = &[
        "falcon", "river", "oak", "flame", "wave", "frost", "spark", "panda", "bloom", "ridge",
    ];

    let mut rng = rand::rng();
    let adj = ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
    let noun = NOUNS[rng.random_range(0..NOUNS.len())];
    format!("agent-{adj}-{noun}")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_init_happy_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        init_at(root, "test-bot", "openrouter", "anthropic/claude-sonnet-4")
            .await
            .unwrap();

        // Directories exist
        assert!(root.join(".duragent/agents").is_dir());
        assert!(root.join(".duragent/sessions").is_dir());
        assert!(root.join(".duragent/memory/world").is_dir());
        assert!(root.join(".duragent/directives").is_dir());
        assert!(root.join(".duragent/schedules").is_dir());
        assert!(root.join(".duragent/artifacts").is_dir());

        // Files exist with correct content
        let config = std::fs::read_to_string(root.join("duragent.yaml")).unwrap();
        assert!(config.contains("server:"));
        assert!(config.contains("port: 8080"));

        let agent =
            std::fs::read_to_string(root.join(".duragent/agents/test-bot/agent.yaml")).unwrap();
        assert!(agent.contains("name: test-bot"));
        assert!(agent.contains("provider: openrouter"));
        assert!(agent.contains("name: anthropic/claude-sonnet-4"));

        let soul = std::fs::read_to_string(root.join(".duragent/agents/test-bot/SOUL.md")).unwrap();
        assert!(soul.contains("helpful assistant"));

        let prompt =
            std::fs::read_to_string(root.join(".duragent/agents/test-bot/SYSTEM_PROMPT.md"))
                .unwrap();
        assert!(prompt.contains("Answer the user"));
    }

    #[tokio::test]
    async fn test_init_idempotent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // First run
        init_at(root, "test-bot", "openrouter", "anthropic/claude-sonnet-4")
            .await
            .unwrap();

        // Modify a file to verify it's not overwritten
        let agent_path = root.join(".duragent/agents/test-bot/agent.yaml");
        std::fs::write(&agent_path, "modified content").unwrap();

        // Second run
        init_at(root, "test-bot", "openrouter", "anthropic/claude-sonnet-4")
            .await
            .unwrap();

        // File should retain modified content
        let content = std::fs::read_to_string(&agent_path).unwrap();
        assert_eq!(content, "modified content");
    }

    #[tokio::test]
    async fn test_init_custom_agent_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        init_at(root, "code-bot", "openrouter", "anthropic/claude-sonnet-4")
            .await
            .unwrap();

        assert!(root.join(".duragent/agents/code-bot/agent.yaml").exists());
        assert!(root.join(".duragent/agents/code-bot/SOUL.md").exists());
        assert!(
            root.join(".duragent/agents/code-bot/SYSTEM_PROMPT.md")
                .exists()
        );

        let agent =
            std::fs::read_to_string(root.join(".duragent/agents/code-bot/agent.yaml")).unwrap();
        assert!(agent.contains("name: code-bot"));
    }

    #[tokio::test]
    async fn test_init_custom_provider_and_model() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        init_at(root, "my-bot", "anthropic", "claude-sonnet-4-20250514")
            .await
            .unwrap();

        let agent =
            std::fs::read_to_string(root.join(".duragent/agents/my-bot/agent.yaml")).unwrap();
        assert!(agent.contains("provider: anthropic"));
        assert!(agent.contains("name: claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_generate_random_name() {
        let name = generate_random_name();
        assert!(
            name.starts_with("agent-"),
            "name should start with 'agent-': {name}"
        );

        let parts: Vec<&str> = name.splitn(3, '-').collect();
        assert_eq!(
            parts.len(),
            3,
            "name should have format agent-adj-noun: {name}"
        );
        assert_eq!(parts[0], "agent");
        assert!(!parts[1].is_empty());
        assert!(!parts[2].is_empty());
    }
}
