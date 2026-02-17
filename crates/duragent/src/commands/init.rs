use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::fs;

use duragent::config::{
    DEFAULT_AGENTS_DIR, DEFAULT_ARTIFACTS_DIR, DEFAULT_DIRECTIVES_DIR, DEFAULT_SCHEDULES_DIR,
    DEFAULT_SESSIONS_DIR, DEFAULT_WORKSPACE, DEFAULT_WORLD_MEMORY_DIR,
};

// ============================================================================
// Templates (compiled into binary)
// ============================================================================

const TEMPLATE_DURAGENT_YAML: &str = include_str!("../../templates/duragent.yaml");
const TEMPLATE_POLICY_YAML: &str = include_str!("../../templates/policy.yaml");
pub(super) const TEMPLATE_AGENT_YAML: &str = include_str!("../../templates/agent.yaml");
pub(super) const TEMPLATE_AGENT_POLICY_YAML: &str =
    include_str!("../../templates/agent-policy.yaml");
pub(super) const TEMPLATE_SOUL: &str = include_str!("../../templates/SOUL.md");
pub(super) const TEMPLATE_SYSTEM_PROMPT: &str = include_str!("../../templates/SYSTEM_PROMPT.md");

// ============================================================================
// Defaults
// ============================================================================

const DEFAULT_AGENT_NAME: &str = "my-assistant";
pub(super) const DEFAULT_PROVIDER: &str = "openrouter";
pub(super) const DEFAULT_MODEL: &str = "moonshotai/kimi-k2.5";

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
        None if no_interactive => DEFAULT_AGENT_NAME.to_string(),
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
// Shared API (used by sibling modules)
// ============================================================================

/// Creates agent files (agent.yaml, policy.yaml, SOUL.md, SYSTEM_PROMPT.md) under
/// `agents_dir/{agent_name}/`. Returns lists of (created, skipped) absolute paths.
pub(super) async fn create_agent_files(
    agents_dir: &Path,
    agent_name: &str,
    provider: &str,
    model: &str,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let agent_yaml = TEMPLATE_AGENT_YAML
        .replace("{name}", agent_name)
        .replace("{provider}", provider)
        .replace("{model}", model);

    let agent_dir = agents_dir.join(agent_name);
    fs::create_dir_all(&agent_dir).await?;

    let files: &[(&Path, &str)] = &[
        (&agent_dir.join("agent.yaml"), &agent_yaml),
        (&agent_dir.join("policy.yaml"), TEMPLATE_AGENT_POLICY_YAML),
        (&agent_dir.join("SOUL.md"), TEMPLATE_SOUL),
        (&agent_dir.join("SYSTEM_PROMPT.md"), TEMPLATE_SYSTEM_PROMPT),
    ];

    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for (path, content) in files {
        if write_if_not_exists(path, content).await? {
            created.push(path.to_path_buf());
        } else {
            skipped.push(path.to_path_buf());
        }
    }

    Ok((created, skipped))
}

pub(super) fn print_file_summary(created: &[PathBuf], skipped: &[PathBuf]) {
    println!();
    if !created.is_empty() {
        println!("Created:");
        for path in created {
            println!("  {}", path.display());
        }
    }
    if !skipped.is_empty() {
        println!("Skipped (already exists):");
        for path in skipped {
            println!("  {}", path.display());
        }
    }
}

pub(super) fn credential_hint(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("Run: duragent login anthropic"),
        "openrouter" => Some("Run: export OPENROUTER_API_KEY=your-key"),
        "openai" => Some("Run: export OPENAI_API_KEY=your-key"),
        "ollama" => Some("Ensure Ollama is running: ollama serve"),
        _ => None,
    }
}

pub(super) fn prompt_with_default(prompt: &str, default: &str) -> Result<String> {
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

// ============================================================================
// Private
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

    let agents_dir = workspace.join(DEFAULT_AGENTS_DIR);

    // Write workspace-level files
    let workspace_files: &[(&Path, &str)] = &[
        (&root.join("duragent.yaml"), TEMPLATE_DURAGENT_YAML),
        (&workspace.join("policy.yaml"), TEMPLATE_POLICY_YAML),
    ];

    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for (path, content) in workspace_files {
        if write_if_not_exists(path, content).await? {
            created.push(path.strip_prefix(root).unwrap_or(path).to_path_buf());
        } else {
            skipped.push(path.strip_prefix(root).unwrap_or(path).to_path_buf());
        }
    }

    // Write agent files
    let (agent_created, agent_skipped) =
        create_agent_files(&agents_dir, agent_name, provider, model).await?;
    created.extend(
        agent_created
            .into_iter()
            .filter_map(|p| p.strip_prefix(root).ok().map(|r| r.to_path_buf())),
    );
    skipped.extend(
        agent_skipped
            .into_iter()
            .filter_map(|p| p.strip_prefix(root).ok().map(|r| r.to_path_buf())),
    );

    // Print summary
    print_file_summary(&created, &skipped);

    println!();
    println!("Workspace initialized! Next steps:");

    let step = credential_hint(provider);
    if let Some(hint) = &step {
        println!("  1. {hint}");
    }

    let chat_step = if step.is_some() { "2" } else { "1" };
    println!("  {chat_step}. Run: duragent chat --agent {agent_name}");
    println!();
    println!("Optional: export BRAVE_API_KEY=your-key  # enables web search");

    Ok(())
}

async fn write_if_not_exists(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    fs::write(path, content).await?;
    Ok(true)
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
        assert!(
            config.contains("host: 127.0.0.1"),
            "should bind to 127.0.0.1, not 0.0.0.0"
        );

        let agent =
            std::fs::read_to_string(root.join(".duragent/agents/test-bot/agent.yaml")).unwrap();
        assert!(agent.contains("name: test-bot"));
        assert!(agent.contains("provider: openrouter"));
        assert!(agent.contains("name: anthropic/claude-sonnet-4"));
        // memory section should be enabled (not commented)
        assert!(
            agent.contains("\n  memory:\n"),
            "memory section should be enabled"
        );
        // bash and web tools should be enabled (not commented)
        assert!(agent.contains("name: bash"), "bash tool should be enabled");
        assert!(agent.contains("name: web"), "web tool should be enabled");

        let workspace_policy = std::fs::read_to_string(root.join(".duragent/policy.yaml")).unwrap();
        assert!(workspace_policy.contains("deny:"));

        let agent_policy =
            std::fs::read_to_string(root.join(".duragent/agents/test-bot/policy.yaml")).unwrap();
        assert!(agent_policy.contains("mode: ask"));

        let soul = std::fs::read_to_string(root.join(".duragent/agents/test-bot/SOUL.md")).unwrap();
        assert!(soul.contains("helpful assistant"));

        let prompt =
            std::fs::read_to_string(root.join(".duragent/agents/test-bot/SYSTEM_PROMPT.md"))
                .unwrap();
        assert!(prompt.contains("Answer the user"));
        assert!(
            prompt.contains("{{date}}"),
            "should use {{date}} template syntax"
        );
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
        assert!(root.join(".duragent/agents/code-bot/policy.yaml").exists());
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
}
