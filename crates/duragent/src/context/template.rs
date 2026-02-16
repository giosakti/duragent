//! Template variable interpolation for agent spec content.
//!
//! Replaces `{{var}}` placeholders in soul, system_prompt, and instructions
//! with runtime values. Unknown variables are left as-is.

use chrono::Utc;

use crate::agent::AgentSpec;

/// Replace `{{var}}` placeholders with runtime values.
///
/// Supported variables:
/// - `{{date}}` — Current date (ISO 8601, e.g. `2026-02-16`)
/// - `{{time}}` — Current time (HH:MM UTC, e.g. `14:30 UTC`)
/// - `{{agent.name}}` — Agent metadata name
/// - `{{agent.home}}` — Agent directory path
///
/// Unknown variables like `{{foo}}` are left unchanged.
pub fn interpolate_template_vars(input: &str, agent: &AgentSpec) -> String {
    let now = Utc::now();
    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];

        if let Some(end) = after_open.find("}}") {
            let var_name = after_open[..end].trim();
            match var_name {
                "date" => result.push_str(&now.format("%Y-%m-%d").to_string()),
                "time" => result.push_str(&now.format("%H:%M UTC").to_string()),
                "agent.name" => result.push_str(&agent.metadata.name),
                "agent.home" => result.push_str(&agent.agent_dir.display().to_string()),
                _ => {
                    // Unknown variable — leave as-is
                    result.push_str("{{");
                    result.push_str(&after_open[..end]);
                    result.push_str("}}");
                }
            }
            rest = &after_open[end + 2..];
        } else {
            // No closing `}}` — emit the `{{` literally and move on
            result.push_str("{{");
            rest = after_open;
        }
    }

    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentMetadata, AgentSessionConfig, HooksConfig, ModelConfig, ToolPolicy};
    use crate::llm::Provider;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_agent() -> AgentSpec {
        AgentSpec {
            api_version: "duragent/v1alpha1".to_string(),
            kind: "Agent".to_string(),
            metadata: AgentMetadata {
                name: "iova".to_string(),
                description: None,
                version: None,
                labels: HashMap::new(),
            },
            model: ModelConfig {
                provider: Provider::Other("test".to_string()),
                name: "test-model".to_string(),
                base_url: None,
                temperature: None,
                max_input_tokens: None,
                max_output_tokens: None,
            },
            soul: None,
            system_prompt: None,
            instructions: None,
            skills: Vec::new(),
            session: AgentSessionConfig::default(),
            memory: None,
            tools: Vec::new(),
            policy: ToolPolicy::default(),
            hooks: HooksConfig::default(),
            access: None,
            agent_dir: PathBuf::from("/opt/agents/iova/.duragent/agents/iova"),
        }
    }

    #[test]
    fn interpolates_date() {
        let agent = test_agent();
        let result = interpolate_template_vars("Today is {{date}}.", &agent);
        let expected_date = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(result, format!("Today is {}.", expected_date));
    }

    #[test]
    fn interpolates_time() {
        let agent = test_agent();
        let result = interpolate_template_vars("Now: {{time}}", &agent);
        assert!(result.contains("UTC"));
        assert!(!result.contains("{{time}}"));
    }

    #[test]
    fn interpolates_agent_name() {
        let agent = test_agent();
        let result = interpolate_template_vars("I am {{agent.name}}.", &agent);
        assert_eq!(result, "I am iova.");
    }

    #[test]
    fn interpolates_agent_home() {
        let agent = test_agent();
        let result = interpolate_template_vars("Home: {{agent.home}}", &agent);
        assert_eq!(result, "Home: /opt/agents/iova/.duragent/agents/iova");
    }

    #[test]
    fn unknown_vars_left_as_is() {
        let agent = test_agent();
        let result = interpolate_template_vars("Hello {{unknown}}!", &agent);
        assert_eq!(result, "Hello {{unknown}}!");
    }

    #[test]
    fn no_vars_passthrough() {
        let agent = test_agent();
        let result = interpolate_template_vars("Plain text, no variables.", &agent);
        assert_eq!(result, "Plain text, no variables.");
    }

    #[test]
    fn multiple_vars_in_one_string() {
        let agent = test_agent();
        let result = interpolate_template_vars(
            "Agent {{agent.name}} at {{agent.home}} on {{date}}.",
            &agent,
        );
        let expected_date = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(
            result,
            format!(
                "Agent iova at /opt/agents/iova/.duragent/agents/iova on {}.",
                expected_date
            )
        );
    }
}
