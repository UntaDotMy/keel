//! Purpose: Operator and advanced help text rendering for the native CLI.
//! Caller: commands.rs `run_help_command` and the bare-invocation path in `Application::run`.
//! Dependencies: std::io::{self, Write}; embedded help_*.txt resources.
//! Main Functions: render_help_surface.
//! Side Effects: Writes embedded help text to the supplied writer..

use std::io::{self, Write};

static OPERATOR_HELP_COMMAND_LINES: &str = include_str!("help_operator.txt");
static ADVANCED_HELP_COMMAND_LINES: &str = include_str!("help_advanced.txt");
static OPERATOR_MIGRATION_STATE_LINES: &str = include_str!("help_operator_state.txt");
static ADVANCED_MIGRATION_STATE_LINES: &str = include_str!("help_advanced_state.txt");

pub fn render_help_surface<W: Write + ?Sized>(
    output_writer: &mut W,
    include_advanced: bool,
) -> io::Result<()> {
    writeln!(output_writer, "claude-skills")?;
    writeln!(output_writer)?;
    if include_advanced {
        writeln!(output_writer, "Help mode: advanced")?;
    } else {
        writeln!(output_writer, "Help mode: operator")?;
    }
    writeln!(output_writer)?;
    writeln!(output_writer, "Operator commands:")?;
    write_help_lines(output_writer, OPERATOR_HELP_COMMAND_LINES)?;
    writeln!(output_writer)?;
    writeln!(output_writer, "Advanced surfaces:")?;
    writeln!(output_writer, "  help advanced")?;
    writeln!(
        output_writer,
        "  Use this when you need orchestration, memory, or memoriesv2 internals instead of the default operator path."
    )?;
    if include_advanced {
        writeln!(output_writer)?;
        writeln!(output_writer, "Advanced commands:")?;
        write_help_lines(output_writer, ADVANCED_HELP_COMMAND_LINES)?;
    }
    writeln!(output_writer)?;
    writeln!(output_writer, "Current migration state:")?;
    write_help_lines(output_writer, OPERATOR_MIGRATION_STATE_LINES)?;
    if include_advanced {
        write_help_lines(output_writer, ADVANCED_MIGRATION_STATE_LINES)?;
    }
    Ok(())
}

fn write_help_lines<W: Write + ?Sized>(output_writer: &mut W, lines_body: &str) -> io::Result<()> {
    for line in lines_body.split_inclusive('\n') {
        output_writer.write_all(line.as_bytes())?;
    }
    Ok(())
}

/// Parse the leading command tokens from a help line.
///
/// Help lines look like `  orchestration runtime-preflight [--claude-home <path>] [--json]`.
/// The first run of non-flag, non-bracket tokens is the command name. We stop at the first
/// token that starts with `[`, `<`, or `--` — those are flag/argument descriptors.
#[cfg(test)]
fn parse_command_tokens(line: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for raw in line.split_whitespace() {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let first_byte = token.as_bytes()[0];
        if first_byte == b'[' || first_byte == b'<' || first_byte == b'(' || token.starts_with("--")
        {
            break;
        }
        tokens.push(token.to_string());
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utility::{run_memory_command, run_orchestration_command};

    /// Every command advertised in help_advanced.txt must route to a real dispatcher arm.
    ///
    /// "Real" means the dispatcher recognizes the command path: it does not print
    /// "not implemented" and it does not print "Unknown ... command".
    /// We invoke each command with empty further-args. Real handlers respond by
    /// running their own preflight (returning a missing-flag or "no repository root"
    /// message on stderr) or by printing usage. Phantom handlers respond with the
    /// literal strings "not implemented" or "Unknown" — those are the regressions we want to catch.
    #[test]
    fn every_advertised_advanced_command_routes_to_real_dispatcher() {
        for raw_line in ADVANCED_HELP_COMMAND_LINES.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let tokens = parse_command_tokens(raw_line);
            assert!(
                tokens.len() >= 2,
                "help line did not start with a recognizable command: {raw_line:?}"
            );
            let group = tokens[0].clone();
            let subcommand_args: Vec<String> = tokens[1..].to_vec();

            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let _ = match group.as_str() {
                "memory" | "memoriesv2" => {
                    run_memory_command(group.as_str(), &subcommand_args, &mut stdout, &mut stderr)
                }
                "orchestration" => {
                    run_orchestration_command(&subcommand_args, &mut stdout, &mut stderr)
                }
                other => panic!("help line uses unknown top-level group: {other:?}"),
            };

            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
            assert!(
                !combined.contains("not implemented"),
                "advertised command is a phantom (dispatcher reports 'not implemented'): {raw_line:?}\noutput: {combined}"
            );
            assert!(
                !combined.contains("Unknown"),
                "advertised command does not route to a known dispatcher arm: {raw_line:?}\noutput: {combined}"
            );
        }
    }

    #[test]
    fn parse_command_tokens_extracts_leading_command_path() {
        assert_eq!(
            parse_command_tokens("  memory scope resolve [--workspace-root <path>] [--json]"),
            vec!["memory", "scope", "resolve"]
        );
        assert_eq!(
            parse_command_tokens(
                "  orchestration runtime-preflight [--claude-home <path>] [--json]"
            ),
            vec!["orchestration", "runtime-preflight"]
        );
        assert_eq!(
            parse_command_tokens("  memoriesv2 working-brief [write|show|list] (note)"),
            vec!["memoriesv2", "working-brief"]
        );
    }
}
