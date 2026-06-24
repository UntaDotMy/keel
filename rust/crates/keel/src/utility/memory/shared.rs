//! Purpose: Shared helpers used across memory submodules
//! Caller: scope, system_map_cmd, working_brief_cmd, completion_gate, orchestration, workflow
//! Dependencies: crate::json
//! Main Functions: is_help_argument, render_workflow_json, probe_marker, probe_value
//! Side Effects: None, pure helpers

use std::io::Write;

use crate::json::{write_indented, Value};

pub(super) fn is_help_argument(argument: &str) -> bool {
    argument == "--help" || argument == "-h" || argument == "help"
}

pub(super) fn render_workflow_json(
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
    value: &Value,
) -> u8 {
    if let Err(write_error) = write_indented(standard_output, value) {
        let _ = writeln!(
            standard_error,
            "Unable to render workflow JSON output: {write_error}"
        );
        return 1;
    }
    0
}

pub(super) fn probe_marker<T, E>(probe: &Result<T, E>) -> &'static str {
    if probe.is_ok() {
        "ok"
    } else {
        "fail"
    }
}

pub(super) fn probe_value<T, E>(probe: &Result<T, E>, status: &str) -> Value {
    Value::Object(vec![
        ("ok".into(), Value::Bool(probe.is_ok())),
        ("detail".into(), Value::String(status.to_string())),
    ])
}
