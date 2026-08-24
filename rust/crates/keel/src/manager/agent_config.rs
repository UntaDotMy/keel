//! Purpose: Agent config parsing and TOML rendering for keel manager.
//! Caller: install.rs via sync_agents.
//! Dependencies: std::fs, std::time, crate::runtime.
//! Main Functions: parse_agent_config, extract_quoted_yaml_value, extract_yaml_bool, decode_basic_json_string, render_agent_toml, escape_toml_string, unix_timestamp.
//! Side Effects: Reads agent config YAML files, renders TOML agent profiles.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runtime::{display_path, AgentConfig};

#[derive(Default)]
pub struct ParsedAgentConfig {
    pub display_name: String,
    pub reasoning_effort: String,
    pub short_description: String,
    pub allow_implicit_invocation: bool,
    pub default_prompt: String,
}

pub fn parse_agent_config(agent_config: &AgentConfig) -> Result<ParsedAgentConfig, String> {
    let text = fs::read_to_string(&agent_config.config_path)
        .map_err(|error| format!("read {}: {error}", display_path(&agent_config.config_path)))?;
    Ok(ParsedAgentConfig {
        display_name: extract_quoted_yaml_value(&text, "display_name").unwrap_or_default(),
        reasoning_effort: extract_quoted_yaml_value(&text, "reasoning_effort")
            .unwrap_or_else(|| "high".to_string()),
        short_description: extract_quoted_yaml_value(&text, "short_description")
            .unwrap_or_default(),
        allow_implicit_invocation: extract_yaml_bool(&text, "allow_implicit_invocation")
            .unwrap_or(false),
        default_prompt: extract_quoted_yaml_value(&text, "default_prompt").ok_or_else(|| {
            format!(
                "missing default_prompt in {}",
                display_path(&agent_config.config_path)
            )
        })?,
    })
}

pub fn extract_quoted_yaml_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with(&prefix) {
            continue;
        }
        let value = trimmed[prefix.len()..].trim();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            return Some(decode_basic_json_string(&value[1..value.len() - 1]));
        }
        if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
            return Some(value[1..value.len() - 1].to_string());
        }
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}
pub fn extract_yaml_bool(text: &str, key: &str) -> Option<bool> {
    match extract_quoted_yaml_value(text, key)?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn decode_basic_json_string(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

pub fn render_agent_toml(config: &ParsedAgentConfig, agent_name: &str) -> Result<String, String> {
    if config.default_prompt.contains("'''") {
        return Err(format!(
            "triple single quotes are not supported inside developer_instructions for {agent_name}"
        ));
    }
    let mut lines = Vec::new();
    lines.push(format!("name = \"{}\"", escape_toml_string(agent_name)));
    if !config.display_name.trim().is_empty() {
        lines.push(format!(
            "display_name = \"{}\"",
            escape_toml_string(&config.display_name)
        ));
    }
    lines.push(format!(
        "model_reasoning_effort = \"{}\"",
        escape_toml_string(&config.reasoning_effort)
    ));
    if !config.short_description.trim().is_empty() {
        lines.push(format!(
            "description = \"{}\"",
            escape_toml_string(&config.short_description)
        ));
    }
    lines.push(format!(
        "allow_implicit_invocation = {}",
        config.allow_implicit_invocation
    ));
    lines.push("developer_instructions = '''".to_string());
    lines.push(config.default_prompt.clone());
    lines.push("'''".to_string());
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
