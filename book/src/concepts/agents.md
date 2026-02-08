# Agents

An agent in Duragent is defined by a set of files — YAML for structure, Markdown for prose. No code required.

## Agent Directory

Each agent lives in its own directory under `.duragent/agents/`:

```
.duragent/agents/my-assistant/
├── agent.yaml           # Agent definition (required)
├── SOUL.md              # "Who the agent IS" (identity and personality)
├── SYSTEM_PROMPT.md     # "What the agent DOES" (core system prompt)
├── INSTRUCTIONS.md      # Additional runtime instructions (optional)
├── policy.yaml          # Tool execution policy (optional)
├── skills/              # Skill definitions (optional)
│   └── task-extraction/
│       └── SKILL.md
├── memory/              # Agent's long-term memory
│   ├── MEMORY.md
│   └── daily/
└── tools/               # CLI tools for this agent (optional)
    └── my-tool.sh
```

## Minimal Agent

The simplest agent needs just two files:

```yaml
# agent.yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: simple-assistant
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./SYSTEM_PROMPT.md
```

```markdown
<!-- SYSTEM_PROMPT.md -->
You are a helpful assistant.
```

## Prompt Files

Duragent separates agent prompts into three optional files:

| File | Purpose | When Loaded |
|------|---------|-------------|
| `SOUL.md` | "Who the agent IS" — identity and personality | Every turn |
| `SYSTEM_PROMPT.md` | "What the agent DOES" — core system prompt | Every turn |
| `INSTRUCTIONS.md` | Additional runtime instructions | Every turn |

**Best practice:** Keep `SYSTEM_PROMPT.md` minimal. Put detailed behavioral rules in `INSTRUCTIONS.md` and situational context in skills (loaded on demand).

## Agent Loading

Duragent supports two loading modes:

| Mode | Source | Use Case |
|------|--------|----------|
| **Static** | Files in `.duragent/agents/` | GitOps, version-controlled agents |
| **Dynamic** | Admin API | API-deployed, multi-tenant |

Agents are lazily loaded with an LRU cache — you can have thousands of agent definitions without memory overhead.

## Agent Routing

When a message arrives from a gateway, Duragent uses routing rules to determine which agent handles it. See [Configuration > Routes](../reference/configuration.md) for details.

For more on agent definition, see the [Agent Format](../guides/agent-format.md) guide.
