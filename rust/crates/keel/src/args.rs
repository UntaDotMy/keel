//! Purpose: Minimal argument parser used by the Rust-native command surface.
//! Caller: Command handlers in commands.rs that need stable flag parsing without external dependencies.
//! Dependencies: std::collections::HashMap.
//! Main Functions: FlagSet::new, FlagSet::bool_flag, FlagSet::string_flag, FlagSet::parse, FlagSet::bool_value, FlagSet::string_value, FlagValue::as_bool, FlagValue::as_str.
//! Side Effects: None beyond returning parse results; this module is pure.
//!
//! Scope note: this parser covers the **success paths** needed for the Phase 1
//! commands. Malformed flag inputs intentionally use a concise Rust-native
//! diagnostic instead of preserving historical implementation-specific wording.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum FlagSpec {
    Bool,
    String,
}

#[derive(Debug, Clone)]
pub enum FlagValue {
    Bool(bool),
    String(String),
}

impl FlagValue {
    pub fn as_bool(&self) -> bool {
        match self {
            FlagValue::Bool(value) => *value,
            FlagValue::String(_) => false,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            FlagValue::String(value) => value.as_str(),
            FlagValue::Bool(_) => "",
        }
    }
}

pub struct FlagSet {
    pub name: String,
    pub specs: HashMap<String, FlagSpec>,
    pub values: HashMap<String, FlagValue>,
    pub positional: Vec<String>,
}

#[derive(Debug)]
pub struct FlagError {
    pub message: String,
}

impl FlagSet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            specs: HashMap::new(),
            values: HashMap::new(),
            positional: Vec::new(),
        }
    }

    pub fn bool_flag(&mut self, flag_name: &str, default_value: bool) {
        self.specs.insert(flag_name.to_string(), FlagSpec::Bool);
        self.values
            .insert(flag_name.to_string(), FlagValue::Bool(default_value));
    }

    pub fn string_flag(&mut self, flag_name: &str, default_value: impl Into<String>) {
        let default_value = default_value.into();
        self.specs.insert(flag_name.to_string(), FlagSpec::String);
        self.values
            .insert(flag_name.to_string(), FlagValue::String(default_value));
    }

    /// Comma-separated `--flag` list of every registered flag, for unknown-flag
    /// diagnostics. Sorted so the message is stable across runs. Empty when the
    /// command takes no flags.
    fn accepted_flags_help(&self) -> String {
        let mut names: Vec<&str> = self.specs.keys().map(String::as_str).collect();
        names.sort_unstable();
        if names.is_empty() {
            return "(none)".to_string();
        }
        names
            .iter()
            .map(|name| format!("--{name}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn parse(&mut self, arguments: &[String]) -> Result<(), FlagError> {
        let mut index = 0;
        while index < arguments.len() {
            let token = &arguments[index];
            if token == "--" {
                self.positional
                    .extend(arguments[index + 1..].iter().cloned());
                return Ok(());
            }
            if !token.starts_with('-') || token == "-" {
                self.positional.extend(arguments[index..].iter().cloned());
                return Ok(());
            }
            let stripped = token.trim_start_matches('-');
            let (flag_name, inline_value) = match stripped.split_once('=') {
                Some((flag_name, flag_value)) => {
                    (flag_name.to_string(), Some(flag_value.to_string()))
                }
                None => (stripped.to_string(), None),
            };
            let spec = match self.specs.get(&flag_name) {
                Some(spec) => spec.clone(),
                None => {
                    return Err(FlagError {
                        message: format!(
                            "{}: flag provided but not defined: -{flag_name}. Accepted flags: {}",
                            self.name,
                            self.accepted_flags_help()
                        ),
                    });
                }
            };
            match spec {
                FlagSpec::Bool => {
                    let bool_value = match inline_value {
                        Some(raw_value) => parse_bool(&raw_value).ok_or_else(|| FlagError {
                            message: format!(
                                "{}: invalid boolean value {raw_value:?} for -{flag_name}",
                                self.name
                            ),
                        })?,
                        None => true,
                    };
                    self.values.insert(flag_name, FlagValue::Bool(bool_value));
                    index += 1;
                }
                FlagSpec::String => {
                    let string_value = match inline_value {
                        Some(raw_value) => {
                            index += 1;
                            raw_value
                        }
                        None => {
                            if index + 1 >= arguments.len() {
                                return Err(FlagError {
                                    message: format!(
                                        "{}: flag needs an argument: -{flag_name}",
                                        self.name
                                    ),
                                });
                            }
                            let taken_value = arguments[index + 1].clone();
                            index += 2;
                            taken_value
                        }
                    };
                    self.values
                        .insert(flag_name, FlagValue::String(string_value));
                }
            }
        }
        Ok(())
    }

    pub fn bool_value(&self, flag_name: &str) -> bool {
        self.values
            .get(flag_name)
            .map(FlagValue::as_bool)
            .unwrap_or(false)
    }

    pub fn string_value(&self, flag_name: &str) -> &str {
        self.values
            .get(flag_name)
            .map(FlagValue::as_str)
            .unwrap_or("")
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flag_error_lists_accepted_flags() {
        // Regression: `keel memory consolidate --repo-root X` failed with a
        // cryptic "flag provided but not defined: -repo-root" and no hint about
        // what IS accepted, forcing the agent to guess across 3 attempts. The
        // error must now name the accepted flags so the caller self-corrects.
        let mut flags = FlagSet::new("memory consolidate");
        flags.string_flag("claude-home", "");
        flags.bool_flag("json", false);
        let error = flags
            .parse(&["--repo-root".to_string(), "x".to_string()])
            .expect_err("unknown flag should error");
        assert!(
            error.message.contains("not defined: -repo-root"),
            "names the rejected flag: {}",
            error.message
        );
        assert!(
            error.message.contains("--claude-home") && error.message.contains("--json"),
            "lists accepted flags: {}",
            error.message
        );
    }

    #[test]
    fn unknown_flag_error_reports_none_when_no_flags_registered() {
        let mut flags = FlagSet::new("bare command");
        let error = flags
            .parse(&["--bogus".to_string()])
            .expect_err("unknown flag should error");
        assert!(
            error.message.contains("(none)"),
            "empty flag set reports (none): {}",
            error.message
        );
    }

    #[test]
    fn accepted_flags_help_is_sorted_and_dashed() {
        let mut flags = FlagSet::new("demo");
        flags.bool_flag("zebra", false);
        flags.string_flag("alpha", "");
        flags.string_flag("mid", "");
        let help = flags.accepted_flags_help();
        assert_eq!(help, "--alpha, --mid, --zebra");
    }
}
