//! Simple, human-readable agent memory system.
//!
//! Memory is stored as plain markdown files:
//! - `{world_dir}/*.md` — Shared facts (all agents see)
//! - `{agent_memory_dir}/MEMORY.md` — Agent's curated long-term memory
//! - `{agent_memory_dir}/daily/*.md` — Agent's daily experiences (append-only)

mod tools;

pub use tools::{RecallTool, ReflectTool, RememberTool, UpdateWorldTool};

use std::path::PathBuf;

/// Default instruction for memory tool access.
pub const DEFAULT_MEMORY_DIRECTIVE: &str = r#"You have access to a persistent memory system:
- `recall` - Load your memory (world knowledge, long-term memory, recent experiences)
- `remember` - Record an experience to today's daily log
- `reflect` - Read then update your long-term MEMORY.md (call without content to read, then with content to write)
- `update_world` - Write shared world knowledge for a topic (replaces existing content)

Guidelines:
- Call `recall` at the start of conversations when context might be helpful
- Use `remember` after learning something important about the user or project
- Use `reflect` at the end of substantial sessions to consolidate learnings — always read first, then write
- Use `update_world` when discovering facts relevant to all agents"#;

use anyhow::Result;
use chrono::Local;

/// Memory system for reading and writing agent memories.
#[derive(Debug, Clone)]
pub struct Memory {
    /// Shared world knowledge directory (contains `*.md` files directly).
    world_dir: PathBuf,
    /// Agent's memory directory, e.g. `agents/{name}/memory` (contains `MEMORY.md` and `daily/`).
    agent_memory_dir: PathBuf,
}

impl Memory {
    /// Create a new memory instance.
    ///
    /// - `world_dir`: shared world knowledge directory (e.g. `.agnx/memory/world`)
    /// - `agent_memory_dir`: agent's memory directory (e.g. `agents/{name}/memory`)
    pub fn new(world_dir: PathBuf, agent_memory_dir: PathBuf) -> Self {
        Self {
            world_dir,
            agent_memory_dir,
        }
    }

    /// Read memory context (called by recall tool).
    ///
    /// Returns concatenated content from:
    /// 1. World knowledge (`world_dir/*.md`)
    /// 2. Agent's curated memory (`agent_memory_dir/MEMORY.md`)
    /// 3. Recent daily experiences (`agent_memory_dir/daily/*.md`)
    pub fn recall(&self, days: usize) -> Result<String> {
        let mut sections = vec![];

        // World knowledge
        let world = self.read_world_facts()?;
        if !world.is_empty() {
            sections.push(format!("# World Knowledge\n\n{}", world));
        }

        // Agent's curated memory
        let memory_path = self.agent_memory_dir.join("MEMORY.md");
        if let Ok(memory) = std::fs::read_to_string(&memory_path)
            && !memory.is_empty()
        {
            sections.push(format!("# Your Memory\n\n{}", memory));
        }

        // Recent daily experiences
        let recent = self.read_recent_daily(days)?;
        if !recent.is_empty() {
            sections.push(format!("# Recent Experiences\n\n{}", recent));
        }

        Ok(sections.join("\n\n---\n\n"))
    }

    /// Read all world/*.md files.
    fn read_world_facts(&self) -> Result<String> {
        if !self.world_dir.exists() {
            return Ok(String::new());
        }

        let mut entries: Vec<_> = std::fs::read_dir(&self.world_dir)?
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
    fn read_recent_daily(&self, days: usize) -> Result<String> {
        let daily_dir = self.agent_memory_dir.join("daily");

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
    pub fn append_daily(&self, content: &str) -> Result<PathBuf> {
        let daily_dir = self.agent_memory_dir.join("daily");
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

    /// Read agent's MEMORY.md content (empty string if not found).
    pub fn read_memory(&self) -> Result<String> {
        let path = self.agent_memory_dir.join("MEMORY.md");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Rewrite agent's MEMORY.md.
    pub fn write_memory(&self, content: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.agent_memory_dir)?;

        let path = self.agent_memory_dir.join("MEMORY.md");
        let temp = self.agent_memory_dir.join("MEMORY.md.tmp");

        // Atomic write
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &path)?;

        Ok(path)
    }

    /// Write world knowledge for a topic (replaces existing content).
    pub fn write_world(&self, topic: &str, content: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.world_dir)?;

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
        let path = self.world_dir.join(&filename);

        // Atomic write
        let temp = self.world_dir.join(format!("{}.md.tmp", safe_topic));
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
        let world_dir = temp.path().join("world");
        let agent_memory_dir = temp.path().join("agent-memory");
        let memory = Memory::new(world_dir, agent_memory_dir);
        (temp, memory)
    }

    #[test]
    fn recall_empty_workspace() {
        let (_temp, memory) = setup();
        let result = memory.recall(3).unwrap();
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
            .write_memory("User prefers concise responses")
            .unwrap();

        // Add daily experience
        memory.append_daily("Helped debug issue").unwrap();

        let result = memory.recall(3).unwrap();

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
        let (_temp, memory) = setup();

        // Create a daily file for "yesterday" manually
        let daily_dir = memory.agent_memory_dir.join("daily");
        std::fs::create_dir_all(&daily_dir).unwrap();

        let yesterday = (Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d");
        let yesterday_file = daily_dir.join(format!("{}.md", yesterday));
        std::fs::write(&yesterday_file, "# Yesterday\n\nOld stuff").unwrap();

        // Create today's file
        memory.append_daily("Today's stuff").unwrap();

        // Recall with 1 day should only get today
        let result = memory.recall(1).unwrap();
        assert!(result.contains("Today's stuff"));
        assert!(!result.contains("Old stuff"));

        // Recall with 2 days should get both
        let result = memory.recall(2).unwrap();
        assert!(result.contains("Today's stuff"));
        assert!(result.contains("Old stuff"));
    }

    #[test]
    fn append_daily_creates_file() {
        let (_temp, memory) = setup();
        let path = memory.append_daily("Learned something new").unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Learned something new"));
    }

    #[test]
    fn append_daily_appends_to_existing() {
        let (_temp, memory) = setup();

        memory.append_daily("First entry").unwrap();
        memory.append_daily("Second entry").unwrap();

        let daily_dir = memory.agent_memory_dir.join("daily");
        let today = Local::now().format("%Y-%m-%d");
        let path = daily_dir.join(format!("{}.md", today));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("First entry"));
        assert!(content.contains("Second entry"));
    }

    #[test]
    fn read_memory_empty() {
        let (_temp, memory) = setup();
        let content = memory.read_memory().unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn read_memory_existing() {
        let (_temp, memory) = setup();
        memory.write_memory("Some notes").unwrap();
        let content = memory.read_memory().unwrap();
        assert_eq!(content, "Some notes");
    }

    #[test]
    fn write_memory_creates_file() {
        let (_temp, memory) = setup();
        let path = memory
            .write_memory("# My Memory\n\nImportant stuff")
            .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# My Memory\n\nImportant stuff");
    }

    #[test]
    fn write_memory_overwrites_existing() {
        let (_temp, memory) = setup();

        memory.write_memory("Old content").unwrap();
        memory.write_memory("New content").unwrap();

        let path = memory.agent_memory_dir.join("MEMORY.md");
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

        let path = memory.world_dir.join("people.md");
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
