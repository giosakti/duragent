# Writing Skills

Skills are reusable behaviors that teach agents how to accomplish tasks. Unlike tools (which provide capabilities), skills provide instructions and patterns.

## Skills vs Tools

| Concept | Purpose | Format |
|---------|---------|--------|
| **Tool** | A capability the agent can invoke | CLI command or built-in (MCP planned) |
| **Skill** | Instructions for how to accomplish a task | `SKILL.md` with steps, triggers, examples |

Skills may *use* tools, but they're primarily about teaching patterns.

## Directory Structure

```
skills/
└── task-extraction/
    ├── SKILL.md              # Required: skill definition
    ├── scripts/
    │   └── extract.py        # Optional: executable scripts
    ├── references/
    │   └── examples.md       # Optional: loaded into context
    └── assets/
        └── template.txt      # Optional: templates
```

## SKILL.md Format

Duragent adopts the [Agent Skills](https://agentskills.io) open standard, ensuring portability across ecosystems.

### Structure

- YAML frontmatter between `---` delimiters
- Required fields: `name`, `description`
- Optional fields: `allowed-tools`, `metadata`
- Body content: instructions, examples, output format

### Example

```markdown
---
name: task-extraction
description: Extract actionable tasks from a message or conversation
allowed-tools: calculator
metadata:
  version: "1.0.0"
  author: Duragent
---

## Instructions

1. Find explicit action items and implied commitments.
2. Present tasks in a consistent JSON shape.

## Output Format

Return tasks as JSON:

\`\`\`json
{
  "tasks": [
    {
      "description": "Review PR #123",
      "assignee": "alice",
      "priority": "high"
    }
  ]
}
\`\`\`
```

### Naming Rules

- Lowercase letters and hyphens only
- Maximum 64 characters
- Should match the directory name

## Configuring Skills

Point your agent to a skills directory:

```yaml
# agent.yaml
spec:
  skills_dir: ./skills/
```

Duragent scans the directory for subdirectories containing `SKILL.md` files. Skill IDs are derived from the `name` field in the SKILL.md frontmatter, or the directory name if not specified.

## Best Practices

- **Keep skills focused** — one skill per task type
- **Include examples** — put reference material in `references/` for the agent to load
- **Use allowed-tools** — declare which tools the skill expects to use
- **Test with real conversations** — verify the skill produces the expected output
