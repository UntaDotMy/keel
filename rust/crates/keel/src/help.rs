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
    writeln!(output_writer, "keel")?;
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
        "  Use this when you need memory or anvil internals instead of the default operator path."
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
/// Help lines look like `  memory scope resolve [--workspace-root <path>] [--json]`.
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
    use crate::utility::run_memory_command;

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
                "memory" => {
                    run_memory_command(group.as_str(), &subcommand_args, &mut stdout, &mut stderr)
                }
                "anvil" => {
                    crate::utility::run_anvil_command(&subcommand_args, &mut stdout, &mut stderr)
                }
                other => panic!("help line uses unknown top-level group: {other:?}"),
            };

            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
            let stderr_text = String::from_utf8_lossy(&stderr);
            assert!(
                !stderr_text.contains("not implemented"),
                "advertised command is a phantom (dispatcher reports 'not implemented'): {raw_line:?}\noutput: {combined}"
            );
            assert!(
                !stderr_text.contains("Unknown"),
                "advertised command does not route to a known dispatcher arm: {raw_line:?}\noutput: {combined}"
            );
        }
    }

    /// Same guarantee as `every_advertised_advanced_command_routes_to_real_dispatcher`,
    /// but for the operator-tier help surface in `help_operator.txt`.
    ///
    /// The operator file mixes top-level commands (help, version, install, ...) with
    /// group commands (memory). Top-level commands
    /// are matched in `commands.rs` directly and don't have the phantom-subcommand
    /// failure mode this test guards against, so we only inspect lines whose first
    /// token is a group dispatcher. Group-command lines may use pipe-separated
    /// alternations (e.g. `memory working-brief|completion-gate`); each alternative is tested
    /// independently.
    #[test]
    fn every_advertised_operator_command_routes_to_real_dispatcher() {
        for raw_line in OPERATOR_HELP_COMMAND_LINES.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let tokens = parse_command_tokens(raw_line);
            if tokens.is_empty() {
                continue;
            }
            let group = tokens[0].clone();
            assert!(
                crate::commands::TOP_LEVEL_COMMANDS.contains(&group.as_str()),
                "help_operator.txt advertises `{group}`, which is not dispatched by commands.rs"
            );
            if group != "memory" {
                continue;
            }
            assert!(
                tokens.len() >= 2,
                "group help line must advertise at least one subcommand: {raw_line:?}"
            );

            // The second token may be a pipe-alternation list. Expand it so each
            // alternative is checked. Any tokens after the second are kept verbatim
            // (e.g. `memory scope resolve` -> ["scope", "resolve"]).
            let alternation = tokens[1].split('|').map(|s| s.to_string());
            let trailing: Vec<String> = tokens[2..].to_vec();

            for first_subcommand in alternation {
                let mut subcommand_args: Vec<String> = vec![first_subcommand];
                subcommand_args.extend(trailing.iter().cloned());

                let mut stdout: Vec<u8> = Vec::new();
                let mut stderr: Vec<u8> = Vec::new();
                let _ = match group.as_str() {
                    "memory" => run_memory_command(
                        group.as_str(),
                        &subcommand_args,
                        &mut stdout,
                        &mut stderr,
                    ),
                    _ => unreachable!(),
                };

                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
                assert!(
                    !combined.contains("not implemented"),
                    "advertised command is a phantom (dispatcher reports 'not implemented'): {raw_line:?} arg={subcommand_args:?}\noutput: {combined}"
                );
                assert!(
                    !combined.contains("Unknown"),
                    "advertised command does not route to a known dispatcher arm: {raw_line:?} arg={subcommand_args:?}\noutput: {combined}"
                );
            }
        }
    }

    #[test]
    fn parse_command_tokens_extracts_leading_command_path() {
        assert_eq!(
            parse_command_tokens("  memory scope resolve [--workspace-root <path>] [--json]"),
            vec!["memory", "scope", "resolve"]
        );
    }

    /// H9 inverse guard: every top-level command dispatched in commands.rs must
    /// appear in one of the help surfaces. The forward tests above check
    /// advertised -> routes; this checks the inverse (routes -> advertised) so a
    /// dispatched-but-undocumented command cannot silently disappear from help
    /// again.
    #[test]
    fn every_dispatched_top_level_command_is_advertised() {
        // Single-sourced from commands.rs so a new dispatch arm cannot ship
        // unadvertised (the blind spot that hid checkpoint/code-index).
        let dispatched = crate::commands::TOP_LEVEL_COMMANDS;
        let combined_help = format!(
            "{}\n{}",
            OPERATOR_HELP_COMMAND_LINES, ADVANCED_HELP_COMMAND_LINES
        );
        for command in dispatched.iter().copied() {
            // A command is "advertised" if it appears as the first token of some
            // help line (so `hook` matches `  hook install|...` but not a flag).
            let advertised = combined_help.lines().any(|line| {
                let tokens = parse_command_tokens(line);
                tokens.first().is_some_and(|first| first == command)
            });
            assert!(
                advertised,
                "dispatched top-level command `{command}` is missing from both help surfaces. \
                 Add it to help_operator.txt (or help_advanced.txt for memory/anvil internals)."
            );
        }
    }

    /// H9 guard: the hook admin verbs in the static help_operator.txt line must
    /// match the verbs render_hook_help advertises (the dynamic help), so the two
    /// hook help surfaces cannot drift. Catches the missing diagnose|git-hooks.
    #[test]
    fn help_operator_advertises_every_hook_admin_verb() {
        // The hook verbs are advertised in a single pipe-separated line in
        // help_operator.txt. Extract them and assert the known admin verbs appear.
        let hook_line = OPERATOR_HELP_COMMAND_LINES
            .lines()
            .find(|line| {
                let tokens = parse_command_tokens(line);
                tokens.first().is_some_and(|first| first == "hook")
            })
            .expect("help_operator.txt must have a `hook` command line");
        for verb in [
            "install",
            "uninstall",
            "list",
            "show",
            "instructions",
            "diagnose",
            "git-hooks",
        ] {
            assert!(
                hook_line.contains(verb),
                "help_operator.txt hook line must advertise the `{verb}` verb; got: {hook_line}"
            );
        }
    }
}
