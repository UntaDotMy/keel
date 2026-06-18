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
                            "{}: flag provided but not defined: -{flag_name}",
                            self.name
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
