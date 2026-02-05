//! Simple, human-readable agent memory system.
//!
//! Memory is stored as plain markdown files:
//! - `world/*.md` — Shared facts (all agents see)
//! - `agents/{id}/MEMORY.md` — Agent's curated long-term memory
//! - `agents/{id}/daily/*.md` — Agent's daily experiences (append-only)

mod tools;

pub use tools::{LearnFactTool, RecallTool, ReflectTool, RememberTool};

use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;

/// Memory system for reading and writing agent memories.
#[derive(Debug, Clone)]
pub struct Memory {
    workspace: PathBuf,
}

impl Memory {
    /// Create a new memory instance with the given workspace directory.
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Read memory context (called by recall tool).
    ///
    /// Returns concatenated content from:
    /// 1. World knowledge (`world/*.md`)
    /// 2. Agent's curated memory (`agents/{id}/MEMORY.md`)
    /// 3. Recent daily experiences (`agents/{id}/daily/*.md`)
    pub fn recall(&self, agent_id: &str, days: usize) -> Result<String> {
        let mut sections = vec![];

        // World knowledge
        let world = self.read_world_facts()?;
        if !world.is_empty() {
            sections.push(format!("# World Knowledge\n\n{}", world));
        }

        // Agent's curated memory
        let memory_path = self
            .workspace
            .join("agents")
            .join(agent_id)
            .join("MEMORY.md");
        if let Ok(memory) = std::fs::read_to_string(&memory_path) {
            if !memory.is_empty() {
                sections.push(format!("# Your Memory\n\n{}", memory));
            }
        }

        // Recent daily experiences
        let recent = self.read_recent_daily(agent_id, days)?;
        if !recent.is_empty() {
            sections.push(format!("# Recent Experiences\n\n{}", recent));
        }

        Ok(sections.join("\n\n---\n\n"))
    }

    /// Read all world/*.md files.
    fn read_world_facts(&self) -> Result<String> {
        let world_dir = self.workspace.join("world");
        if !world_dir.exists() {
            return Ok(String::new());
        }

        let mut entries: Vec<_> = std::fs::read_dir(&world_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .collect();

        // Sort by filename for consistent ordering
        entries.sort_by_key(|e| e.file_name());

        let mut content = vec![];
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let text = std::fs::read_to_string(&path)?;
            content.push(format!("## {}\n\n{}", name, text));
        }

        Ok(content.join("\n\n"))
    }

    /// Read last N days of daily logs.
    fn read_recent_daily(&self, agent_id: &str, days: usize) -> Result<String> {
        let daily_dir = self.workspace.join("agents").join(agent_id).join("daily");

        if !daily_dir.exists() {
            return Ok(String::new());
        }

        let today = Local::now().date_naive();
        let mut content = vec![];

        for i in 0..days {
            let date = today - chrono::Duration::days(i as i64);
            let filename = format!("{}.md", date.format("%Y-%m-%d"));
            let path = daily_dir.join(&filename);

            if path.exists() {
                let text = std::fs::read_to_string(&path)?;
                content.push(text);
            }
        }

        Ok(content.join("\n\n---\n\n"))
    }

    /// Append to today's daily log.
    pub fn append_daily(&self, agent_id: &str, content: &str) -> Result<PathBuf> {
        let daily_dir = self.workspace.join("agents").join(agent_id).join("daily");
        std::fs::create_dir_all(&daily_dir)?;

        let today = Local::now();
        let filename = format!("{}.md", today.format("%Y-%m-%d"));
        let path = daily_dir.join(&filename);

        let timestamp = today.format("%H:%M");
        let entry = format!("\n## {}\n\n{}\n", timestamp, content);

        // Create file with header if it doesn't exist
        if !path.exists() {
            let header = format!("# {}\n", today.format("%Y-%m-%d"));
            std::fs::write(&path, header)?;
        }

        // Append entry
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
        file.write_all(entry.as_bytes())?;

        Ok(path)
    }

    /// Rewrite agent's MEMORY.md.
    pub fn write_memory(&self, agent_id: &str, content: &str) -> Result<PathBuf> {
        let agent_dir = self.workspace.join("agents").join(agent_id);
        std::fs::create_dir_all(&agent_dir)?;

        let path = agent_dir.join("MEMORY.md");
        let temp = agent_dir.join("MEMORY.md.tmp");

        // Atomic write
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &path)?;

        Ok(path)
    }

    /// Write world knowledge for a topic (replaces existing content).
    pub fn write_world(&self, topic: &str, content: &str) -> Result<PathBuf> {
        let world_dir = self.workspace.join("world");
        std::fs::create_dir_all(&world_dir)?;

        // Sanitize topic name (only alphanumeric, dash, underscore)
        let safe_topic: String = topic
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();

        let filename = format!("{}.md", safe_topic);
        let path = world_dir.join(&filename);

        // Atomic write
        let temp = world_dir.join(format!("{}.md.tmp", safe_topic));
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &path)?;

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Memory) {
        let temp = TempDir::new().unwrap();
        let memory = Memory::new(temp.path().to_path_buf());
        (temp, memory)
    }

    #[test]
    fn recall_empty_workspace() {
        let (_temp, memory) = setup();
        let result = memory.recall("agent-1", 3).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn recall_includes_all_sections() {
        let (_temp, memory) = setup();

        // Add world knowledge
        memory
            .write_world("systems", "## Production\n- DB: PostgreSQL")
            .unwrap();

        // Add agent memory
        memory
            .write_memory("agent-1", "User prefers concise responses")
            .unwrap();

        // Add daily experience
        memory
            .append_daily("agent-1", "Helped debug issue")
            .unwrap();

        let result = memory.recall("agent-1", 3).unwrap();

        assert!(result.contains("# World Knowledge"));
        assert!(result.contains("## systems"));
        assert!(result.contains("PostgreSQL"));

        assert!(result.contains("# Your Memory"));
        assert!(result.contains("concise responses"));

        assert!(result.contains("# Recent Experiences"));
        assert!(result.contains("Helped debug issue"));
    }

    #[test]
    fn recall_respects_days_limit() {
        let (temp, memory) = setup();

        // Create a daily file for "yesterday" manually
        let daily_dir = temp.path().join("agents/agent-1/daily");
        std::fs::create_dir_all(&daily_dir).unwrap();

        let yesterday = (Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d");
        let yesterday_file = daily_dir.join(format!("{}.md", yesterday));
        std::fs::write(&yesterday_file, "# Yesterday\n\nOld stuff").unwrap();

        // Create today's file
        memory.append_daily("agent-1", "Today's stuff").unwrap();

        // Recall with 1 day should only get today
        let result = memory.recall("agent-1", 1).unwrap();
        assert!(result.contains("Today's stuff"));
        assert!(!result.contains("Old stuff"));

        // Recall with 2 days should get both
        let result = memory.recall("agent-1", 2).unwrap();
        assert!(result.contains("Today's stuff"));
        assert!(result.contains("Old stuff"));
    }

    #[test]
    fn append_daily_creates_file() {
        let (_temp, memory) = setup();
        let path = memory
            .append_daily("agent-1", "Learned something new")
            .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Learned something new"));
    }

    #[test]
    fn append_daily_appends_to_existing() {
        let (_temp, memory) = setup();

        memory.append_daily("agent-1", "First entry").unwrap();
        memory.append_daily("agent-1", "Second entry").unwrap();

        let daily_dir = memory.workspace.join("agents/agent-1/daily");
        let today = Local::now().format("%Y-%m-%d");
        let path = daily_dir.join(format!("{}.md", today));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("First entry"));
        assert!(content.contains("Second entry"));
    }

    #[test]
    fn write_memory_creates_file() {
        let (_temp, memory) = setup();
        let path = memory
            .write_memory("agent-1", "# My Memory\n\nImportant stuff")
            .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# My Memory\n\nImportant stuff");
    }

    #[test]
    fn write_memory_overwrites_existing() {
        let (_temp, memory) = setup();

        memory.write_memory("agent-1", "Old content").unwrap();
        memory.write_memory("agent-1", "New content").unwrap();

        let path = memory.workspace.join("agents/agent-1/MEMORY.md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "New content");
    }

    #[test]
    fn write_world_creates_file() {
        let (_temp, memory) = setup();
        let path = memory
            .write_world("people", "# People\n\n## Alice\n- Role: Engineer")
            .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# People"));
        assert!(content.contains("## Alice"));
    }

    #[test]
    fn write_world_overwrites_existing() {
        let (_temp, memory) = setup();

        memory.write_world("people", "Old content").unwrap();
        memory.write_world("people", "New content").unwrap();

        let path = memory.workspace.join("world/people.md");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "New content");
    }

    #[test]
    fn write_world_sanitizes_topic() {
        let (_temp, memory) = setup();
        let path = memory.write_world("My Topic!", "Some content").unwrap();

        assert!(path.file_name().unwrap().to_str().unwrap() == "my-topic-.md");
    }
}
