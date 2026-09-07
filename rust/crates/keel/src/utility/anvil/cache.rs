use serde_json::Value;
use std::env;

/// Resolve the provider label supplied by the host adapter.  The default is
/// intentionally `host-cli`; it identifies the measurement owner without
/// pretending Anvil knows which vendor is behind that executable.
pub fn provider_from_env() -> String {
    ["KEEL_ANVIL_PROVIDER", "ANVIL_PROVIDER"]
        .iter()
        .find_map(|name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "host-cli".into())
}

/// Usage that a host CLI explicitly emitted in machine-readable output.
///
/// Every field is optional because host CLIs do not share one usage schema and
/// Anvil must never turn a missing provider measurement into a fabricated zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

impl ProviderUsage {
    pub fn merge(&mut self, other: &Self) {
        merge_optional(&mut self.input_tokens, other.input_tokens);
        merge_optional(&mut self.output_tokens, other.output_tokens);
        merge_optional(
            &mut self.cache_read_input_tokens,
            other.cache_read_input_tokens,
        );
        merge_optional(
            &mut self.cache_creation_input_tokens,
            other.cache_creation_input_tokens,
        );
    }

    pub fn total_tokens(&self) -> Option<u64> {
        match (self.input_tokens, self.output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            (Some(input), None) | (None, Some(input)) => Some(input),
            (None, None) => None,
        }
    }

    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let read = self.cache_read_input_tokens?;
        let created = self.cache_creation_input_tokens?;
        let total = read.saturating_add(created);
        (total > 0).then_some(read as f64 / total as f64)
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cache_read_input_tokens": self.cache_read_input_tokens,
            "cache_creation_input_tokens": self.cache_creation_input_tokens,
        })
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let usage = Self {
            input_tokens: number(object, &["input_tokens", "prompt_tokens", "inputTokens"]),
            output_tokens: number(
                object,
                &["output_tokens", "completion_tokens", "outputTokens"],
            ),
            cache_read_input_tokens: number(
                object,
                &[
                    "cache_read_input_tokens",
                    "cache_read_tokens",
                    "cached_input_tokens",
                    "cacheReadInputTokens",
                ],
            ),
            cache_creation_input_tokens: number(
                object,
                &[
                    "cache_creation_input_tokens",
                    "cache_creation_tokens",
                    "cache_write_input_tokens",
                    "cacheCreationInputTokens",
                ],
            ),
        };
        usage.has_measurement().then_some(usage)
    }

    fn has_measurement(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
    }
}

/// Extract a provider usage object from newline-delimited or nested JSON host
/// output.  Plain text output is deliberately treated as unavailable.
pub fn usage_from_host_output(stdout: &[u8], stderr: &[u8]) -> Option<ProviderUsage> {
    let mut found = ProviderUsage::default();
    let mut measured = false;
    for stream in [stdout, stderr] {
        if let Ok(value) = serde_json::from_slice::<Value>(stream) {
            if let Some(usage) = find_usage(&value) {
                found.merge(&usage);
                measured = true;
            }
            continue;
        }
        let text = String::from_utf8_lossy(stream);
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if let Some(usage) = find_usage(&value) {
                found.merge(&usage);
                measured = true;
            }
        }
    }
    measured.then_some(found)
}

fn find_usage(value: &Value) -> Option<ProviderUsage> {
    if let Some(object) = value.as_object() {
        if let Some(usage) = ProviderUsage::from_json(value) {
            return Some(usage);
        }
        for child in object.values() {
            if let Some(usage) = find_usage(child) {
                return Some(usage);
            }
        }
    } else if let Some(values) = value.as_array() {
        for child in values {
            if let Some(usage) = find_usage(child) {
                return Some(usage);
            }
        }
    }
    None
}

fn number(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
}

fn merge_optional(target: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *target = Some(target.unwrap_or(0).saturating_add(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_host_output_does_not_claim_usage() {
        assert_eq!(usage_from_host_output(b"tokens: 20\n", b""), None);
    }

    #[test]
    fn parses_nested_provider_usage_and_cache_ratio() {
        let stdout = br#"{"response":{"usage":{"prompt_tokens":100,"completion_tokens":20,"cache_read_input_tokens":60,"cache_creation_input_tokens":40}}}"#;
        let usage = usage_from_host_output(stdout, b"").expect("usage");
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(60));
        assert_eq!(usage.cache_creation_input_tokens, Some(40));
        assert!((usage.cache_hit_ratio().expect("ratio") - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn provider_defaults_to_host_cli_without_claiming_vendor_identity() {
        assert!(!provider_from_env().is_empty());
    }
}
