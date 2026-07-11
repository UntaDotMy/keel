use super::*;
use std::path::PathBuf;

#[test]
fn hook_payload_preserves_unrelated_events_and_replaces_managed_hook() {
    let hook_path = temp_hook_path("keel-hook-payload");

    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

    std::fs::write(
        &hook_path,
        r#"{

  "hooks": {

    "PostToolUse": [

      {

        "matcher": "Write|Edit",

        "hooks": [

          {

            "type": "command",

            "command": "./scripts/post_write_figma_parity_check.sh"

          }

        ]

      }

    ],

    "PreToolUse": [

      {

        "matcher": "Bash",

        "hooks": [

          {

            "type": "command",

            "command": "keel hook instructions --format json"

          }

        ]

      }

    ]

  }

}

"#,
    )
    .unwrap();

    let rendered = build_hooks_payload(&hook_path, Path::new(r"C:\tools\keel.exe")).unwrap();

    assert!(rendered.contains("PostToolUse"));

    assert!(rendered.contains("PermissionRequest"));

    assert!(rendered.contains("Notification"));

    assert!(rendered.contains("PreCompact"));

    assert!(rendered.contains("PostCompact"));

    assert!(rendered.contains("SessionStart"));

    assert!(rendered.contains("SessionEnd"));

    assert!(rendered.contains("UserPromptSubmit"));

    assert!(rendered.contains("SubagentStop"));

    assert!(rendered.contains("Stop"));

    assert!(rendered.contains("post_write_figma_parity_check"));

    // Args-form: each managed entry now carries the slug in `args`, not in
    // the command string. The legacy single-string entry that lived in the
    // fixture's PreToolUse stanza must be gone (replaced by our managed
    // args-form entry).
    assert!(rendered.contains("\"pre-tool-use\""));

    assert!(!rendered.contains("hook instructions --format json"));

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

#[test]
fn hook_payload_uses_exact_managed_commands_for_each_event() {
    let hook_path = temp_hook_path("keel-hook-command-prefix");

    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

    std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

    let executable = std::env::current_exe().unwrap();

    let rendered = build_hooks_payload(&hook_path, &executable).unwrap();

    let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

    let hooks = document
        .get("hooks")
        .and_then(JsonDocument::as_object)
        .unwrap();

    let expected_command = display_path(&executable);

    for event in HOOK_EVENTS {
        if !event.installs_in_settings {
            // FileChanged (and any future opt-out) is not written to
            // settings.json by `keel hook install`, so the
            // payload won't contain a stanza for it.
            continue;
        }
        let entry = hooks
            .get(event.name)
            .and_then(JsonDocument::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("hooks"))
            .and_then(JsonDocument::as_array)
            .and_then(|commands| commands.first())
            .unwrap_or_else(|| panic!("missing hooks entry for {}", event.name));

        // CC 2.1.139 args exec form: command is the bare executable path,
        // args carries `["hook", <slug>]`, no shell wrapping.
        let command = entry
            .get("command")
            .and_then(JsonDocument::as_str)
            .unwrap_or_else(|| panic!("missing command for {}", event.name));
        assert_eq!(
            command, expected_command,
            "command must be the bare executable for {}",
            event.name
        );

        let args: Vec<&str> = entry
            .get("args")
            .and_then(JsonDocument::as_array)
            .unwrap_or_else(|| panic!("missing args for {}", event.name))
            .iter()
            .map(|value| value.as_str().expect("args entries are strings"))
            .collect();
        assert_eq!(
            args,
            vec!["hook", event.slug],
            "args must be [\"hook\", \"{}\"] for {}",
            event.slug,
            event.name
        );

        assert!(
            !command.contains("powershell"),
            "args form must not wrap the command in PowerShell for {}",
            event.name
        );
        assert!(
            !command.starts_with("& "),
            "args form must not use the PowerShell call operator for {}",
            event.name
        );
    }

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

#[test]
fn managed_hook_detection_recognizes_args_form_and_legacy_string_form() {
    let executable = if cfg!(windows) {
        Path::new(r"C:\Users\Example User\.claude\keel.exe")
    } else {
        Path::new("/home/example/.claude/keel")
    };

    // Args form: managed entry built by managed_hook_entry.
    let entry = managed_hook_entry(executable, "session-start");
    let args_form = serde_json::json!({
        "type": "command",
        "command": entry.command,
        "args": entry.args,
        "statusMessage": "test",
    });
    assert!(is_managed_hook_entry(&args_form));

    // Legacy single-string form (older keel versions): plain
    // string mentioning `keel` and a known slug. Detector must
    // still flag it so uninstall cleans up upgrades from older builds.
    let legacy_plain = serde_json::json!({
        "type": "command",
        "command": "keel hook session-start",
    });
    assert!(is_managed_hook_entry(&legacy_plain));

    // Legacy PowerShell-encoded form (Windows installs from older
    // keel versions). Hand-rolled snapshot of what the previous
    // encoder produced for `& 'keel' hook session-start` so we
    // don't depend on the deleted encoder. The base64 below decodes via
    // the still-present decode_powershell_encoded_command helper.
    let encoded_script = "& 'keel' hook session-start";
    let encoded_bytes: Vec<u8> = encoded_script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let encoded = base64_encode_for_test(&encoded_bytes);
    let legacy_encoded_command = format!("powershell.exe -NoProfile -EncodedCommand {encoded}");
    let legacy_encoded = serde_json::json!({
        "type": "command",
        "command": legacy_encoded_command,
    });
    assert!(is_managed_hook_entry(&legacy_encoded));

    // Unrelated entries must not be flagged.
    let unrelated = serde_json::json!({
        "type": "command",
        "command": "./scripts/format.sh",
    });
    assert!(!is_managed_hook_entry(&unrelated));

    let unrelated_encoded = serde_json::json!({
        "type": "command",
        "command": "powershell.exe -NoProfile -EncodedCommand SQBuAHYAYQBsAGkAZAA=",
    });
    assert!(!is_managed_hook_entry(&unrelated_encoded));

    // Args form with the right binary basename but a slug that isn't in
    // HOOK_EVENTS must be rejected, so a hand-rolled user entry for a
    // future or experimental subcommand isn't auto-removed by uninstall.
    let unknown_slug = serde_json::json!({
        "type": "command",
        "command": entry.command,
        "args": ["hook", "not-a-real-slug"],
    });
    assert!(!is_managed_hook_entry(&unknown_slug));
}

#[test]
fn install_then_uninstall_leaves_no_managed_hook_keys() {
    // Round-trip: building the full payload installs a stanza per
    // installable event; removing it must strip every key it added so the
    // hooks object returns to empty. Regression for the bug where empty
    // `"Stop": []` arrays were left behind after uninstall (28 dead keys).
    let executable = Path::new(if cfg!(windows) {
        r"C:\Users\Example\.claude\keel.exe"
    } else {
        "/home/example/.claude/keel"
    });
    let hook_path = temp_hook_path("keel-uninstall-roundtrip");
    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

    let installed = build_hooks_payload(&hook_path, executable).unwrap();
    std::fs::write(&hook_path, &installed).unwrap();
    // Sanity: the install added stanzas.
    let installed_doc: JsonDocument = serde_json::from_str(&installed).unwrap();
    assert!(
        !installed_doc
            .get("hooks")
            .and_then(JsonDocument::as_object)
            .unwrap()
            .is_empty(),
        "install must add hook stanzas"
    );

    let (removed_payload, removed) = remove_managed_hook_payload(&hook_path).unwrap();
    assert!(removed, "uninstall must report a change");
    let removed_doc: JsonDocument = serde_json::from_str(&removed_payload).unwrap();
    let hooks = removed_doc
        .get("hooks")
        .and_then(JsonDocument::as_object)
        .unwrap();
    assert!(
        hooks.is_empty(),
        "uninstall must leave zero hook event keys, found: {:?}",
        hooks.keys().collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

#[test]
fn uninstall_preserves_user_authored_hook_on_shared_event() {
    // A user's own hook on an event we also manage must survive uninstall —
    // only our managed entry is removed, and the event key is preserved
    // because it still holds the user's matcher.
    let mut document = serde_json::json!({
        "hooks": {
            "Stop": [
                {
                    "matcher": "",
                    "hooks": [
                        { "type": "command", "command": "keel", "args": ["hook", "stop"] },
                        { "type": "command", "command": "/usr/local/bin/my-own-stop.sh" }
                    ]
                }
            ],
            "PostToolUse": [
                {
                    "matcher": "",
                    "hooks": [
                        { "type": "command", "command": "keel", "args": ["hook", "post-tool-use"] }
                    ]
                }
            ]
        }
    });

    remove_managed_hooks(&mut document);
    let hooks = document
        .get("hooks")
        .and_then(JsonDocument::as_object)
        .unwrap();

    // PostToolUse held only our entry -> key pruned entirely.
    assert!(
        !hooks.contains_key("PostToolUse"),
        "fully-managed event key must be pruned"
    );
    // Stop still holds the user's script -> key preserved with that entry.
    let stop_commands = hooks
        .get("Stop")
        .and_then(JsonDocument::as_array)
        .and_then(|entries| entries.first())
        .and_then(|entry| entry.get("hooks"))
        .and_then(JsonDocument::as_array)
        .expect("Stop event must survive with the user's hook");
    assert_eq!(stop_commands.len(), 1, "only the user's hook remains");
    assert_eq!(
        stop_commands[0]
            .get("command")
            .and_then(JsonDocument::as_str),
        Some("/usr/local/bin/my-own-stop.sh"),
        "the user's own hook must be preserved verbatim"
    );
}

fn base64_encode_for_test(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rendered = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        rendered.push(ALPHABET[(first >> 2) as usize] as char);
        rendered.push(ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            rendered
                .push(ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            rendered.push('=');
        }
        if chunk.len() > 2 {
            rendered.push(ALPHABET[(third & 0b0011_1111) as usize] as char);
        } else {
            rendered.push('=');
        }
    }
    rendered
}

#[test]
fn pre_tool_use_scopes_to_bash_and_post_tool_use_fires_for_all_tools() {
    // PreToolUse stays Bash-scoped: the rewriter only operates on shell
    // commands. PostToolUse must fire for every tool — the handler gates
    // the edit-counter path on `is_edit_class_tool` (Edit/Write/MultiEdit/
    // NotebookEdit) at runtime, which would be unreachable if the harness
    // only delivered Bash events. The empty matcher also lets
    // `tool_timings::record_tool_timing` sample non-Bash tools so the
    // compression-discipline nudge fires when context fills with file
    // reads and edits, not only with shell output.
    let hook_path = temp_hook_path("keel-hook-matcher-scope");

    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();

    std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

    let rendered = build_hooks_payload(&hook_path, Path::new("keel")).unwrap();

    let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

    let hooks = document
        .get("hooks")
        .and_then(JsonDocument::as_object)
        .unwrap();

    for (event, expected_matcher) in [
        ("PreToolUse", ""),
        ("PostToolUse", ""),
        ("UserPromptSubmit", ""),
        ("SessionStart", ""),
    ] {
        let matcher = hooks
            .get(event)
            .and_then(JsonDocument::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("matcher"))
            .and_then(JsonDocument::as_str)
            .unwrap_or_else(|| panic!("missing matcher for {event}"));

        assert_eq!(matcher, expected_matcher, "unexpected matcher for {event}");
    }

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

#[test]
fn silenced_high_frequency_hooks_emit_no_additional_context() {
    // PostToolUse / SubagentStop / SessionEnd fire per tool call or
    // turn end and carry a per-prompt token cost that outweighs the
    // value of any per-call reminder. The operating contract belongs
    // in CLAUDE.md and SessionStart, both paid at most once per session.
    // These events must stay silent.
    //
    // Stop is deliberately NOT in this list: per the official docs it
    // supports additionalContext and we now use it for closeout context.
    // SubagentStart is also NOT here: it injects iron law context.
    // UserPromptSubmit and PostToolBatch emit their own context,
    // gated by their own dedicated tests below.
    for subcommand in [
        "post-tool-use",
        "post-tool-use-failure",
        "subagent-stop",
        "session-end",
    ] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_lifecycle(subcommand, &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "stderr for {subcommand}: {}",
            String::from_utf8_lossy(&stderr)
        );

        assert!(
            stdout.is_empty(),
            "{subcommand} must emit no additional context to avoid per-prompt token cost; got: {}",
            String::from_utf8_lossy(&stdout)
        );
    }
}

#[test]
fn system_map_refresh_fires_on_session_start_pre_compact_and_session_end() {
    // The agent has historically forgotten to invoke memory scope
    // resolve / system-map refresh by hand. The lifecycle handler now
    // does it automatically at the three natural transition points so
    // SYSTEM_MAP.md is fresh when a new session starts, when context is
    // about to be compacted, and after the session ends. Any change to
    // this trigger set is a behavior change the user should see — this
    // test pins it.
    for event_name in ["SessionStart", "PreCompact", "SessionEnd"] {
        assert!(
            should_refresh_system_map(event_name),
            "{event_name} must auto-refresh the workspace SYSTEM_MAP"
        );
    }
}

#[test]
fn system_map_refresh_does_not_fire_on_per_prompt_or_per_tool_events() {
    // Per-prompt and per-tool-call events fire too often to pay the
    // SYSTEM_MAP refresh cost on each. The PostToolUse path has its own
    // edit-counter gate (see run_hook_post_tool_use); these slugs must
    // stay out of the lifecycle auto-refresh trigger set.
    for event_name in [
        "UserPromptSubmit",
        "PostToolUse",
        "PostToolBatch",
        "Stop",
        "SubagentStop",
        "SubagentStart",
        "PostCompact",
        "Notification",
        "PermissionRequest",
    ] {
        assert!(
            !should_refresh_system_map(event_name),
            "{event_name} must not auto-refresh the workspace SYSTEM_MAP"
        );
    }
}

#[test]
fn user_prompt_submit_emits_research_first_pointer() {
    // UserPromptSubmit lands per-prompt, so the injected text must be
    // short and pointer-shaped. The iron law (trust the codebase, invoke
    // skills before responding, find root cause) restates the bootstrap
    // skill that SessionStart already delivered, so it stays top-of-mind
    // on each turn.
    //
    // Production path: `run_hook_command` routes the slug to
    // `run_hook_user_prompt_submit`, which reads stdin for `session_id`
    // and applies the optional compression-discipline nudge. This test
    // exercises that dispatcher directly with an empty reader so the
    // fail-open branch yields the base `user_prompt_submit_context()`
    // with no compression nudge appended — exactly the back-compat
    // contract. The empty reader is also what makes this test safe to
    // run from a parent process that holds an open stdin handle (e.g.
    // PowerShell on Windows); reading the real stdin handle there
    // blocks indefinitely.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    // Make the assertion deterministic: even if some operator has
    // CLAUDE_SKILLS_COMPRESSION_HINT=force exported in the test
    // environment, this test asserts the unforced base contract.
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

    let mut stdin = std::io::empty();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }

    assert_eq!(
        code,
        0,
        "stderr for user-prompt-submit: {}",
        String::from_utf8_lossy(&stderr)
    );

    let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

    let context = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("additionalContext"))
        .and_then(JsonDocument::as_str)
        .expect("UserPromptSubmit must emit additionalContext");

    assert!(context.contains("Research-first"));
    assert!(context.contains("SYSTEM_MAP"));
    assert!(context.contains("trust the codebase"));
    assert!(context.contains("Skill tool"));
    assert!(context.contains("root cause"));
    assert!(context.contains("No assumptions"));

    // MCP-tool advertisement — every per-prompt injection must name the
    // always-available keel MCP tools so the model reaches for
    // `system_map`/`recall` instead of guessing about the repo or its
    // memory. This is the base-context half of the fix; the targeted
    // repo/memory-question pointer (tested separately) is the other half.
    assert!(
        context.contains("system_map"),
        "UserPromptSubmit must advertise the system_map MCP tool"
    );
    assert!(
        context.contains("recall"),
        "UserPromptSubmit must advertise the recall MCP tool"
    );
    assert!(
        context.contains("run_command"),
        "UserPromptSubmit must advertise the run_command MCP tool"
    );

    // Understand-before-building — the per-prompt hook must require the
    // model to understand the request and research what is needed before
    // writing code, so it does not build the wrong thing. This is distinct
    // from the root-cause/debugging cue below: it governs the front of the
    // task (what to build), not the middle (where the bug is). It lands
    // per prompt because the SessionStart bootstrap drops out of the
    // working window after a few turns.
    assert!(
        context.contains("Understand before building"),
        "UserPromptSubmit must name the understand-before-building rule"
    );
    assert!(
        context.contains("building the wrong thing"),
        "UserPromptSubmit must state that research prevents building the wrong thing"
    );

    // Deep-dive cues — the per-prompt pointer must keep the model from
    // jumping from suspicion to fix. These two phrases name the failure
    // mode ("this may be the case" → patch) and the required discipline
    // (trace the symptom and confirm the suspect is on that path before
    // changing it). They live here, not just in the bootstrap, because
    // SessionStart context drops out of the working window after a few
    // turns while UserPromptSubmit lands per prompt.
    assert!(
        context.contains("suspicion is a hypothesis"),
        "UserPromptSubmit must restate that suspicion is a hypothesis, not a finding"
    );
    assert!(
        context.contains("trace the symptom"),
        "UserPromptSubmit must require tracing the symptom before naming a root cause"
    );
    assert!(
        context.contains("this may be the case"),
        "UserPromptSubmit must name the \"this may be the case\" jump as the failure mode"
    );

    // Implementation-discipline pillars — UserPromptSubmit lands per
    // prompt, so naming the four pillars by name keeps them top-of-mind
    // even after the SessionStart bootstrap drops out of the model's
    // working window. The full text lives in the bootstrap and in
    // _shared/common-discipline.md; this hook only restates the names.
    assert!(
        context.contains("Think Before Coding"),
        "UserPromptSubmit must name the Think Before Coding pillar"
    );
    assert!(
        context.contains("Simplicity First"),
        "UserPromptSubmit must name the Simplicity First pillar"
    );
    assert!(
        context.contains("Surgical Changes"),
        "UserPromptSubmit must name the Surgical Changes pillar"
    );
    assert!(
        context.contains("Goal-Driven Execution"),
        "UserPromptSubmit must name the Goal-Driven Execution pillar"
    );

    let event_name = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("hookEventName"))
        .and_then(JsonDocument::as_str);

    assert_eq!(event_name, Some("UserPromptSubmit"));
}

#[test]
fn mcp_pointer_fires_for_repo_structure_questions() {
    // The skill matcher stays silent on these prompts (no distinctive
    // domain token), so this targeted pointer is the only thing that nudges
    // the model to call `system_map` instead of guessing the layout. Cover
    // the common phrasings.
    for prompt in [
        "what is this project about?",
        "so what's this repo for",
        "give me a project overview",
        "explain the architecture here",
        "how is this codebase structured",
        "what does this project do",
    ] {
        let pointer = mcp_tool_pointer_for_prompt(prompt).unwrap_or_else(|| {
            panic!("repo-question prompt should point at system_map: {prompt:?}")
        });
        assert!(
            pointer.contains("system_map"),
            "repo-question pointer must name system_map: {prompt:?}"
        );
    }
}

#[test]
fn mcp_pointer_fires_for_memory_questions() {
    for prompt in [
        "what do you remember about this work",
        "what did you learn last session",
        "do you remember the auth refactor",
        "recall what we decided about pagination",
    ] {
        let pointer = mcp_tool_pointer_for_prompt(prompt)
            .unwrap_or_else(|| panic!("memory-question prompt should point at recall: {prompt:?}"));
        assert!(
            pointer.contains("recall"),
            "memory-question pointer must name recall: {prompt:?}"
        );
    }
}

#[test]
fn mcp_pointer_silent_for_ordinary_work() {
    // Must not fire on ordinary feature/bugfix prompts — even ones that
    // mention "project" incidentally — or the reminder becomes noise on
    // every turn. Silence here is the correct, conservative default.
    for prompt in [
        "add a logout button to the navbar",
        "fix the failing pagination test",
        "refactor the project's auth module to use PKCE",
        "why is this function returning null",
        "",
        "   ",
    ] {
        assert_eq!(
            mcp_tool_pointer_for_prompt(prompt),
            None,
            "pointer must stay silent for ordinary work: {prompt:?}"
        );
    }
}

#[test]
fn mcp_pointer_prefers_recall_for_memory_shaped_repo_question() {
    // "what did you learn about this project" mentions "project" but is a
    // memory ask — the recall answer is the right one, so the memory branch
    // must win over the repo branch.
    let pointer = mcp_tool_pointer_for_prompt("what did you learn about this project")
        .expect("memory-shaped prompt should match");
    assert!(
        pointer.contains("recall"),
        "memory-shaped prompt must prefer recall over system_map"
    );
    assert!(
        !pointer.contains("structural map"),
        "memory-shaped prompt must not fire the repo-structure pointer"
    );
}

#[test]
fn work_intent_pointer_fires_on_code_change_prompts() {
    // The targeting fix: code-change prompts must get the read-map / recall /
    // write-brief / preserve-flow reminder. These are exactly the prompts the
    // question pointer stays silent on.
    for prompt in [
        "rework the github skills in this repo",
        "fix the failing pagination test",
        "refactor the auth module to use PKCE",
        "implement a logout endpoint",
        "add a retry to the upload client",
        "update the config loader to read TOML",
        "migrate the store to sqlite",
        "rename getUserName to getUsername",
    ] {
        let pointer = work_intent_pointer_for_prompt(prompt)
            .unwrap_or_else(|| panic!("work pointer must fire for: {prompt:?}"));
        assert!(
            pointer.contains("SYSTEM_MAP") && pointer.contains("working brief"),
            "work pointer must name the map and the brief: {prompt:?}"
        );
        assert!(
                pointer.contains("preserve-existing-flow"),
                "work pointer must route existing-code edits through preserve-existing-flow: {prompt:?}"
            );
    }
}

#[test]
fn work_intent_pointer_silent_for_questions_and_chitchat() {
    // Must not fire on questions, read-only asks, or empty prompts — that
    // would turn the reminder into per-turn noise. Conservative by design.
    for prompt in [
        "why is this function returning null",
        "what does this module do",
        "explain how the gate works",
        "is the build passing",
        "thanks, that looks good",
        "",
        "   ",
    ] {
        assert_eq!(
            work_intent_pointer_for_prompt(prompt),
            None,
            "work pointer must stay silent for non-change prompts: {prompt:?}"
        );
    }
}

#[test]
fn user_prompt_submit_injects_mcp_pointer_for_repo_question() {
    // End-to-end through the dispatcher: a repo-structure prompt on stdin
    // must surface the system_map pointer in the emitted additionalContext,
    // ahead of the base research-first context. This is the integration
    // half of the fix — the unit tests above prove the detector, this
    // proves it is actually wired into the per-prompt payload.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

    let payload = serde_json::json!({
        "session_id": "",
        "prompt": "what is this project about?"
    })
    .to_string();
    let payload_bytes = payload.into_bytes();
    let mut stdin: &[u8] = &payload_bytes;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }

    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));

    let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");
    let context = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("additionalContext"))
        .and_then(JsonDocument::as_str)
        .expect("additionalContext present");

    assert!(
        context.contains("system_map"),
        "repo question must inject the system_map pointer; got: {context}"
    );
    assert!(
        context.contains("Research-first"),
        "base research-first context must still be present"
    );
}

#[test]
fn user_prompt_submit_consumes_injected_stdin_payload_without_blocking() {
    // Regression test for the stdin-blocking hang fixed in this commit.
    // Before the fix, `run_hook_user_prompt_submit` read directly from
    // `std::io::stdin()`, which on Windows under PowerShell hangs
    // indefinitely because the parent's open console handle is inherited
    // by the test runner. The fix injects the reader, so this test can
    // pass real JSON bytes through `&mut &[u8]` and prove the parser
    // actually consumed them. If this test ever hangs, the fix has
    // regressed: the function is reading the global stdin handle again.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

    // Real the harness payload shape — UserPromptSubmit always carries a
    // `session_id`. The hook fail-opens to the base context when the
    // session-keyed compression-hint heuristic returns None (no JSONL
    // rows recorded yet for this session in the current claude_home),
    // so the assertion here is just "exit 0 + base context present"
    // rather than "compression hint included," which keeps the test
    // stable across hosts that may or may not have CLAUDE_TARGET_OVERRIDE
    // populated.
    let payload =
        br#"{"session_id":"test-session-stdin-injection","hook_event_name":"UserPromptSubmit"}"#;
    let mut stdin: &[u8] = payload;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_user_prompt_submit(&mut stdin, &mut stdout, &mut stderr);

    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }

    assert_eq!(
        code,
        0,
        "stderr for injected-payload user-prompt-submit: {}",
        String::from_utf8_lossy(&stderr)
    );

    let output: JsonDocument =
        serde_json::from_slice(&stdout).expect("valid JSON for injected payload");
    let context = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("additionalContext"))
        .and_then(JsonDocument::as_str)
        .expect("UserPromptSubmit must emit additionalContext for injected payload");
    assert!(
        context.contains("Research-first"),
        "base context must still appear when stdin carries a real payload"
    );

    // Reader was fully consumed. `&[u8]` advances on read, so a
    // post-call slice length of 0 proves the function read the whole
    // payload (the fix is exercising the injection point) and did not
    // silently drop straight to the empty-stdin fallback. Combined
    // with the function having exactly one read path (line 870), this
    // is sufficient: a regression that re-introduced a global stdin
    // read would have to also remove this read of the injected reader,
    // which would leave the byte slice non-empty.
    assert!(
        stdin.is_empty(),
        "function must drain the injected reader; remaining bytes signal a regression"
    );
}

#[test]
fn post_tool_batch_emits_reviewer_on_close_reminder() {
    // PostToolBatch fires after a batch of parallel tools resolves, just
    // before the model's next turn. It's the officially-supported event
    // for "before close" reminders — Stop/SubagentStop don't accept
    // hookSpecificOutput per the schema.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_lifecycle("post-tool-batch", &mut stdout, &mut stderr);

    assert_eq!(
        code,
        0,
        "stderr for post-tool-batch: {}",
        String::from_utf8_lossy(&stderr)
    );

    let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

    let context = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("additionalContext"))
        .and_then(JsonDocument::as_str)
        .expect("PostToolBatch must emit additionalContext");

    assert!(
        context.contains("reviewer pass"),
        "PostToolBatch reminder must surface the reviewer-pass closeout requirement"
    );
    assert!(
        context.contains("logic edits")
            || context.contains("multi-file")
            || context.contains("public-API")
            || context.contains("code changed"),
        "PostToolBatch reminder must state what triggers a reviewer pass"
    );
    assert!(
        context.contains("Trivial") || context.contains("docs") || context.contains("formatting"),
        "PostToolBatch reminder must spell out the exempt trivial cases"
    );
    assert!(
            !context.contains("Routing Rules"),
            "PostToolBatch reminder must not cite a repo-specific section name; the rule is stated inline so it works across host repos"
        );
    assert!(
        context.contains("clearable nudge")
            || context.contains("decide deliberately")
            || context.contains("rules take precedence"),
        "PostToolBatch reminder must encourage deliberate consideration before skipping"
    );

    let event_name = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("hookEventName"))
        .and_then(JsonDocument::as_str);

    assert_eq!(event_name, Some("PostToolBatch"));
}

// ----- Shared gate decision-core tests (review gate + brief gate) -----

#[test]
fn gate_disabled_decision_is_always_advisory() {
    // With a gate Off, no combination of inputs fires.
    for edit_count in [0usize, 1, 50] {
        for satisfied in [true, false] {
            assert_eq!(
                decide_gate(GateMode::Off, 1, 0, edit_count, satisfied),
                GateDecision::Advisory,
                "Off gate must never fire (edits={edit_count}, satisfied={satisfied})"
            );
        }
    }
}

#[test]
fn gate_max_blocks_zero_is_advisory() {
    // Cap of 0 is a second kill switch: enabled but never fires, in either mode.
    assert_eq!(
        decide_gate(GateMode::Nudge, 0, 0, 5, false),
        GateDecision::Advisory
    );
    assert_eq!(
        decide_gate(GateMode::Block, 0, 0, 5, false),
        GateDecision::Advisory
    );
}

#[test]
fn gate_no_edits_is_advisory() {
    // Pure-research / question turns changed no code — never fire them.
    assert_eq!(
        decide_gate(GateMode::Nudge, 1, 0, 0, false),
        GateDecision::Advisory
    );
    assert_eq!(
        decide_gate(GateMode::Block, 1, 0, 0, false),
        GateDecision::Advisory
    );
}

#[test]
fn gate_satisfied_is_advisory() {
    // The gate-specific requirement is already met (review ran / brief
    // written) — nothing to fire on, in either mode.
    assert_eq!(
        decide_gate(GateMode::Nudge, 1, 0, 5, true),
        GateDecision::Advisory
    );
    assert_eq!(
        decide_gate(GateMode::Block, 1, 0, 5, true),
        GateDecision::Advisory
    );
}

#[test]
fn gate_fires_unsatisfied_edits_once() {
    // Enabled, code changed, requirement unmet, under the cap → fire.
    // Default Nudge mode yields a non-blocking nudge; Block mode yields a stop.
    assert_eq!(
        decide_gate(GateMode::Nudge, 1, 0, 5, false),
        GateDecision::Nudge,
        "default mode must NUDGE, never block — this is the no-stop fix"
    );
    assert_eq!(
        decide_gate(GateMode::Block, 1, 0, 5, false),
        GateDecision::Block,
        "block mode must restore the opt-in hard stop"
    );
}

#[test]
fn gate_cannot_loop_terminates_at_cap() {
    // THE TERMINATION PROOF. Simulate the worst case: the gate stays enabled,
    // code stays changed, and the requirement is NEVER satisfied
    // (satisfied=false forever). Drive the loop exactly as the dispatcher
    // does — increment the issued counter on every fire — and assert it
    // stops firing once the cap is reached, no matter how many turns elapse.
    // If this ever looped in production it would spam (nudge) or wedge (block)
    // every project; this test is the guarantee that it cannot. Covers both
    // gates AND both modes because they share this exact decision core.
    for mode in [GateMode::Nudge, GateMode::Block] {
        for max_blocks in [1u64, 2, 3] {
            let mut blocks_issued = 0u64;
            let mut total_fires = 0u64;
            for _turn in 0..1000 {
                match decide_gate(mode, max_blocks, blocks_issued, 5, false) {
                    GateDecision::Nudge | GateDecision::Block => {
                        blocks_issued += 1;
                        total_fires += 1;
                    }
                    GateDecision::Advisory => {}
                }
            }
            assert_eq!(
                    total_fires, max_blocks,
                    "gate ({mode:?}) must fire exactly max_blocks ({max_blocks}) times across a long session, then fall through forever"
                );
        }
    }
}

#[test]
fn run_hook_post_tool_batch_both_gates_off_matches_advisory_path() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _silenced = NewGatesSilenced::new();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    let mut gate_stdin = std::io::empty();
    let mut gate_out = Vec::new();
    let mut gate_err = Vec::new();
    let gate_code = run_hook_post_tool_batch(&mut gate_stdin, &mut gate_out, &mut gate_err);

    let mut adv_out = Vec::new();
    let mut adv_err = Vec::new();
    let adv_code = run_hook_lifecycle("post-tool-batch", &mut adv_out, &mut adv_err);

    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }

    assert_eq!(gate_code, 0);
    assert_eq!(adv_code, 0);
    assert_eq!(
        String::from_utf8_lossy(&gate_out),
        String::from_utf8_lossy(&adv_out),
        "all-gates-off dispatcher output must match the advisory lifecycle path exactly"
    );
    assert!(
        !String::from_utf8_lossy(&gate_out).contains("\"decision\""),
        "disabled gates must never emit a decision field"
    );
}

#[test]
fn gate_mode_parses_off_block_nudge_and_escalate_default() {
    // The default-on-as-escalate contract: unset → Escalate; explicit disable
    // tokens → Off; `nudge` → Nudge (opt-down); `block` → Block (opt-up);
    // anything else (including a typo) → Escalate (fail toward the default
    // that warns first and only then blocks).
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const PROBE: &str = "CLAUDE_SKILLS_GATE_DEFAULT_PROBE";
    let previous = std::env::var(PROBE).ok();

    std::env::remove_var(PROBE);
    assert_eq!(
        gate_mode(PROBE),
        GateMode::Escalate,
        "unset must default to the escalating gate"
    );

    for disable in ["off", "0", "false", "no", "OFF", "  off  ", "False"] {
        std::env::set_var(PROBE, disable);
        assert_eq!(
            gate_mode(PROBE),
            GateMode::Off,
            "disable token {disable:?} must turn the gate Off"
        );
    }
    for nudge in ["nudge", "NUDGE", "  Nudge  "] {
        std::env::set_var(PROBE, nudge);
        assert_eq!(
            gate_mode(PROBE),
            GateMode::Nudge,
            "value {nudge:?} must select the advisory-only nudge"
        );
    }
    for block in ["block", "BLOCK", "  Block  "] {
        std::env::set_var(PROBE, block);
        assert_eq!(
            gate_mode(PROBE),
            GateMode::Block,
            "value {block:?} must select the always-block hard stop"
        );
    }
    for escalate in ["on", "1", "true", "yes", "wibble", "", "escalate"] {
        std::env::set_var(PROBE, escalate);
        assert_eq!(
            gate_mode(PROBE),
            GateMode::Escalate,
            "non-off, non-nudge, non-block value {escalate:?} must default to escalate"
        );
    }

    match previous {
        Some(value) => std::env::set_var(PROBE, value),
        None => std::env::remove_var(PROBE),
    }
}

#[test]
fn gate_escalate_nudges_first_then_blocks() {
    // The core escalation contract: fire 0 (blocks_issued == 0) is a
    // non-blocking nudge; once that nudge is spent (blocks_issued == 1) the
    // SAME unmet requirement escalates to a hard block. Uses the escalate
    // default cap of 2 so both fires are under the cap.
    let max = default_max_blocks_for(GateMode::Escalate);
    assert_eq!(max, 2, "escalate default cap must allow nudge + block");
    assert_eq!(
        decide_gate(GateMode::Escalate, max, 0, 5, false),
        GateDecision::Nudge,
        "escalate first contact must NUDGE, not interrupt mid-task"
    );
    assert_eq!(
        decide_gate(GateMode::Escalate, max, 1, 5, false),
        GateDecision::Block,
        "escalate second fire must BLOCK once the nudge was ignored"
    );
    // Satisfying the requirement at any point stops the escalation cold.
    assert_eq!(
        decide_gate(GateMode::Escalate, max, 1, 5, true),
        GateDecision::Advisory,
        "meeting the requirement must halt escalation immediately"
    );
}

#[test]
fn gate_escalate_terminates_at_cap() {
    // Termination proof for the escalating gate: driven turn-by-turn with the
    // requirement NEVER met, it fires exactly `max_blocks` times (one nudge
    // then blocks) and then falls through to advisory forever — never loops.
    let max = default_max_blocks_for(GateMode::Escalate);
    let mut blocks_issued = 0u64;
    let mut nudges = 0u64;
    let mut blocks = 0u64;
    for _turn in 0..1000 {
        match decide_gate(GateMode::Escalate, max, blocks_issued, 5, false) {
            GateDecision::Nudge => {
                nudges += 1;
                blocks_issued += 1;
            }
            GateDecision::Block => {
                blocks += 1;
                blocks_issued += 1;
            }
            GateDecision::Advisory => {}
        }
    }
    assert_eq!(
        nudges, 1,
        "escalate must nudge exactly once (the first fire)"
    );
    assert_eq!(
        blocks,
        max - 1,
        "escalate must block (cap - 1) times after the opening nudge"
    );
    assert_eq!(
        nudges + blocks,
        max,
        "total fires must equal the cap, then advisory forever"
    );
}

#[test]
fn gate_mode_parses_off_block_and_nudge_default() {
    // Back-compat shim: the historical test name kept as a thin wrapper so a
    // grep for the old name still finds coverage. Delegates to the canonical
    // escalate-aware test above.
    gate_mode_parses_off_block_nudge_and_escalate_default();
}

#[test]
fn review_gate_messages_name_the_switches() {
    // Operators must always be told how to change/disable the gate, right in
    // the message — keyed on the emitted decision (nudge vs block).
    let nudge = review_gate_message(GateDecision::Nudge);
    assert!(nudge.contains("CLAUDE_SKILLS_REVIEW_GATE"));
    assert!(nudge.contains("=block"));
    assert!(nudge.contains("=off"));
    assert!(nudge.contains("review pre-pr"));
    assert!(
        nudge.contains("does not stop the turn"),
        "nudge message must make clear it is non-blocking"
    );
    assert!(
        nudge.contains("escalate"),
        "nudge message must warn that an unmet requirement escalates"
    );

    let block = review_gate_message(GateDecision::Block);
    assert!(block.contains("CLAUDE_SKILLS_REVIEW_GATE"));
    assert!(block.contains("=off"));
    assert!(block.contains("review pre-pr"));
    assert!(
        block.contains("cannot loop") || block.contains("bounded"),
        "block message must reassure that the gate is bounded"
    );
    assert!(
        block.contains("hard stop"),
        "block message must make clear it now halts the turn"
    );
}

#[test]
fn brief_gate_messages_name_the_switches_and_action() {
    // The brief-gate message must tell the model how to clear it (write a
    // brief) and how to change/disable it, keyed on the emitted decision.
    let nudge = brief_gate_message(GateDecision::Nudge);
    assert!(nudge.contains("CLAUDE_SKILLS_BRIEF_GATE"));
    assert!(nudge.contains("=block"));
    assert!(nudge.contains("=off"));
    assert!(
        nudge.contains("working-brief write"),
        "nudge message must name the brief-write surface that clears the gate"
    );
    assert!(
        nudge.contains("does not stop the turn"),
        "nudge message must make clear it is non-blocking"
    );
    assert!(
        nudge.contains("escalate"),
        "nudge message must warn that an unmet requirement escalates"
    );

    let block = brief_gate_message(GateDecision::Block);
    assert!(block.contains("CLAUDE_SKILLS_BRIEF_GATE"));
    assert!(block.contains("=off"));
    assert!(
        block.contains("working-brief write"),
        "block message must name the brief-write surface that clears the gate"
    );
    assert!(
        block.contains("cannot loop") || block.contains("bounded"),
        "block message must reassure that the gate is bounded"
    );
    assert!(
        block.contains("hard stop"),
        "block message must make clear it now halts the turn"
    );
}

#[test]
fn brief_written_this_session_logic() {
    const WS_A: &str = "D:/Nasri/Project/alpha";
    const WS_B: &str = "D:/Nasri/Project/beta";

    // Unknown session start → satisfied (fail-open: never block a session we
    // cannot time).
    let claude_home = temp_brief_gate_home("unknown-start");
    assert!(
        brief_written_this_session(&claude_home, WS_A, None),
        "unknown session start must report satisfied"
    );

    // Known start, no brief on disk → not satisfied.
    assert!(
        !brief_written_this_session(&claude_home, WS_A, Some(now_ms())),
        "known start with no brief must be unsatisfied"
    );

    // A brief written ~now for WS_A covers a WS_A session starting ~now.
    let brief_a = crate::utility::working_brief::create_brief(
        "wb-gate-a".into(),
        "cover this session".into(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        WS_A.into(),
        "2026-06-06T00:00:00Z".into(),
    );
    crate::utility::working_brief::write_brief(&claude_home, &brief_a).expect("write brief a");
    assert!(
        brief_written_this_session(&claude_home, WS_A, Some(now_ms())),
        "a freshly written brief for this workspace must satisfy a session starting now"
    );

    // WORKSPACE SCOPING (the point of the fix): the WS_A brief must NOT
    // satisfy a session editing WS_B — a brief for one project does not
    // count for another.
    assert!(
        !brief_written_this_session(&claude_home, WS_B, Some(now_ms())),
        "a brief written for another workspace must not satisfy this one"
    );

    // BACKWARD COMPAT: a legacy brief with an empty workspace applies
    // anywhere, so it satisfies WS_B too (older briefs never start blocking).
    let brief_legacy = crate::utility::working_brief::create_brief(
        "wb-gate-legacy".into(),
        "legacy brief".into(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        String::new(),
        "2026-06-06T00:00:00Z".into(),
    );
    crate::utility::working_brief::write_brief(&claude_home, &brief_legacy)
        .expect("write legacy brief");
    assert!(
        brief_written_this_session(&claude_home, WS_B, Some(now_ms())),
        "an empty-workspace (legacy) brief must apply to any workspace"
    );

    // A brief far older than a session that starts well beyond the grace
    // window → not satisfied (prior-session brief does not count). Use a
    // fresh home so the briefs written above do not satisfy it.
    let stale_home = temp_brief_gate_home("stale");
    let stale_brief = crate::utility::working_brief::create_brief(
        "wb-gate-stale".into(),
        "old".into(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        WS_A.into(),
        "2026-06-06T00:00:00Z".into(),
    );
    crate::utility::working_brief::write_brief(&stale_home, &stale_brief)
        .expect("write stale brief");
    let far_future_start = now_ms().saturating_add(BRIEF_GATE_SESSION_GRACE_MS * 10);
    assert!(
        !brief_written_this_session(&stale_home, WS_A, Some(far_future_start)),
        "a brief older than (session_start - grace) must not satisfy the gate"
    );

    let _ = std::fs::remove_dir_all(&claude_home);
    let _ = std::fs::remove_dir_all(&stale_home);
}

struct NewGatesSilenced {
    previous: Vec<(&'static str, Option<String>)>,
}

impl NewGatesSilenced {
    fn new() -> Self {
        let vars = [
            MEMORY_GATE_ENV_VAR,
            SPRINT_START_GATE_ENV_VAR,
            LEARNED_SKILL_GATE_ENV_VAR,
        ];
        let previous = vars
            .iter()
            .map(|&var| {
                let prior = std::env::var(var).ok();
                std::env::set_var(var, "off");
                (var, prior)
            })
            .collect();
        Self { previous }
    }
}

impl Drop for NewGatesSilenced {
    fn drop(&mut self) {
        for (var, prior) in &self.previous {
            match prior {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
    }
}

fn temp_brief_gate_home(label: &str) -> std::path::PathBuf {
    let unique: u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let directory = std::env::temp_dir().join(format!(
        "keel-brief-gate-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create tempdir");
    directory
}

#[test]
fn run_hook_post_tool_batch_brief_gate_nudges_in_nudge_mode_then_falls_through() {
    // END-TO-END through the real dispatcher in explicit NUDGE mode (the
    // opt-down). Two things proven here:
    //   1. NUDGE mode (BRIEF_GATE=nudge): a session that edited code with no
    //      working brief gets exactly one NON-BLOCKING nudge — additionalContext
    //      carrying the brief reminder, and crucially NO `decision` field, so
    //      the turn is never halted.
    //   2. The per-session counter still advances and the next call falls
    //      through to the generic advisory, so the nudge is bounded (no spam).
    //   3. Opt-up: with BRIEF_GATE=block a fresh session emits `decision:block`.
    // The escalate DEFAULT (nudge-then-block) is covered by the decide_gate
    // unit tests. The review gate is disabled throughout to isolate the brief gate.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-nudge");
    let _silenced = NewGatesSilenced::new();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "nudge");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    // Seed one edit-class timing row for this session so stats.count > 0 and
    // session_start_ms resolves. No brief is written → gate must fire.
    let session_id = "sess-e2e-nudge";
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timings_dir = claude_home.join("state").join("tool-timings");
    std::fs::create_dir_all(&timings_dir).expect("create timings dir");
    let row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": session_id,
        "cwd": "D:/Nasri/Project/gate-e2e",
        "effort_level": "",
    });
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n"),
    )
    .expect("write timings row");

    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    // First call (default mode): must NUDGE — additionalContext with the brief
    // reminder and NO decision field (the turn is not halted).
    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default brief gate must NOT block (no decision field): {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext") && out1_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
        "default brief gate must emit a non-blocking nudge naming the gate: {out1_text}"
    );

    // The counter must have advanced to 1 (the nudge is bounded like a block).
    let blocks_path = brief_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "brief-gate counter must advance to 1 after the nudge"
    );

    // Second call (same unsatisfied state): cap reached → generic advisory.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        !out2_text.contains("\"decision\""),
        "second call must not block: {out2_text}"
    );
    assert!(
        out2_text.contains("Closeout check"),
        "second call must fall through to the generic advisory (cap reached): {out2_text}"
    );

    // Opt-in hard stop: a FRESH session with BRIEF_GATE=block must emit a real
    // decision:block. New session id so its counter starts at zero.
    std::env::set_var(BRIEF_GATE_ENV_VAR, "block");
    let block_session = "sess-e2e-block-optin";
    let block_row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": block_session,
        "cwd": "D:/Nasri/Project/gate-e2e",
        "effort_level": "",
    });
    // Append the new session's row alongside the first.
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n{block_row}\n"),
    )
    .expect("rewrite timings rows");
    let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        out3_text.contains("additionalContext")
            && out3_text.contains("now a hard stop")
            && out3_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
        "BRIEF_GATE=block must emit the feed-forward hard stop: {out3_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn run_hook_post_tool_batch_brief_gate_escalates_by_default_nudge_then_block() {
    // END-TO-END proof of the ESCALATE DEFAULT through the real dispatcher:
    // with BRIEF_GATE unset, a session that edited code with no working brief
    // gets a NON-BLOCKING nudge on the first end-of-turn, then a real
    // `decision:block` on the second (the requirement is still unmet), then
    // falls through to the generic advisory once the cap (2) is spent. This is
    // the "not optional" behavior — ignoring the nudge is no longer free.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-escalate");
    let _silenced = NewGatesSilenced::new();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off"); // isolate the brief gate
    std::env::remove_var(BRIEF_GATE_ENV_VAR); // default → escalate
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    let session_id = "sess-e2e-escalate";
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timings_dir = claude_home.join("state").join("tool-timings");
    std::fs::create_dir_all(&timings_dir).expect("create timings dir");
    let row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": session_id,
        "cwd": "D:/Nasri/Project/escalate-e2e",
        "effort_level": "",
    });
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n"),
    )
    .expect("write timings row");

    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    // First call: non-blocking nudge (no decision field).
    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "escalate first fire must NUDGE, not block: {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext") && out1_text.contains("CLAUDE_SKILLS_BRIEF_GATE"),
        "escalate first fire must emit a non-blocking nudge: {out1_text}"
    );

    // Second call (still no brief): escalate to a real hard block.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        out2_text.contains("additionalContext") && out2_text.contains("now a hard stop"),
        "escalate second fire must emit the feed-forward hard stop: {out2_text}"
    );

    // Third call: cap (2) spent → generic advisory, no decision field.
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        !out3_text.contains("\"decision\""),
        "third call must fall through to advisory (cap spent): {out3_text}"
    );
    assert!(
        out3_text.contains("Closeout check"),
        "third call must be the generic advisory: {out3_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn run_hook_post_tool_batch_review_gate_nudges_in_nudge_mode_then_falls_through() {
    // END-TO-END for the REVIEW gate in explicit NUDGE mode, symmetric to the
    // brief-gate test above. The review gate has distinct plumbing from the
    // brief gate (review_marker_ms, review_gate_blocks_path,
    // review_gate_message), so a regression there could silently re-introduce
    // a wrong `decision:block` even while the brief gate stays correct. This
    // isolates the review gate (brief gate off) and proves:
    //   1. NUDGE mode (REVIEW_GATE=nudge): a session that edited code with no
    //      reviewer marker gets a NON-BLOCKING nudge — additionalContext with
    //      the review reminder and NO `decision` field.
    //   2. The per-session counter advances and the next call falls through to
    //      the generic advisory (bounded, no spam).
    //   3. Opt-up: with REVIEW_GATE=block a fresh session emits decision:block.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-review-nudge");
    let _silenced = NewGatesSilenced::new();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off"); // isolate the review gate
    std::env::set_var(REVIEW_GATE_ENV_VAR, "nudge"); // explicit advisory-only mode
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    // Seed one edit-class timing row. No `.reviewed` marker is written → the
    // review gate sees an unreviewed edit and must fire.
    let session_id = "sess-e2e-review-nudge";
    let cwd = "D:/Nasri/Project/gate-review-e2e";
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timings_dir = claude_home.join("state").join("tool-timings");
    std::fs::create_dir_all(&timings_dir).expect("create timings dir");
    let row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": session_id,
        "cwd": cwd,
        "effort_level": "",
    });
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n"),
    )
    .expect("write timings row");

    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    // First call (default mode): must NUDGE — additionalContext naming the
    // review gate, and NO decision field (turn not halted).
    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default review gate must NOT block (no decision field): {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext") && out1_text.contains("CLAUDE_SKILLS_REVIEW_GATE"),
        "default review gate must emit a non-blocking nudge naming the gate: {out1_text}"
    );

    // Counter advances to 1 (the nudge is bounded like a block).
    let blocks_path = review_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "review-gate counter must advance to 1 after the nudge"
    );

    // Second call (same unsatisfied state): cap reached → generic advisory.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        !out2_text.contains("\"decision\""),
        "second call must not block: {out2_text}"
    );
    assert!(
        out2_text.contains("Closeout check"),
        "second call must fall through to the generic advisory (cap reached): {out2_text}"
    );

    // Opt-in hard stop: a FRESH session with REVIEW_GATE=block must emit a
    // real decision:block. New session id so its counter starts at zero.
    std::env::set_var(REVIEW_GATE_ENV_VAR, "block");
    let block_session = "sess-e2e-review-block-optin";
    let block_row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": block_session,
        "cwd": cwd,
        "effort_level": "",
    });
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n{block_row}\n"),
    )
    .expect("rewrite timings rows");
    let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        out3_text.contains("additionalContext")
            && out3_text.contains("now a hard stop")
            && out3_text.contains("CLAUDE_SKILLS_REVIEW_GATE"),
        "REVIEW_GATE=block must emit the feed-forward hard stop: {out3_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

/// Seed the sprint store for `workspace_cwd` with the given (story, state)
/// pairs, using the same slug + group the gate resolves, so the gate sees a
/// real active sprint. Returns nothing; the records land under
/// `<home>/sprint/<slug>/`.
fn seed_sprint(claude_home: &std::path::Path, workspace_cwd: &str, stories: &[(&str, &str)]) {
    let slug = crate::utility::sprint::workspace_slug_for_test(workspace_cwd);
    let store =
        crate::utility::record_store::RecordStore::new(claude_home, &format!("sprint/{slug}"));
    for (index, (story, state)) in stories.iter().enumerate() {
        let id = format!("s{}", index + 1);
        let record: crate::utility::record_store::Record = vec![
            ("id".into(), id.clone()),
            ("story".into(), (*story).into()),
            ("state".into(), (*state).into()),
            ("note".into(), String::new()),
        ];
        store.write_record(&id, &record).expect("seed sprint story");
    }
}

/// Seed one edit-class timing row so `session_edit_stats` reports the given
/// cwd and a non-zero count (the closeout gate resolves the workspace from the
/// last edit's cwd).
fn seed_edit_row(claude_home: &std::path::Path, session_id: &str, cwd: &str) {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timings_dir = claude_home.join("state").join("tool-timings");
    std::fs::create_dir_all(&timings_dir).expect("create timings dir");
    let row = serde_json::json!({
        "recorded_at_ms": now_ms(),
        "event": "PostToolUse",
        "tool_name": "Edit",
        "duration_ms": 5u64,
        "session_id": session_id,
        "cwd": cwd,
        "effort_level": "",
    });
    std::fs::write(
        timings_dir.join(format!("{date}.jsonl")),
        format!("{row}\n"),
    )
    .expect("write timings row");
}

#[test]
fn story_closeout_gate_nudges_when_sprint_incomplete_then_silent_without_sprint() {
    // The honest-closeout gate (story 1 + 2 + 3). Isolates it by disabling the
    // brief and review gates. Proves:
    //   1. Active sprint with an open story -> NON-BLOCKING nudge naming the gap
    //      (no `decision` field), and the counter advances (bounded).
    //   2. A different workspace with NO sprint -> the gate stays silent and the
    //      turn falls through to the generic advisory.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-closeout");
    let _silenced = NewGatesSilenced::new();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR); // default → escalate (first fire nudges)

    // Workspace WITH an incomplete sprint.
    let open_cwd = "D:/Nasri/Project/closeout-open";
    let session_id = "sess-closeout-open";
    seed_edit_row(&claude_home, session_id, open_cwd);
    seed_sprint(
        &claude_home,
        open_cwd,
        &[
            ("As a dev, I want A, so that X.", "done"),
            ("As a dev, I want B, so that Y.", "todo"),
        ],
    );
    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default closeout gate must NOT block: {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext")
            && out1_text.contains("CLAUDE_SKILLS_STORY_CLOSEOUT_GATE")
            && out1_text.contains("s2"),
        "closeout nudge must name the open story s2 as a gap: {out1_text}"
    );
    let blocks_path = story_closeout_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "closeout counter must advance to 1 after the nudge"
    );

    // Workspace WITHOUT a sprint -> gate silent, generic advisory.
    let none_cwd = "D:/Nasri/Project/closeout-none";
    let none_session = "sess-closeout-none";
    seed_edit_row(&claude_home, none_session, none_cwd);
    let none_stdin = format!("{{\"session_id\":\"{none_session}\"}}");
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut none_stdin.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        !out2_text.contains("CLAUDE_SKILLS_STORY_CLOSEOUT_GATE"),
        "no sprint -> closeout gate must stay silent: {out2_text}"
    );
    assert!(
        out2_text.contains("Closeout check"),
        "no-sprint turn must fall through to the generic advisory: {out2_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn story_closeout_gate_blocks_when_opted_in_and_silent_when_complete() {
    // Proves the opt-in hard stop (=block) fires with `decision:block` on an
    // incomplete sprint, and that a fully-Done sprint never fires (silent).
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-closeout-block");
    let _silenced = NewGatesSilenced::new();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "block");

    // Incomplete sprint -> decision:block.
    let block_cwd = "D:/Nasri/Project/closeout-block";
    let block_session = "sess-closeout-block";
    seed_edit_row(&claude_home, block_session, block_cwd);
    seed_sprint(
        &claude_home,
        block_cwd,
        &[("As a dev, I want C, so that Z.", "blocked")],
    );
    let block_stdin = format!("{{\"session_id\":\"{block_session}\"}}");
    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut block_stdin.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        out1_text.contains("additionalContext")
            && out1_text.contains("Do NOT")
            && out1_text.contains("now a hard stop"),
        "STORY_CLOSEOUT_GATE=block must emit the feed-forward hard stop: {out1_text}"
    );

    // Fully-Done sprint -> silent (generic advisory), even under =block.
    let done_cwd = "D:/Nasri/Project/closeout-done";
    let done_session = "sess-closeout-done";
    seed_edit_row(&claude_home, done_session, done_cwd);
    seed_sprint(
        &claude_home,
        done_cwd,
        &[("As a dev, I want D, so that W.", "done")],
    );
    let done_stdin = format!("{{\"session_id\":\"{done_session}\"}}");
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut done_stdin.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        !out2_text.contains("\"decision\""),
        "a fully-Done sprint must not fire the closeout gate: {out2_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

/// Seed a research-cache record file with a fresh mtime so the memory gate
/// sees a durable write this session. Mirrors the `memory/research-cache`
/// layout `keel memory research-cache record` writes to.
fn seed_memory_write(claude_home: &std::path::Path) {
    let dir = claude_home.join("memory").join("research-cache");
    std::fs::create_dir_all(&dir).expect("create research-cache dir");
    std::fs::write(dir.join("rc-1.json"), "{\"id\":\"rc-1\"}").expect("write research record");
}

/// Seed the newest working brief for `workspace_cwd` with `criteria_count`
/// acceptance criteria so the sprint-start gate's multi-story check resolves.
fn seed_brief_with_criteria(
    claude_home: &std::path::Path,
    workspace_cwd: &str,
    criteria_count: usize,
) {
    let criteria: Vec<String> = (0..criteria_count)
        .map(|index| format!("Given X, When Y{index}, Then Z{index}."))
        .collect();
    let brief = crate::utility::working_brief::create_brief(
        format!("wb-sprint-{criteria_count}"),
        "multi-story request".into(),
        Vec::new(),
        criteria,
        Vec::new(),
        workspace_cwd.into(),
        "2026-06-06T00:00:00Z".into(),
    );
    crate::utility::working_brief::write_brief(claude_home, &brief).expect("write brief");
}

/// Seed a template-state generated learned skill plus the trusted instincts it
/// was built from, so `collect_synthesis_briefs` reports one pending brief. The
/// fnv1a-64 here matches the learning loop's marker hash so the skill reads as
/// unrefined (template state). Returns the skill name.
fn seed_pending_learned_skill(claude_home: &std::path::Path, project: &str) -> String {
    fn fnv1a_64(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    let skill_name = format!("learned-{project}");
    let skill_dir = claude_home.join("skills").join(&skill_name);
    std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
    let content =
        format!("---\nname: {skill_name}\ngenerated: true\nprovenance: learned\n---\nbody\n");
    std::fs::write(skill_dir.join("SKILL.md"), &content).expect("write skill");
    let marker = serde_json::json!({
        "generator": "keel-learning",
        "generatedHash": fnv1a_64(content.as_bytes()).to_string(),
        "signatureSet": "cargo test\ngit commit",
        "project": project,
        "predictedSignatures": ["cargo test", "git commit"],
    });
    std::fs::write(
        skill_dir.join(".learning-meta.json"),
        serde_json::to_string_pretty(&marker).unwrap(),
    )
    .expect("write marker");
    let store = crate::utility::record_store::RecordStore::new(claude_home, "memory/instincts");
    for (index, trigger) in ["cargo test", "git commit"].iter().enumerate() {
        let id = format!("inst-{index}");
        let record: crate::utility::record_store::Record = vec![
            ("id".into(), id.clone()),
            ("trigger".into(), (*trigger).into()),
            ("guidance".into(), format!("always run {trigger}")),
            ("confidence".into(), "8".into()),
            ("observations".into(), "8".into()),
            ("sessions".into(), "2".into()),
            ("project".into(), project.into()),
            ("source".into(), "observed".into()),
        ];
        store.write_record(&id, &record).expect("seed instinct");
    }
    skill_name
}

#[test]
fn memory_gate_messages_name_the_switches_and_action() {
    // The memory-gate message must name the clearing action (a memory write)
    // and how to change/disable it, keyed on the emitted decision.
    let nudge = memory_gate_message(GateDecision::Nudge);
    assert!(nudge.contains("CLAUDE_SKILLS_MEMORY_GATE"));
    assert!(nudge.contains("=block"));
    assert!(nudge.contains("=off"));
    assert!(
        nudge.contains("research-cache record")
            && nudge.contains("maintenance append-working-buffer"),
        "nudge message must name the memory-write surfaces that clear the gate"
    );
    assert!(nudge.contains("does not stop the turn"));
    assert!(nudge.contains("escalate"));

    let block = memory_gate_message(GateDecision::Block);
    assert!(block.contains("CLAUDE_SKILLS_MEMORY_GATE"));
    assert!(block.contains("=off"));
    assert!(block.contains("research-cache record"));
    assert!(block.contains("cannot loop") || block.contains("bounded"));
    assert!(block.contains("hard stop"));
}

#[test]
fn sprint_start_gate_messages_name_the_switches_and_action() {
    let nudge = sprint_start_gate_message(GateDecision::Nudge);
    assert!(nudge.contains("CLAUDE_SKILLS_SPRINT_START_GATE"));
    assert!(nudge.contains("=block"));
    assert!(nudge.contains("=off"));
    assert!(
        nudge.contains("keel sprint plan") && nudge.contains("running-a-sprint"),
        "nudge message must name the sprint-plan action and the sprint skill"
    );
    assert!(nudge.contains("does not stop the turn"));
    assert!(nudge.contains("escalate"));

    let block = sprint_start_gate_message(GateDecision::Block);
    assert!(block.contains("CLAUDE_SKILLS_SPRINT_START_GATE"));
    assert!(block.contains("=off"));
    assert!(block.contains("keel sprint plan"));
    assert!(block.contains("cannot loop") || block.contains("bounded"));
    assert!(block.contains("hard stop"));
}

#[test]
fn learned_skill_gate_message_names_switch_and_skill() {
    let briefs = vec![crate::runner::learning::SynthesisBrief {
        skill_name: "learned-demo".into(),
        skill_path: "/skills/learned-demo/SKILL.md".into(),
        project: "demo".into(),
        prompt: "...".into(),
    }];
    let nudge = learned_skill_gate_message(GateDecision::Nudge, &briefs);
    assert!(nudge.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"));
    assert!(nudge.contains("=off"));
    assert!(
        nudge.contains("Skill(\"learned-demo\")"),
        "message must name the learned skill as a load action: {nudge}"
    );
    assert!(
        nudge.contains("never halts the turn"),
        "learned-skill reminder is advisory, never a hard stop"
    );
}

#[test]
fn memory_gate_nudges_when_no_memory_saved_then_satisfied_off_and_capped() {
    // END-TO-END for the memory-save gate. Isolates it by disabling the other
    // gates. Proves: fires when code changed but nothing saved; silent once a
    // memory write exists; silent when off; bounded per session.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-memory");
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
    let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
    let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
    std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
    std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
    std::env::set_var(MEMORY_GATE_ENV_VAR, "nudge");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");

    let session_id = "sess-memory";
    let cwd = "D:/Nasri/Project/memory-e2e";
    seed_edit_row(&claude_home, session_id, cwd);
    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    // Fires: edited code, no memory write → non-blocking nudge naming the gate.
    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default memory gate must NOT block: {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext") && out1_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
        "memory gate must emit a non-blocking nudge naming the gate: {out1_text}"
    );
    let blocks_path = memory_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "memory-gate counter must advance to 1 after the nudge"
    );

    // Cap reached (still no write): falls through to the generic advisory.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        out2_text.contains("Closeout check") && !out2_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
        "second call must fall through to the generic advisory (cap reached): {out2_text}"
    );

    // Satisfied: a fresh session with a memory write present → silent.
    let saved_session = "sess-memory-saved";
    seed_edit_row(&claude_home, saved_session, cwd);
    seed_memory_write(&claude_home);
    let saved_stdin = format!("{{\"session_id\":\"{saved_session}\"}}");
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut saved_stdin.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        !out3_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
        "a memory write this session must satisfy the gate (silent): {out3_text}"
    );

    // Off: a fresh session with no write but MEMORY_GATE=off → silent.
    std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
    let off_session = "sess-memory-off";
    seed_edit_row(&claude_home, off_session, cwd);
    let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
    let mut out4 = Vec::new();
    let mut err4 = Vec::new();
    let code4 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out4, &mut err4);
    let out4_text = String::from_utf8_lossy(&out4);
    assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
    assert!(
        !out4_text.contains("CLAUDE_SKILLS_MEMORY_GATE"),
        "MEMORY_GATE=off must keep the gate silent: {out4_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    for (var, prior) in [
        (REVIEW_GATE_ENV_VAR, previous_review),
        (BRIEF_GATE_ENV_VAR, previous_brief),
        (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
        (SPRINT_START_GATE_ENV_VAR, previous_sprint),
        (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
        (MEMORY_GATE_ENV_VAR, previous_memory),
        (RESEARCH_GATE_ENV_VAR, previous_research),
        (STORY_FIRST_GATE_ENV_VAR, previous_story_first),
    ] {
        match prior {
            Some(value) => std::env::set_var(var, value),
            None => std::env::remove_var(var),
        }
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn sprint_start_gate_nudges_for_multi_story_without_sprint_then_satisfied_off_and_capped() {
    // END-TO-END for the sprint-start gate. Isolates it by disabling the other
    // gates. Proves: fires on multi-story scope with no sprint; silent once a
    // sprint exists; silent for single-story scope; silent when off; bounded.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-sprint-start");
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
    let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
    let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
    std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
    std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
    std::env::set_var(SPRINT_START_GATE_ENV_VAR, "nudge");

    // Multi-story scope, no sprint → fire.
    let cwd = "D:/Nasri/Project/sprint-start-e2e";
    let session_id = "sess-sprint-start";
    seed_edit_row(&claude_home, session_id, cwd);
    seed_brief_with_criteria(&claude_home, cwd, 2);
    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default sprint-start gate must NOT block: {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext")
            && out1_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
        "sprint-start gate must nudge on multi-story scope with no sprint: {out1_text}"
    );
    let blocks_path = sprint_start_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "sprint-start counter must advance to 1 after the nudge"
    );

    // Cap reached: falls through to the generic advisory.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        out2_text.contains("Closeout check")
            && !out2_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
        "second call must fall through to the generic advisory (cap reached): {out2_text}"
    );

    // Satisfied: a multi-story workspace that already has a sprint → silent.
    let with_sprint_cwd = "D:/Nasri/Project/sprint-start-has-sprint";
    let with_sprint_session = "sess-sprint-has";
    seed_edit_row(&claude_home, with_sprint_session, with_sprint_cwd);
    seed_brief_with_criteria(&claude_home, with_sprint_cwd, 2);
    seed_sprint(
        &claude_home,
        with_sprint_cwd,
        &[("As a dev, I want A, so that X.", "todo")],
    );
    let with_sprint_stdin = format!("{{\"session_id\":\"{with_sprint_session}\"}}");
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut with_sprint_stdin.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        !out3_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
        "an existing sprint must satisfy the gate (silent): {out3_text}"
    );

    // Single-story scope (one acceptance criterion) → not multi-story → silent.
    let single_cwd = "D:/Nasri/Project/sprint-start-single";
    let single_session = "sess-sprint-single";
    seed_edit_row(&claude_home, single_session, single_cwd);
    seed_brief_with_criteria(&claude_home, single_cwd, 1);
    let single_stdin = format!("{{\"session_id\":\"{single_session}\"}}");
    let mut out4 = Vec::new();
    let mut err4 = Vec::new();
    let code4 = run_hook_post_tool_batch(&mut single_stdin.as_bytes(), &mut out4, &mut err4);
    let out4_text = String::from_utf8_lossy(&out4);
    assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
    assert!(
        !out4_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
        "single-story scope must keep the gate silent: {out4_text}"
    );

    // Off: a fresh multi-story session but SPRINT_START_GATE=off → silent.
    std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
    let off_session = "sess-sprint-off";
    seed_edit_row(&claude_home, off_session, cwd);
    let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
    let mut out5 = Vec::new();
    let mut err5 = Vec::new();
    let code5 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out5, &mut err5);
    let out5_text = String::from_utf8_lossy(&out5);
    assert_eq!(code5, 0, "stderr: {}", String::from_utf8_lossy(&err5));
    assert!(
        !out5_text.contains("CLAUDE_SKILLS_SPRINT_START_GATE"),
        "SPRINT_START_GATE=off must keep the gate silent: {out5_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    for (var, prior) in [
        (REVIEW_GATE_ENV_VAR, previous_review),
        (BRIEF_GATE_ENV_VAR, previous_brief),
        (RESEARCH_GATE_ENV_VAR, previous_research),
        (STORY_FIRST_GATE_ENV_VAR, previous_story_first),
        (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
        (MEMORY_GATE_ENV_VAR, previous_memory),
        (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
        (SPRINT_START_GATE_ENV_VAR, previous_sprint),
    ] {
        match prior {
            Some(value) => std::env::set_var(var, value),
            None => std::env::remove_var(var),
        }
    }
    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn learned_skill_gate_nudges_when_pending_then_silent_off_and_capped() {
    // END-TO-END for the learned-skill reminder. Isolates it by disabling the
    // other gates. Proves: fires when a template-state learned skill is pending
    // (independent of edits); silent when none pending; silent when off; bounded.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let claude_home = temp_brief_gate_home("e2e-learned");
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    let previous_memory = std::env::var(MEMORY_GATE_ENV_VAR).ok();
    let previous_sprint = std::env::var(SPRINT_START_GATE_ENV_VAR).ok();
    let previous_learned = std::env::var(LEARNED_SKILL_GATE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");
    std::env::set_var(MEMORY_GATE_ENV_VAR, "off");
    std::env::set_var(SPRINT_START_GATE_ENV_VAR, "off");
    std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "nudge");

    // Pending learned skill (no edit row needed — the gate is edit-independent).
    let skill_name = seed_pending_learned_skill(&claude_home, "learnedgate");
    let session_id = "sess-learned";
    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");

    let mut out1 = Vec::new();
    let mut err1 = Vec::new();
    let code1 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out1, &mut err1);
    let out1_text = String::from_utf8_lossy(&out1);
    assert_eq!(code1, 0, "stderr: {}", String::from_utf8_lossy(&err1));
    assert!(
        !out1_text.contains("\"decision\""),
        "default learned-skill gate must NOT block: {out1_text}"
    );
    assert!(
        out1_text.contains("additionalContext")
            && out1_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE")
            && out1_text.contains(&format!("Skill(\\\"{skill_name}\\\")")),
        "learned-skill gate must name the pending skill as a load action: {out1_text}"
    );
    let blocks_path = learned_skill_gate_blocks_path(&claude_home, session_id);
    assert_eq!(
        read_counter_value(&blocks_path),
        1,
        "learned-skill counter must advance to 1 after the nudge"
    );

    // Cap reached (skill still pending): falls through to the generic advisory.
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let code2 = run_hook_post_tool_batch(&mut stdin_json.as_bytes(), &mut out2, &mut err2);
    let out2_text = String::from_utf8_lossy(&out2);
    assert_eq!(code2, 0, "stderr: {}", String::from_utf8_lossy(&err2));
    assert!(
        out2_text.contains("Closeout check")
            && !out2_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
        "second call must fall through to the generic advisory (cap reached): {out2_text}"
    );

    // Satisfied: a fresh home with no learned skills pending → silent.
    let empty_home = temp_brief_gate_home("e2e-learned-empty");
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &empty_home);
    let empty_session = "sess-learned-empty";
    let empty_stdin = format!("{{\"session_id\":\"{empty_session}\"}}");
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let code3 = run_hook_post_tool_batch(&mut empty_stdin.as_bytes(), &mut out3, &mut err3);
    let out3_text = String::from_utf8_lossy(&out3);
    assert_eq!(code3, 0, "stderr: {}", String::from_utf8_lossy(&err3));
    assert!(
        !out3_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
        "no pending learned skill must keep the gate silent: {out3_text}"
    );

    // Off: pending skill present but LEARNED_SKILL_GATE=off → silent.
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(LEARNED_SKILL_GATE_ENV_VAR, "off");
    let off_session = "sess-learned-off";
    let off_stdin = format!("{{\"session_id\":\"{off_session}\"}}");
    let mut out4 = Vec::new();
    let mut err4 = Vec::new();
    let code4 = run_hook_post_tool_batch(&mut off_stdin.as_bytes(), &mut out4, &mut err4);
    let out4_text = String::from_utf8_lossy(&out4);
    assert_eq!(code4, 0, "stderr: {}", String::from_utf8_lossy(&err4));
    assert!(
        !out4_text.contains("CLAUDE_SKILLS_LEARNED_SKILL_GATE"),
        "LEARNED_SKILL_GATE=off must keep the gate silent: {out4_text}"
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    for (var, prior) in [
        (REVIEW_GATE_ENV_VAR, previous_review),
        (BRIEF_GATE_ENV_VAR, previous_brief),
        (STORY_CLOSEOUT_GATE_ENV_VAR, previous_closeout),
        (MEMORY_GATE_ENV_VAR, previous_memory),
        (SPRINT_START_GATE_ENV_VAR, previous_sprint),
        (LEARNED_SKILL_GATE_ENV_VAR, previous_learned),
    ] {
        match prior {
            Some(value) => std::env::set_var(var, value),
            None => std::env::remove_var(var),
        }
    }
    let _ = std::fs::remove_dir_all(&claude_home);
    let _ = std::fs::remove_dir_all(&empty_home);
}

#[test]
fn edit_counter_increments_and_resets_at_threshold() {
    // The counter file is the bridge between PostToolUse fires (one per
    // tool call) and the periodic SYSTEM_MAP refresh. Verify the file
    // round-trips correctly so the threshold check in run_hook_post_tool_use
    // sees the right value.
    let dir = std::env::temp_dir().join(format!("keel-edit-counter-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let counter = dir.join("counter");

    for expected in 1..=3 {
        let next = increment_counter_file(&counter).unwrap();
        assert_eq!(next, expected);
    }

    reset_counter_file(&counter).unwrap();
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "0");

    let after_reset = increment_counter_file(&counter).unwrap();
    assert_eq!(after_reset, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn edit_class_tools_match_documented_set() {
    // Only edit-class tools should bump the counter; read-only tools must
    // not, otherwise the SYSTEM_MAP refresh fires on every Read/Grep too.
    for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
        assert!(
            is_edit_class_tool(tool),
            "{tool} should count as edit-class"
        );
    }
    for tool in ["Read", "Grep", "Glob", "Bash", "Task"] {
        assert!(
            !is_edit_class_tool(tool),
            "{tool} must not count as edit-class"
        );
    }
}

#[test]
fn session_start_context_embeds_bootstrap_skill_and_memory_pointer() {
    // SessionStart is the documented entry point for delivering durable
    // model context, so it carries the bootstrap skill (iron law + Red
    // Flags + skill catalog) plus the runtime-resolved workspace memory
    // pointer. Both pieces have to be there: the skill delivers the
    // operating contract, the pointer delivers the workspace-specific
    // memory path that CLAUDE.md cannot know in advance.
    let context = session_start_context();
    // Bootstrap skill markers — these come from
    // <repo>/using-keel/SKILL.md via include_str! and are what
    // make the model treat skill invocation as non-optional.
    assert!(
        context.contains("EXTREMELY_IMPORTANT"),
        "SessionStart must embed the bootstrap skill iron-law block"
    );
    assert!(
        context.contains("Trust the codebase, not your knowledge base"),
        "SessionStart must restate the trust-the-codebase rule"
    );
    // The four rules must be labeled with the literal phrase "Iron Law" in
    // the always-loaded SessionStart channel. Regression guard for the bug
    // where the contract WAS in context but never named: an agent scanning
    // its context for "iron law" found nothing because the bootstrap only
    // said "operating contract"/"EXTREMELY_IMPORTANT", so it answered "no
    // iron law in my context" even though the rules were right there. The
    // name is the lookup key the user (and the model) search for.
    assert!(
            context.contains("Iron Law"),
            "SessionStart must label the four rules with the literal phrase \"Iron Law\" so an agent asked whether the Iron Law is in context can find it by name"
        );
    assert!(
        context.contains("Red Flags"),
        "SessionStart must embed the Red Flags rationalization table"
    );
    // Catalog spot-check: a couple of representative skill names so the
    // model knows what is invokable. Full enumeration lives in the skill
    // file; this assertion just guards that the catalog survived the
    // include.
    assert!(
        context.contains("preserve-existing-flow"),
        "SessionStart skill catalog must list preserve-existing-flow"
    );
    assert!(
        context.contains("reviewer"),
        "SessionStart skill catalog must list the reviewer skill"
    );

    // Runtime memory pointer.
    assert!(
        context.contains("Workspace memory system map"),
        "SessionStart must include the runtime memory pointer"
    );

    // Memory-writes section. Auto-refresh on PreCompact/SessionEnd
    // covers SYSTEM_MAP only; working-brief writes are still on the
    // agent. The bootstrap skill teaches when to call the four real
    // memory subcommands; this assertion guards that block from being
    // silently deleted in a future edit.
    assert!(
        context.contains("Memory writes (when you learn something durable)"),
        "SessionStart must embed the memory-writes instruction block"
    );
    assert!(
        context.contains("keel memory working-brief write"),
        "SessionStart memory-writes block must name the working-brief write surface"
    );
    assert!(
        context.contains("keel memory completion-gate check"),
        "SessionStart memory-writes block must name the completion-gate probe"
    );

    // Implementation-discipline pillars — the bootstrap skill carries
    // the full Code Implementation Discipline section so the model
    // gets the four pillars on every session start, not only when an
    // on-demand skill match fires. Each pillar name is asserted so a
    // future trim of the SKILL.md cannot silently drop them.
    assert!(
        context.contains("Think Before Coding"),
        "SessionStart must embed the Think Before Coding pillar"
    );
    assert!(
        context.contains("Simplicity First"),
        "SessionStart must embed the Simplicity First pillar"
    );
    assert!(
        context.contains("Surgical Changes"),
        "SessionStart must embed the Surgical Changes pillar"
    );
    assert!(
        context.contains("Goal-Driven Execution"),
        "SessionStart must embed the Goal-Driven Execution pillar"
    );

    // Root-cause deep-dive guard — the bootstrap must teach that
    // suspicion is a hypothesis, not a finding, so the model does not
    // jump from "this looks like the cause" to a patch. The exact
    // phrasing lives in using-keel/SKILL.md; this assertion
    // protects the rule from being silently trimmed during a future
    // edit of the bootstrap.
    assert!(
        context.contains("Suspicion is a hypothesis, not a finding"),
        "SessionStart must restate that suspicion is a hypothesis, not a finding"
    );
    assert!(
        context.contains("Oh this may be the case"),
        "SessionStart Red Flags must name the \"Oh this may be the case\" jump"
    );
}

#[test]
fn session_start_context_stays_under_truncation_cap() {
    // The bug this guards: the harness truncates hook
    // `hookSpecificOutput.additionalContext` once it crosses ~10KB,
    // persisting the full text to a tool-results file and injecting only a
    // ~2KB preview + a file pointer the model never reads back. Verified
    // against live session transcripts — a 27.6KB SessionStart context was
    // replaced by a 2KB preview while a 5.9KB UserPromptSubmit context landed
    // intact. The previous implementation embedded the full 27KB
    // using-keel/SKILL.md here, so the operating contract was silently
    // truncated to its first ~2KB in every project: the model never saw the
    // later iron-law rules, the discipline pillars, the MCP tools, or the
    // memory writers.
    //
    // The contract MUST fit in full. We assert a conservative 9KB ceiling on
    // the UTF-8 byte length — below the observed ~10KB cap with headroom for
    // the appended runtime memory pointer. If a future edit grows the
    // bootstrap past this, it re-introduces the truncation bug, so the bound
    // fails loudly instead of shipping a contract the model cannot see.
    const TRUNCATION_CEILING_BYTES: usize = 9 * 1024;
    // Isolate the home so the base measures ONLY the compact bootstrap, not
    // this machine's accumulated instinct/synthesis digest for whatever
    // project the test happens to run in. session_start_context() appends
    // project_instinct_digest / project_synthesis_nudge from the resolved
    // ~/.claude home; without isolation a developer's real home (or this
    // repo's own growing instinct store, now that failures are captured too)
    // inflates the base so the worst-case assertion fails locally while
    // passing on a clean CI home. Point CLAUDE_TARGET_OVERRIDE at an empty
    // temp dir under the shared ENV_LOCK so the measurement is deterministic
    // everywhere.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let empty_home = temp_brief_gate_home("truncation-cap");
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &empty_home);

    let context = session_start_context();

    match &previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    let _ = std::fs::remove_dir_all(&empty_home);

    let byte_len = context.len();
    assert!(
            byte_len < TRUNCATION_CEILING_BYTES,
            "SessionStart context is {byte_len} bytes, at/over the {TRUNCATION_CEILING_BYTES}-byte ceiling — the harness truncates additionalContext above ~10KB, so the operating contract would be cut off mid-way and the model would never see the full iron law. Trim the compact bootstrap or move detail into the on-demand Skill(\"using-keel\") body."
        );

    // Major-3 guard: in a fresh test env `workspace_memory_digest()` is
    // empty, so the line above only certifies the base context. But at
    // runtime the digest is appended and is independently bounded by
    // WORKSPACE_DIGEST_MAX_BYTES. Certify the WORST CASE — base context plus
    // a maxed-out digest — still clears the ceiling, so a future bootstrap
    // growth that would overflow once the digest is present fails loudly
    // here instead of silently truncating in production.
    let worst_case = byte_len + WORKSPACE_DIGEST_MAX_BYTES;
    assert!(
            worst_case < TRUNCATION_CEILING_BYTES,
            "SessionStart base ({byte_len} B) + a maxed workspace digest ({WORKSPACE_DIGEST_MAX_BYTES} B) = {worst_case} B would cross the {TRUNCATION_CEILING_BYTES}-byte ceiling. Shrink the bootstrap or WORKSPACE_DIGEST_MAX_BYTES so the pushed digest can never truncate the iron law."
        );
}

#[test]
fn workspace_memory_digest_pushes_real_content_and_stays_bounded() {
    // s2: the digest must PUSH actual content (system-map head + newest
    // brief + most recent memory note), not just a pointer, and stay within
    // its byte budget so the SessionStart ceiling is never threatened.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!(
        "ws-digest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let base = std::env::temp_dir().join(unique);
    let claude_home = base.join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();

    // The digest reads the map by the same path helper the production code
    // uses, keyed off the current working directory. Drive cwd to a stable
    // workspace and seed a SYSTEM_MAP.md there.
    let workspace = base.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let previous_cwd = std::env::current_dir().ok();
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_current_dir(&workspace).unwrap();

    // 1. Seed the system map at the workspace-keyed reference path.
    if let Some(map_path) = memory_system_map_path_for_workspace(&std::env::current_dir().unwrap())
    {
        std::fs::create_dir_all(map_path.parent().unwrap()).unwrap();
        std::fs::write(
            &map_path,
            "# SYSTEM MAP\n\nThis repo is the WIDGET-FACTORY service.\nEntry: src/main.rs\n",
        )
        .unwrap();
    }

    // 2. Seed a working brief tagged for this workspace.
    let workspace_display = display_path(&std::env::current_dir().unwrap());
    let brief = crate::utility::working_brief::create_brief(
        "wb-digesttest".to_string(),
        "Ship the FROBNICATE endpoint".to_string(),
        vec![],
        vec!["frobnicate returns 200".to_string()],
        vec![],
        workspace_display,
        "2026-06-13T00:00:00Z".to_string(),
    );
    crate::utility::working_brief::write_brief(&claude_home, &brief).unwrap();

    // 3. Seed a recent memory note.
    crate::utility::memory_families::run_memory_family_command(
        "memory",
        "research-cache",
        &[
            "record".to_string(),
            "--question".to_string(),
            "What was last done in WIDGET-FACTORY?".to_string(),
            "--answer".to_string(),
            "Wired the GIZMO cache layer".to_string(),
        ],
        &mut std::io::sink(),
        &mut std::io::sink(),
    );

    let digest = workspace_memory_digest();

    // Restore env/cwd before assertions.
    if let Some(cwd) = previous_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }

    // Real content from all three sources is PUSHED, not pointed at.
    assert!(
        digest.contains("WIDGET-FACTORY"),
        "digest must embed the actual system-map head: {digest}"
    );
    assert!(
        digest.contains("FROBNICATE"),
        "digest must embed the actual working-brief request: {digest}"
    );
    assert!(
        digest.contains("GIZMO"),
        "digest must embed the actual most-recent memory note: {digest}"
    );
    // Bounded.
    assert!(
        digest.len() <= WORKSPACE_DIGEST_MAX_BYTES + 40,
        "digest length {} exceeds its byte budget",
        digest.len()
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn truncate_on_line_boundary_cuts_at_newline_and_marks_elision() {
    let text = "alpha line\nbeta line\ngamma line\ndelta line\n";
    // Cap mid-"gamma": must cut back to the end of "beta line".
    let cut = truncate_on_line_boundary(text, 25);
    assert!(cut.starts_with("alpha line\nbeta line"));
    assert!(cut.contains("[truncated]"));
    assert!(!cut.contains("gamma"));
    // Under cap → returned unchanged.
    assert_eq!(truncate_on_line_boundary("short", 100), "short");
}

#[test]
fn truncate_on_line_boundary_does_not_panic_on_multibyte_at_cap() {
    // Blocker regression: the earlier local impl sliced `&str` by raw byte
    // index (`&text[..max_bytes]`), which panics when a multibyte char
    // straddles the cap. Workspace map / brief / note text routinely carries
    // em-dashes, ellipses, smart quotes, and arrows, so this is a real
    // SessionStart panic path. Build a single line (no newline in range, so
    // the char-boundary fallback is exercised) packed with em-dashes and set
    // a cap that lands inside one. Must return a truncated string, not panic.
    let text = "—".repeat(200); // each '—' is 3 UTF-8 bytes; no newline
    for cap in [10usize, 25, 31, 100, 199] {
        let out = truncate_on_line_boundary(&text, cap);
        // Did not panic, stayed within budget + the marker allowance, and
        // never split a char (valid UTF-8 by construction of the return type).
        assert!(out.len() <= cap + 32, "cap {cap}: len {}", out.len());
    }
    // A CJK line (3-byte chars) with the cut inside a character, too.
    let cjk = "你好世界你好世界你好世界"; // 12 chars × 3 bytes = 36 bytes
    let out = truncate_on_line_boundary(cjk, 10);
    assert!(out.contains("[truncated]"));
}

#[test]
fn top_level_only_hooks_use_system_message_not_hook_specific_output() {
    // Per code.claude.com/docs/en/hooks, every event row carries a
    // `supports_hook_specific_output` flag. Events with `true` accept
    // `hookSpecificOutput.additionalContext`; everything else must use
    // top-level fields like `systemMessage`. This test exercises the
    // wrapper directly with a non-empty context for *every* event so
    // both branches are reached. The earlier version called
    // `run_hook_lifecycle` with five hand-picked events that all
    // produce empty stdout in tests, so the assertions never ran and
    // a regression in either branch would have shipped silently.
    const SAMPLE_CONTEXT: &str = "non-empty test payload";

    for event in HOOK_EVENTS {
        let payload = render_lifecycle_payload(event, SAMPLE_CONTEXT);

        if event.supports_hook_specific_output {
            let hook_specific = payload
                .get("hookSpecificOutput")
                .unwrap_or_else(|| panic!("{} must emit hookSpecificOutput", event.name));
            assert_eq!(
                hook_specific
                    .get("hookEventName")
                    .and_then(JsonDocument::as_str),
                Some(event.name),
                "{}: hookSpecificOutput.hookEventName must match the event row",
                event.name
            );
            assert_eq!(
                hook_specific
                    .get("additionalContext")
                    .and_then(JsonDocument::as_str),
                Some(SAMPLE_CONTEXT),
                "{}: hookSpecificOutput.additionalContext must carry the context",
                event.name
            );
            assert!(
                    payload.get("systemMessage").is_none(),
                    "{}: schema-supported events must not duplicate context into top-level systemMessage",
                    event.name
                );
        } else {
            assert!(
                    payload.get("hookSpecificOutput").is_none(),
                    "{}: top-level-only events must not emit hookSpecificOutput — the official harness schema documents top-level decision fields only for this event",
                    event.name
                );
            assert_eq!(
                payload.get("systemMessage").and_then(JsonDocument::as_str),
                Some(SAMPLE_CONTEXT),
                "{}: top-level-only events must wrap context in systemMessage",
                event.name
            );
        }

        assert_eq!(
                payload.get("suppressOutput").and_then(JsonDocument::as_bool),
                Some(true),
                "{}: every payload must set suppressOutput=true so plain stdout doesn't leak into the transcript",
                event.name
            );
    }
}

#[test]
fn session_start_emits_hook_specific_output_additional_context() {
    // SessionStart is the documented entry point for delivering durable
    // model context via `hookSpecificOutput.additionalContext` per
    // code.claude.com/docs/en/hooks. The bootstrap skill must land in
    // that field, not in the user-facing top-level `systemMessage`
    // warning slot. The inner-string assertions live in
    // `session_start_context_embeds_bootstrap_skill_and_memory_pointer`;
    // this test pins the wrapper shape.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_lifecycle("session-start", &mut stdout, &mut stderr);

    assert_eq!(
        code,
        0,
        "stderr for session-start: {}",
        String::from_utf8_lossy(&stderr)
    );

    let output: JsonDocument = serde_json::from_slice(&stdout).expect("valid JSON");

    let event_name = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("hookEventName"))
        .and_then(JsonDocument::as_str)
        .expect("SessionStart must emit hookSpecificOutput.hookEventName");
    assert_eq!(event_name, "SessionStart");

    let context = output
        .get("hookSpecificOutput")
        .and_then(|node| node.get("additionalContext"))
        .and_then(JsonDocument::as_str)
        .expect("SessionStart must emit hookSpecificOutput.additionalContext");
    assert!(
        !context.trim().is_empty(),
        "SessionStart additionalContext must not be empty"
    );

    assert!(
            output.get("systemMessage").is_none(),
            "SessionStart must not emit top-level systemMessage — additionalContext is the documented vehicle for model-context injection"
        );
}

#[test]
fn session_start_dispatch_self_heals_drifted_mcp_registration() {
    // End-to-end through the production entry point: a SessionStart hook
    // dispatched via run_hook_command must repair a drifted ~/.claude.json
    // (an entry missing `alwaysLoad`) without the user running
    // install/update/repair. This is the fix for the "binary swapped without
    // re-registering" drift vector. We isolate the home via
    // CLAUDE_TARGET_OVERRIDE pointed at a real `.claude` dir so the self-heal
    // is active (it skips non-`.claude` homes) yet never touches the real one.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!(
        "session-start-selfheal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let base = std::env::temp_dir().join(unique);
    // The home must be literally `.claude` for the self-heal to engage.
    let claude_home = base.join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    // ~/.claude.json lives beside the .claude dir (parent), per
    // mcp_config_path — resolve it the same way the production code does.
    let config_path = crate::manager::mcp_register::mcp_config_path(&claude_home);
    // Seed a DRIFTED entry: present but missing alwaysLoad.
    std::fs::write(
            &config_path,
            r#"{"mcpServers":{"keel":{"type":"stdio","command":"old","args":["mcp","serve"],"env":{}}}}"#,
        )
        .unwrap();

    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_self_heal = std::env::var(MCP_SELF_HEAL_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::remove_var(MCP_SELF_HEAL_ENV_VAR); // default → on

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_hook_command(&["session-start".to_string()], &mut stdout, &mut stderr);

    // Restore env before assertions so a failure cannot leak the override.
    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_self_heal {
        Some(value) => std::env::set_var(MCP_SELF_HEAL_ENV_VAR, value),
        None => std::env::remove_var(MCP_SELF_HEAL_ENV_VAR),
    }

    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    // The SessionStart context still renders (load-bearing work survives).
    let output: JsonDocument =
        serde_json::from_slice(&stdout).expect("SessionStart still emits valid JSON");
    assert_eq!(
        output["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "self-heal must not disturb the SessionStart context render"
    );
    // And the drifted entry is now repaired with alwaysLoad:true.
    let parsed: JsonDocument =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        parsed["mcpServers"]["keel"]["alwaysLoad"],
        serde_json::json!(true),
        "SessionStart dispatch must repair the drifted entry to carry alwaysLoad:true"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn session_start_dispatch_respects_self_heal_off_switch() {
    // The off switch must fully disable the write so an operator (or a test)
    // can opt out. With it off, a drifted entry stays drifted.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!(
        "session-start-selfheal-off-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let base = std::env::temp_dir().join(unique);
    let claude_home = base.join(".claude");
    std::fs::create_dir_all(&claude_home).unwrap();
    let config_path = crate::manager::mcp_register::mcp_config_path(&claude_home);
    let drifted = r#"{"mcpServers":{"keel":{"type":"stdio","command":"old","args":["mcp","serve"],"env":{}}}}"#;
    std::fs::write(&config_path, drifted).unwrap();

    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_self_heal = std::env::var(MCP_SELF_HEAL_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::set_var(MCP_SELF_HEAL_ENV_VAR, "off");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_hook_command(&["session-start".to_string()], &mut stdout, &mut stderr);

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_self_heal {
        Some(value) => std::env::set_var(MCP_SELF_HEAL_ENV_VAR, value),
        None => std::env::remove_var(MCP_SELF_HEAL_ENV_VAR),
    }

    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    // The off switch left the drifted entry untouched, byte for byte.
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        drifted,
        "with the self-heal off, the drifted entry must be left exactly as-is"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn session_end_dispatch_auto_captures_work_summary_to_memory() {
    // s5: SessionEnd must auto-write a recallable work summary built from this
    // session's edit-class observations, and that write must be searchable
    // immediately (it routes through the research-cache record path, which
    // s4 made index-syncing). We isolate the home via CLAUDE_TARGET_OVERRIDE.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!(
        "session-end-capture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let claude_home = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&claude_home).unwrap();

    // Seed this session's observation rows: two edits and one command.
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let obs_dir = claude_home.join("state").join("observations");
    std::fs::create_dir_all(&obs_dir).unwrap();
    let session_id = "sess-capture-1";
    let cwd = "D:/Nasri/Project/capture-demo";
    let rows = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Edit", "signature": "edit:rs", "detail": "src/lib.rs"}),
        serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Edit", "signature": "edit:md", "detail": "README.md"}),
        serde_json::json!({"recorded_at_ms": now_ms(), "session_id": session_id, "cwd": cwd, "tool_name": "Bash", "signature": "cargo test", "detail": "cargo test"}),
    );
    std::fs::write(obs_dir.join(format!("{date}.jsonl")), rows).unwrap();

    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_capture = std::env::var(SESSION_CAPTURE_ENV_VAR).ok();
    // SessionEnd's lifecycle path also runs learning; keep it off so the test
    // is scoped to the capture behavior only.
    let previous_learning = std::env::var("CLAUDE_SKILLS_LEARNING").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);
    std::env::remove_var(SESSION_CAPTURE_ENV_VAR); // default → on
    std::env::set_var("CLAUDE_SKILLS_LEARNING", "off");

    let stdin_json = format!("{{\"session_id\":\"{session_id}\"}}");
    // Drive the REAL dispatch body with injected stdin (Major 2 fix): this
    // exercises the production "session-end" arm ordering — capture before
    // the lifecycle side effects — not just the helper in isolation.
    let code = run_hook_session_end(
        &mut stdin_json.as_bytes(),
        &mut std::io::sink(),
        &mut std::io::sink(),
    );
    assert_eq!(code, 0, "session-end dispatch must exit 0");

    // Restore env before assertions.
    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_capture {
        Some(value) => std::env::set_var(SESSION_CAPTURE_ENV_VAR, value),
        None => std::env::remove_var(SESSION_CAPTURE_ENV_VAR),
    }
    match previous_learning {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_LEARNING", value),
        None => std::env::remove_var("CLAUDE_SKILLS_LEARNING"),
    }

    // A research-cache record must now exist carrying the summary.
    let rc_dir = claude_home.join("memory").join("research-cache");
    let mut found_summary = false;
    if let Ok(entries) = std::fs::read_dir(&rc_dir) {
        for entry in entries.flatten() {
            let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if body.contains("Edited 2 file(s)")
                && body.contains("rs")
                && body.contains("md")
                && body.contains("cargo test")
            {
                found_summary = true;
            }
        }
    }
    assert!(
        found_summary,
        "SessionEnd must write a research-cache record summarizing the 2 edits + cargo test"
    );

    // And it must be immediately recallable (s4 index sync on the write path).
    let hit = crate::utility::recall::search_recall_index(&claude_home, "Edited file", 20, None)
        .expect("recall search runs");
    assert!(
        hit.map(|result| !result.hits.is_empty()).unwrap_or(false),
        "the auto-captured summary must be recallable right after SessionEnd"
    );

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn session_end_capture_is_silent_without_edits_and_respects_off_switch() {
    // Two guarantees: (1) a session that edited nothing produces no summary
    // (no memory pollution from research/question turns); (2) the off switch
    // fully disables capture even when edits exist.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!(
        "session-end-capture-off-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let claude_home = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&claude_home).unwrap();
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let obs_dir = claude_home.join("state").join("observations");
    std::fs::create_dir_all(&obs_dir).unwrap();

    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    let previous_capture = std::env::var(SESSION_CAPTURE_ENV_VAR).ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &claude_home);

    // Case 1: command-only session (no edits) with capture ON → silent.
    std::env::remove_var(SESSION_CAPTURE_ENV_VAR);
    let read_only_session = "sess-readonly";
    std::fs::write(
            obs_dir.join(format!("{date}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({"recorded_at_ms": now_ms(), "session_id": read_only_session, "cwd": "D:/x", "tool_name": "Bash", "signature": "cargo test", "detail": "cargo test"})
            ),
        )
        .unwrap();
    maybe_capture_session_summary(
        &mut format!("{{\"session_id\":\"{read_only_session}\"}}").as_bytes(),
        &mut std::io::sink(),
    );
    assert!(
        !claude_home.join("memory").join("research-cache").exists()
            || std::fs::read_dir(claude_home.join("memory").join("research-cache"))
                .map(|mut e| e.next().is_none())
                .unwrap_or(true),
        "a no-edit session must write no summary"
    );

    // Case 2: edits exist but capture is OFF → still no summary.
    std::env::set_var(SESSION_CAPTURE_ENV_VAR, "off");
    let edit_session = "sess-edits-off";
    std::fs::write(
            obs_dir.join(format!("{date}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({"recorded_at_ms": now_ms(), "session_id": edit_session, "cwd": "D:/x", "tool_name": "Edit", "signature": "edit:rs", "detail": "src/lib.rs"})
            ),
        )
        .unwrap();
    maybe_capture_session_summary(
        &mut format!("{{\"session_id\":\"{edit_session}\"}}").as_bytes(),
        &mut std::io::sink(),
    );

    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }
    match previous_capture {
        Some(value) => std::env::set_var(SESSION_CAPTURE_ENV_VAR, value),
        None => std::env::remove_var(SESSION_CAPTURE_ENV_VAR),
    }

    let rc_dir = claude_home.join("memory").join("research-cache");
    let wrote_anything = std::fs::read_dir(&rc_dir)
        .map(|mut e| e.next().is_some())
        .unwrap_or(false);
    assert!(
        !wrote_anything,
        "with the off switch set, no summary must be written even when edits exist"
    );

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn hook_help_lists_every_official_event_slug() {
    // Regression guard: an earlier hand-maintained help string was
    // missing 14 of the 29 official slugs. Anyone running
    // `keel hook` to discover what's available saw a partial
    // list even though every slug dispatched. Generate the help line
    // from HOOK_EVENTS so the "advertised == dispatched" invariant is
    // structural rather than habit.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run_hook_command(&[], &mut stdout, &mut stderr);
    // No-args invocation prints help and exits 1 — that's intentional
    // so a misconfigured pipeline can't silently no-op.
    assert_eq!(exit, 1);
    let rendered = String::from_utf8(stdout).expect("help is UTF-8");
    for event in HOOK_EVENTS {
        assert!(
            rendered.contains(event.slug),
            "hook help is missing slug `{}`; rendered: {rendered}",
            event.slug
        );
    }
    // Admin verbs must also be present.
    for verb in [
        "install",
        "uninstall",
        "list",
        "show",
        "instructions",
        "diagnose",
    ] {
        assert!(
            rendered.contains(verb),
            "hook help is missing admin verb `{verb}`; rendered: {rendered}"
        );
    }
    let _ = stderr;
}

#[test]
fn diagnose_reports_healthy_when_settings_point_at_installed_executable() {
    let claude_home =
        std::env::temp_dir().join(format!("keel-diagnose-healthy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&claude_home);
    std::fs::create_dir_all(&claude_home).unwrap();

    let executable = crate::runtime::installed_executable_path(&claude_home);
    std::fs::write(&executable, b"installed").unwrap();

    let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let payload = build_hooks_payload(&settings_path, &executable).unwrap();
    std::fs::write(&settings_path, &payload).unwrap();

    let report = collect_hook_diagnostics(&claude_home);

    assert!(
        report.healthy(),
        "expected healthy diagnose, got {report:?}"
    );
    assert_eq!(report.settings_parses, Some(true));
    assert_eq!(report.settings_points_at_installed, Some(true));
    assert!(report.orphan_executable_siblings.is_empty());

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn diagnose_flags_settings_pointing_at_wrong_executable() {
    let claude_home =
        std::env::temp_dir().join(format!("keel-diagnose-mismatch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&claude_home);
    std::fs::create_dir_all(&claude_home).unwrap();

    let executable = crate::runtime::installed_executable_path(&claude_home);
    std::fs::write(&executable, b"installed").unwrap();

    // settings.json points at a different binary (the historical
    // ~/.claude/keel.exe.stale-* leakage shape, where the hook
    // was registered against an old path that no longer exists).
    let other_path = claude_home
        .join("elsewhere")
        .join(crate::runtime::executable_file_name());
    std::fs::create_dir_all(other_path.parent().unwrap()).unwrap();
    let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let payload = build_hooks_payload(&settings_path, &other_path).unwrap();
    std::fs::write(&settings_path, &payload).unwrap();

    let report = collect_hook_diagnostics(&claude_home);

    assert!(!report.healthy(), "expected unhealthy diagnose");
    assert_eq!(report.settings_points_at_installed, Some(false));

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn diagnose_flags_orphan_siblings_as_unhealthy() {
    let claude_home =
        std::env::temp_dir().join(format!("keel-diagnose-orphan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&claude_home);
    std::fs::create_dir_all(&claude_home).unwrap();

    let executable = crate::runtime::installed_executable_path(&claude_home);
    std::fs::write(&executable, b"installed").unwrap();

    let settings_path = claude_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let payload = build_hooks_payload(&settings_path, &executable).unwrap();
    std::fs::write(&settings_path, &payload).unwrap();

    // Drop a legacy stale sibling.
    let orphan = executable.with_file_name(format!(
        "{}.stale-1778857819",
        crate::runtime::executable_file_name()
    ));
    std::fs::write(&orphan, b"legacy").unwrap();

    let report = collect_hook_diagnostics(&claude_home);

    assert!(!report.healthy(), "orphan sibling must mark unhealthy");
    assert_eq!(report.orphan_executable_siblings.len(), 1);

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn diagnose_text_output_lists_failures() {
    let claude_home =
        std::env::temp_dir().join(format!("keel-diagnose-text-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&claude_home);
    std::fs::create_dir_all(&claude_home).unwrap();

    // No installed executable, no settings.json — every check fails.
    let report = collect_hook_diagnostics(&claude_home);
    let mut output = Vec::new();
    report.render_text(&mut output);
    let rendered = String::from_utf8(output).unwrap();

    assert!(rendered.contains("[FAIL]"));
    assert!(rendered.contains("issues found"));

    let _ = std::fs::remove_dir_all(&claude_home);
}

#[test]
fn stop_and_subagent_stop_short_circuit_at_dispatch() {
    // Stop and SubagentStop must always exit 0 AND emit no stdout. Two
    // distinct hazards this guards against:
    //   1. A non-zero exit makes the harness re-run the turn (stop cascade).
    //   2. Any stdout carrying hookSpecificOutput.additionalContext on a Stop
    //      hook means "keep going" — so emitting it makes the agent loop
    //      forever. This was the PR #121 regression; the dispatch arm now
    //      short-circuits both events to exit 0 with no output.
    for subcommand in ["stop", "subagent-stop"] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = run_hook_command(&[subcommand.to_string()], &mut stdout, &mut stderr);

        assert_eq!(
            code,
            0,
            "{subcommand} must always exit 0; stderr: {}",
            String::from_utf8_lossy(&stderr)
        );

        assert!(
            stdout.is_empty(),
            "{subcommand} must emit no stdout (additionalContext would loop the turn); got: {}",
            String::from_utf8_lossy(&stdout)
        );
    }
}

#[test]
fn notification_emits_bell_terminal_sequence() {
    // CC 2.1.141: Notification fires when the harness wants the user's
    // attention (permission prompt, idle reminder). Our handler emits a
    // top-level `terminalSequence` carrying the BEL so the user hears
    // it even when the terminal is in the background. The output also
    // sets `suppressOutput` so the bell is the only visible side
    // effect. The hook must always exit 0 — a non-zero notification
    // exit is treated as a permission denial in some CC builds.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_hook_command(&["notification".to_string()], &mut stdout, &mut stderr);

    assert_eq!(
        code,
        0,
        "notification must exit 0; stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(
        stderr.is_empty(),
        "notification must emit no stderr; got: {}",
        String::from_utf8_lossy(&stderr)
    );

    // Stdout is one JSON object terminated by a newline.
    let rendered = String::from_utf8(stdout).expect("notification output is UTF-8");
    let trimmed = rendered.strip_suffix('\n').unwrap_or(&rendered);

    let parsed: JsonDocument =
        serde_json::from_str(trimmed).expect("notification output is valid JSON");
    assert_eq!(
        parsed.get("suppressOutput").and_then(JsonDocument::as_bool),
        Some(true),
        "notification must set suppressOutput so the row stays out of the transcript",
    );
    assert_eq!(
        parsed
            .get("terminalSequence")
            .and_then(JsonDocument::as_str),
        Some("\u{0007}"),
        "terminalSequence must be the BEL byte (CC 2.1.141 allowlist)",
    );
}

#[test]
fn memory_key_sanitization_matches_scope_command_shape() {
    let key = sanitize_memory_key(r#"C:\Users\riezh\OneDrive\Documents\test\claude_core"#);

    assert_eq!(key, "c-users-riezh-onedrive-documents-test-claude-core");
}

fn temp_hook_path(name: &str) -> PathBuf {
    let unique = format!("{}-{}", name, std::process::id());

    std::env::temp_dir().join(unique).join("settings.json")
}

#[test]
fn hook_install_honors_claude_home_flag_and_never_touches_real_home() {
    // DEFECT 1 regression: `hook install` previously hardcoded
    // resolve_claude_home("") and so always wrote the real ~/.claude,
    // ignoring --claude-home. A probe that believed it was isolated
    // rewrote the user's live settings.json. This test pins the fix:
    // --claude-home must route the write to the requested dir, and the
    // env-resolved "real" home (CLAUDE_TARGET_OVERRIDE) must be untouched.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!("hook-install-isolation-{}", std::process::id());
    let base = std::env::temp_dir().join(unique);
    let isolated_home = base.join("isolated");
    let sentinel_real_home = base.join("sentinel-real");
    std::fs::create_dir_all(&isolated_home).unwrap();
    std::fs::create_dir_all(&sentinel_real_home).unwrap();

    // Point the env-resolved "real" home at a sentinel so we can prove the
    // install did NOT fall through to it.
    let previous_home = std::env::var("CLAUDE_TARGET_OVERRIDE").ok();
    std::env::set_var("CLAUDE_TARGET_OVERRIDE", &sentinel_real_home);

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_hook_command(
        &[
            "install".to_string(),
            "--claude-home".to_string(),
            isolated_home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );

    // Restore env before assertions so a failure does not leak override.
    match previous_home {
        Some(value) => std::env::set_var("CLAUDE_TARGET_OVERRIDE", value),
        None => std::env::remove_var("CLAUDE_TARGET_OVERRIDE"),
    }

    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let isolated_settings = isolated_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    let sentinel_settings = sentinel_real_home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    assert!(
        isolated_settings.is_file(),
        "hook install must write settings.json under --claude-home"
    );
    assert!(
        !sentinel_settings.exists(),
        "hook install must NOT write the env-resolved real home when --claude-home is given"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn hook_list_redacts_secret_env_values() {
    // WARN A regression: hook list/show printed settings.json verbatim,
    // leaking a live ANTHROPIC_AUTH_TOKEN in any captured output. The fix
    // masks secret-pattern keys while leaving non-secret structure intact.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!("hook-list-redact-{}", std::process::id());
    let home = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&home).unwrap();
    let settings = home.join(crate::hooks::claude::SETTINGS_FILE_NAME);
    std::fs::write(
        &settings,
        r#"{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "sk-secret-token-value-123456",
    "OPENAI_API_KEY": "key-abcdefghijklmnop",
    "ANTHROPIC_BASE_URL": "https://api.example.com"
  },
  "hooks": {}
}"#,
    )
    .unwrap();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_hook_command(
        &[
            "list".to_string(),
            "--claude-home".to_string(),
            home.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&stderr));
    let out = String::from_utf8_lossy(&stdout);

    // The raw secret values must be gone.
    assert!(
        !out.contains("sk-secret-token-value-123456"),
        "auth token must be redacted, got: {out}"
    );
    assert!(
        !out.contains("key-abcdefghijklmnop"),
        "api key must be redacted, got: {out}"
    );
    // A recognizable prefix is kept so operators can still identify it.
    assert!(out.contains("…(redacted)"), "masked marker missing: {out}");
    // Non-secret values stay readable.
    assert!(
        out.contains("https://api.example.com"),
        "non-secret base url must remain visible: {out}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn is_secret_key_classifies_known_markers() {
    assert!(is_secret_key("ANTHROPIC_AUTH_TOKEN"));
    assert!(is_secret_key("OPENAI_API_KEY"));
    assert!(is_secret_key("some_secret"));
    assert!(is_secret_key("DB_PASSWORD"));
    assert!(is_secret_key("ACCESS_KEY"));
    assert!(!is_secret_key("ANTHROPIC_BASE_URL"));
    assert!(!is_secret_key("matcher"));
    assert!(!is_secret_key("command"));
    // A bare `*key` suffix must NOT trigger without an auth/api/access
    // marker, so ordinary words are not falsely redacted.
    assert!(!is_secret_key("monkey"));
    assert!(!is_secret_key("passkey"));
}

#[test]
fn mask_secret_value_handles_multibyte_utf8_without_panicking() {
    // Regression: slicing &value[..4] by byte offset panics if a multi-byte
    // char straddles offset 4. Mask by chars so this can never panic.
    let masked = mask_secret_value("sk-¥token-multibyte-value");
    assert!(masked.ends_with("…(redacted)"), "got: {masked}");
    assert!(
        !masked.contains("multibyte-value"),
        "tail must be hidden: {masked}"
    );
    // Short multi-byte value is fully masked, also without panicking.
    assert_eq!(mask_secret_value("¥¥"), "****");
}

#[test]
fn redact_settings_suppresses_malformed_json_instead_of_leaking() {
    // Regression: the parse-failure path must NOT return the raw text — a
    // truncated/garbage settings.json could otherwise dump a live token.
    let malformed = r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"sk-leaky-secret-123"} TRAILING GARBAGE"#;
    let out = redact_secrets_in_settings(malformed);
    assert!(
        !out.contains("sk-leaky-secret-123"),
        "malformed JSON must not leak the secret, got: {out}"
    );
    assert!(
        out.contains("suppressed"),
        "expected suppression notice: {out}"
    );
}

#[test]
fn redact_settings_masks_secret_in_nested_object() {
    // A secret reached via a secret-named parent key, nested one level deep,
    // must still be masked (the parent_key_is_secret carry-down path).
    let nested = r#"{"credentials":{"value":"deep-secret-token-value"},"hooks":{}}"#;
    // "credentials" is not itself a marker, but "token" inside the value is
    // not how we detect it — detection is by KEY. Use a secret key wrapping
    // an object to exercise the carry-down.
    let by_secret_parent = r#"{"api_key":{"primary":"nested-secret-abcdef"},"hooks":{}}"#;
    let out = redact_secrets_in_settings(by_secret_parent);
    assert!(
        !out.contains("nested-secret-abcdef"),
        "secret under a secret-named parent key must be masked: {out}"
    );
    // A non-secret nested value stays visible.
    let out2 = redact_secrets_in_settings(nested);
    assert!(
        out2.contains("deep-secret-token-value"),
        "value under a non-secret key stays visible: {out2}"
    );
}

#[test]
fn install_writes_default_skill_listing_budget_fraction() {
    let hook_path = temp_hook_path("keel-skill-budget-default");
    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    std::fs::write(&hook_path, r#"{"hooks": {}}"#).unwrap();

    let executable = std::env::current_exe().unwrap();
    let rendered = build_hooks_payload(&hook_path, &executable).unwrap();
    let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

    assert_eq!(
        document
            .get("skillListingBudgetFraction")
            .and_then(JsonDocument::as_f64),
        Some(0.06),
    );

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

#[test]
fn install_preserves_user_skill_listing_budget_fraction() {
    let hook_path = temp_hook_path("keel-skill-budget-preserve");
    std::fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    std::fs::write(
        &hook_path,
        r#"{"hooks": {}, "skillListingBudgetFraction": 0.05}"#,
    )
    .unwrap();

    let executable = std::env::current_exe().unwrap();
    let rendered = build_hooks_payload(&hook_path, &executable).unwrap();
    let document: JsonDocument = serde_json::from_str(&rendered).unwrap();

    assert_eq!(
        document
            .get("skillListingBudgetFraction")
            .and_then(JsonDocument::as_f64),
        Some(0.05),
    );

    let _ = std::fs::remove_dir_all(hook_path.parent().unwrap());
}

// ----- Compression-discipline hint tests -----
//
// The following tests exercise the auto compression-output heuristic that
// gates the optional per-prompt nudge appended to UserPromptSubmit. They
// mutate process-global env vars `CLAUDE_SKILLS_COMPRESSION_HINT` and
// `CLAUDE_SKILLS_COMPRESSION_HINT_AFTER`, so each one takes the shared
// `crate::test_support::ENV_LOCK` before touching the environment. See
// the doc comment on that lock for the full design note.

fn compression_hint_tempdir(label: &str) -> PathBuf {
    let unique_suffix: u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let candidate = std::env::temp_dir().join(format!("{label}-{unique_suffix}"));
    std::fs::create_dir_all(&candidate).expect("create tempdir");
    candidate
}

fn write_session_timing_rows(claude_home: &Path, session_id: &str, count: usize) {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dir = claude_home.join("state").join("tool-timings");
    std::fs::create_dir_all(&dir).expect("create timings dir");
    let path = dir.join(format!("{date}.jsonl"));
    let mut body = String::new();
    for index in 0..count {
        body.push_str(&format!(
                r#"{{"recorded_at_ms":{index},"event":"PostToolUse","tool_name":"Read","duration_ms":12,"session_id":"{session_id}","cwd":"","effort_level":""}}"#
            ));
        body.push('\n');
    }
    std::fs::write(&path, body).expect("write timings fixture");
}

#[test]
fn compression_hint_text_names_three_actions() {
    // Pure text assertion — no env mutation, so no lock needed. The hint
    // must name the three discipline points so a model that sees only
    // this fragment still gets actionable guidance.
    let hint = compression_hint_text();
    assert!(
        hint.contains("narrower line ranges"),
        "compression hint must point at narrower line ranges"
    );
    assert!(
        hint.contains("Search before reading"),
        "compression hint must point at search-before-read"
    );
    assert!(
        hint.contains("Summarize logs"),
        "compression hint must point at summarizing logs"
    );
    assert!(
        hint.contains("compression-discipline"),
        "compression hint must reference the compression-discipline skill"
    );
}

#[test]
fn maybe_compression_hint_returns_none_when_threshold_not_reached() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-below-threshold");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    write_session_timing_rows(&claude_home, "session-A", 5);

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

    let hint = maybe_compression_hint(&claude_home, "session-A");
    assert!(
        hint.is_none(),
        "5 rows is below threshold of 40, must not inject hint"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn maybe_compression_hint_returns_some_when_threshold_reached() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-at-threshold");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    write_session_timing_rows(&claude_home, "session-B", 50);

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

    let hint = maybe_compression_hint(&claude_home, "session-B");
    assert!(
        hint.is_some(),
        "50 rows exceeds threshold of 40, must inject hint"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn maybe_compression_hint_respects_off_override() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-off");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    write_session_timing_rows(&claude_home, "session-C", 1000);

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "off");

    let hint = maybe_compression_hint(&claude_home, "session-C");
    assert!(
        hint.is_none(),
        "CLAUDE_SKILLS_COMPRESSION_HINT=off must override even at 1000 rows"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn maybe_compression_hint_respects_force_override_below_threshold() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-force");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    // Deliberately no JSONL on disk: force override must win even when the
    // heuristic would normally fail open.

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "force");

    let hint = maybe_compression_hint(&claude_home, "session-D");
    assert!(
        hint.is_some(),
        "CLAUDE_SKILLS_COMPRESSION_HINT=force must inject the hint even with no JSONL on disk"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn maybe_compression_hint_returns_none_for_missing_jsonl() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-missing-jsonl");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    // No state/tool-timings/<date>.jsonl on purpose. Heuristic must fail
    // open silently — telemetry hiccups never break the hook.

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "40");

    let hint = maybe_compression_hint(&claude_home, "session-E");
    assert!(
        hint.is_none(),
        "missing JSONL must yield no hint (fail-open)"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn count_session_tool_timing_rows_filters_to_named_session() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-count-session-rows");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    // Mix two sessions in the same JSONL: the count must only attribute
    // rows whose session_id matches the query.
    write_session_timing_rows(&claude_home, "session-mine", 7);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let mut existing = std::fs::read_to_string(&path).expect("read fixture");
    for index in 0..3 {
        existing.push_str(&format!(
                r#"{{"recorded_at_ms":{index},"event":"PostToolUse","tool_name":"Read","duration_ms":12,"session_id":"session-other","cwd":"","effort_level":""}}"#
            ));
        existing.push('\n');
    }
    // Add a deliberately malformed row to confirm parse errors are
    // skipped silently.
    existing.push_str("not-json\n");
    std::fs::write(&path, existing).expect("rewrite fixture");

    let count = count_session_tool_timing_rows(&claude_home, "session-mine");
    assert_eq!(
            count, 7,
            "must count only the 7 rows tagged with session-mine, ignore session-other and malformed rows"
        );

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn maybe_compression_hint_returns_none_when_threshold_is_zero() {
    // Operator escape hatch: setting CLAUDE_SKILLS_COMPRESSION_HINT_AFTER=0
    // disables the heuristic by short-circuiting before the JSONL is read.
    // This is a different code path from CLAUDE_SKILLS_COMPRESSION_HINT=off
    // and deserves its own coverage so a future change cannot remove the
    // `if threshold == 0` guard without surfacing as a test failure.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = compression_hint_tempdir("keel-hint-threshold-zero");
    let claude_home = temp.join("claude-home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");
    write_session_timing_rows(&claude_home, "session-Z", 1000);

    let previous_after = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER").ok();
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", "0");

    let hint = maybe_compression_hint(&claude_home, "session-Z");
    assert!(
        hint.is_none(),
        "CLAUDE_SKILLS_COMPRESSION_HINT_AFTER=0 must disable the heuristic even at 1000 rows"
    );

    match previous_after {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT_AFTER"),
    }
    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn append_compression_hint_when_forced_injects_hint_under_force() {
    // The fallback path used when stdin or claude_home are unavailable
    // re-reads CLAUDE_SKILLS_COMPRESSION_HINT independently of
    // maybe_compression_hint so diagnostic runs (force override, no real
    // session) still emit the nudge. Cover the force arm directly.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", "force");

    let result = append_compression_hint_when_forced("base context".to_string());
    assert!(
        result.contains("base context"),
        "force path must preserve base context"
    );
    assert!(
        result.contains("Output compression is on"),
        "force path must append the compression hint"
    );

    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
}

#[test]
fn append_compression_hint_when_forced_is_noop_without_force() {
    // The fallback must NOT inject the hint when no force override is set,
    // even if stdin was unavailable. Keeps the default behaviour exactly
    // equal to the unchanged base context.
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_mode = std::env::var("CLAUDE_SKILLS_COMPRESSION_HINT").ok();
    std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT");

    let result = append_compression_hint_when_forced("base context".to_string());
    assert_eq!(
        result, "base context",
        "fallback must be a no-op without the force override"
    );

    match previous_mode {
        Some(value) => std::env::set_var("CLAUDE_SKILLS_COMPRESSION_HINT", value),
        None => std::env::remove_var("CLAUDE_SKILLS_COMPRESSION_HINT"),
    }
}

#[test]
fn lifecycle_additional_context_does_not_handle_user_prompt_submit() {
    // Invariant guard for the dispatch split between run_hook_command and
    // lifecycle_additional_context. The "user-prompt-submit" slug is
    // handled exclusively by run_hook_user_prompt_submit because that
    // dispatcher reads stdin to extract session_id. If anyone re-adds an
    // arm for it in lifecycle_additional_context the per-prompt nudge
    // would silently regress to a stdin-blind path. This test asserts
    // the wildcard fall-through (-> empty string) is in force, which is
    // exactly the contract the dispatcher relies on.
    let result = lifecycle_additional_context("user-prompt-submit");
    assert!(
        result.is_empty(),
        "lifecycle_additional_context must not handle user-prompt-submit; got: {result:?}"
    );
}

#[test]
fn research_gate_nudges_when_no_research_before_edit() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "nudge");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    let temp = std::env::temp_dir().join("keel-research-gate-test");
    let claude_home = temp.join("claude-home");
    let _ = std::fs::create_dir_all(claude_home.join("state").join("tool-timings"));
    let _ = std::fs::create_dir_all(claude_home.join("state").join("research-gate-blocks"));

    let session_id = "test-research-gate-session";
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timings_path = claude_home
        .join("state")
        .join("tool-timings")
        .join(format!("{date}.jsonl"));
    let _ = std::fs::write(
            &timings_path,
            format!(
                "{{\"session_id\":\"{session_id}\",\"tool_name\":\"Write\",\"recorded_at_ms\":1000,\"cwd\":\"/tmp\"}}\n"
            ),
        );

    let decision = decide_gate(
        research_gate_mode(),
        research_gate_max_blocks(),
        0,
        1,
        session_has_research_tool(&claude_home, session_id),
    );
    assert_eq!(
        decision,
        GateDecision::Nudge,
        "research gate must nudge when code edited but no research tool found"
    );

    let nudge_msg = research_gate_message(GateDecision::Nudge);
    assert!(nudge_msg.contains("CLAUDE_SKILLS_RESEARCH_GATE"));
    assert!(nudge_msg.contains("does not stop the turn"));
    assert!(nudge_msg.contains("=off"));

    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn research_gate_off_matches_advisory_path() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");

    assert_eq!(
        decide_gate(GateMode::Off, 1, 0, 5, false),
        GateDecision::Advisory,
        "research gate off must always be Advisory"
    );

    let block_msg = research_gate_message(GateDecision::Block);
    assert!(block_msg.contains("hard stop"));
    let nudge_msg = research_gate_message(GateDecision::Nudge);
    assert!(nudge_msg.contains("does not stop the turn"));

    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
}

#[test]
fn story_first_gate_nudges_when_no_stories_before_edit() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    let previous_brief = std::env::var(BRIEF_GATE_ENV_VAR).ok();
    let previous_review = std::env::var(REVIEW_GATE_ENV_VAR).ok();
    let previous_research = std::env::var(RESEARCH_GATE_ENV_VAR).ok();
    let previous_closeout = std::env::var(STORY_CLOSEOUT_GATE_ENV_VAR).ok();
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "nudge");
    std::env::set_var(BRIEF_GATE_ENV_VAR, "off");
    std::env::set_var(REVIEW_GATE_ENV_VAR, "off");
    std::env::set_var(RESEARCH_GATE_ENV_VAR, "off");
    std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, "off");

    let temp = std::env::temp_dir().join("keel-story-first-gate-test");
    let claude_home = temp.join("claude-home");
    let _ = std::fs::create_dir_all(claude_home.join("state").join("story-first"));
    let _ = std::fs::create_dir_all(claude_home.join("state").join("story-first-gate-blocks"));

    let session_id = "test-story-first-gate-session";
    let marker = story_confirmed_marker_path(&claude_home, session_id);
    assert!(
        !marker.exists(),
        "marker must not exist before being created"
    );

    let decision = decide_gate(
        story_first_gate_mode(),
        story_first_gate_max_blocks(),
        0,
        1,
        marker.exists(),
    );
    assert_eq!(
        decision,
        GateDecision::Nudge,
        "story-first gate must nudge when code edited but no stories confirmed"
    );

    let nudge_msg = story_first_gate_message(GateDecision::Nudge);
    assert!(nudge_msg.contains("CLAUDE_SKILLS_STORY_FIRST_GATE"));
    assert!(nudge_msg.contains("does not stop the turn"));
    assert!(nudge_msg.contains("=off"));

    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
    match previous_brief {
        Some(value) => std::env::set_var(BRIEF_GATE_ENV_VAR, value),
        None => std::env::remove_var(BRIEF_GATE_ENV_VAR),
    }
    match previous_review {
        Some(value) => std::env::set_var(REVIEW_GATE_ENV_VAR, value),
        None => std::env::remove_var(REVIEW_GATE_ENV_VAR),
    }
    match previous_research {
        Some(value) => std::env::set_var(RESEARCH_GATE_ENV_VAR, value),
        None => std::env::remove_var(RESEARCH_GATE_ENV_VAR),
    }
    match previous_closeout {
        Some(value) => std::env::set_var(STORY_CLOSEOUT_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_CLOSEOUT_GATE_ENV_VAR),
    }
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn story_first_gate_off_matches_advisory_path() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_story_first = std::env::var(STORY_FIRST_GATE_ENV_VAR).ok();
    std::env::set_var(STORY_FIRST_GATE_ENV_VAR, "off");

    assert_eq!(
        decide_gate(GateMode::Off, 1, 0, 5, false),
        GateDecision::Advisory,
        "story-first gate off must always be Advisory"
    );

    let block_msg = story_first_gate_message(GateDecision::Block);
    assert!(block_msg.contains("hard stop"));
    let nudge_msg = story_first_gate_message(GateDecision::Nudge);
    assert!(nudge_msg.contains("does not stop the turn"));

    match previous_story_first {
        Some(value) => std::env::set_var(STORY_FIRST_GATE_ENV_VAR, value),
        None => std::env::remove_var(STORY_FIRST_GATE_ENV_VAR),
    }
}

#[test]
fn git_hooks_install_sets_core_hookspath_in_repo_config() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!("git-hooks-test-{}", std::process::id());
    let temp = std::env::temp_dir().join(unique);
    let repo_root = temp.join("repo");
    let githooks_dir = repo_root.join(".githooks");
    let git_dir = repo_root.join(".git");
    let git_config = git_dir.join("config");

    std::fs::create_dir_all(&githooks_dir).unwrap();
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::write(
        githooks_dir.join("pre-commit"),
        "#!/bin/sh\necho pre-commit",
    )
    .unwrap();
    std::fs::write(githooks_dir.join("pre-push"), "#!/bin/sh\necho pre-push").unwrap();
    std::fs::write(&git_config, "[core]\n\tbare = false\n").unwrap();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_hook_command(
        &[
            "git-hooks".to_string(),
            "install".to_string(),
            "--repo-root".to_string(),
            repo_root.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, 0, "git-hooks install should succeed");
    let stdout_str = String::from_utf8_lossy(&stdout);
    assert!(
        stdout_str.contains("pre-commit"),
        "output should list pre-commit hook: {stdout_str}"
    );
    assert!(
        stdout_str.contains("pre-push"),
        "output should list pre-push hook: {stdout_str}"
    );

    let config_content = std::fs::read_to_string(&git_config).unwrap();
    assert!(
        config_content.contains("hooksPath"),
        ".git/config should have hooksPath set: {config_content}"
    );
    assert!(
        config_content.contains(".githooks"),
        ".git/config hooksPath should point to .githooks: {config_content}"
    );

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn git_hooks_install_fails_when_githooks_dir_missing() {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let unique = format!("git-hooks-missing-{}", std::process::id());
    let temp = std::env::temp_dir().join(unique);
    let repo_root = temp.join("repo");
    std::fs::create_dir_all(&repo_root).unwrap();

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_hook_command(
        &[
            "git-hooks".to_string(),
            "install".to_string(),
            "--repo-root".to_string(),
            repo_root.to_string_lossy().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );

    assert_eq!(code, 1, "should fail when .githooks dir is missing");
    let stderr_str = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_str.contains(".githooks"),
        "stderr should mention missing .githooks: {stderr_str}"
    );

    let _ = std::fs::remove_dir_all(&temp);
}

// --- userConfig bridge (H1): the three plugin.json userConfig knobs must
//     actually affect behavior, not just appear in the /plugin settings UI. ---

/// Helper: run a closure with the given env vars set, then restore the prior
/// values (or remove if previously unset). Avoids cross-test env leakage.
/// Acquires the shared ENV_LOCK so env-mutating tests do not race each other.
fn with_env_vars<F: FnOnce()>(vars: &[(&str, Option<&str>)], body: F) {
    let _guard = crate::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved: Vec<(&str, Option<String>)> = vars
        .iter()
        .map(|(name, _)| (*name, std::env::var(name).ok()))
        .collect();
    for (name, value) in vars {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    body();
    for (name, prior) in saved {
        match prior {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

#[test]
fn user_config_review_strictness_advisory_maps_to_nudge() {
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS", Some("advisory")),
            (REVIEW_GATE_ENV_VAR, None),
        ],
        || {
            assert_eq!(review_gate_mode(), GateMode::Nudge);
        },
    );
}

#[test]
fn user_config_review_strictness_strict_maps_to_block() {
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS", Some("strict")),
            (REVIEW_GATE_ENV_VAR, None),
        ],
        || {
            assert_eq!(review_gate_mode(), GateMode::Block);
        },
    );
}

#[test]
fn user_config_review_strictness_off_disables_gate() {
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS", Some("off")),
            (REVIEW_GATE_ENV_VAR, None),
        ],
        || {
            assert_eq!(review_gate_mode(), GateMode::Off);
        },
    );
}

#[test]
fn user_config_review_strictness_loses_to_explicit_operator_var() {
    // The CLAUDE_SKILLS_* var is the operator escape hatch and must win even
    // when the userConfig value is set.
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS", Some("strict")),
            (REVIEW_GATE_ENV_VAR, Some("off")),
        ],
        || {
            assert_eq!(review_gate_mode(), GateMode::Off);
        },
    );
}

#[test]
fn user_config_review_strictness_default_escalate_when_unset() {
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_REVIEW_STRICTNESS", None),
            (REVIEW_GATE_ENV_VAR, None),
        ],
        || {
            assert_eq!(review_gate_mode(), GateMode::Escalate);
        },
    );
}

#[test]
fn user_config_system_map_refresh_interval_takes_effect() {
    with_env_vars(
        &[
            (
                "CLAUDE_PLUGIN_OPTION_SYSTEM_MAP_REFRESH_INTERVAL",
                Some("42"),
            ),
            ("CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL", None),
        ],
        || {
            assert_eq!(system_map_refresh_threshold(), 42);
        },
    );
}

#[test]
fn user_config_memory_retention_days_feeds_all_three_prune_readers() {
    // The single memory_retention_days knob feeds the raw, timings, and
    // observation retention readers via user_config_or_env_u64.
    with_env_vars(
        &[
            ("CLAUDE_PLUGIN_OPTION_MEMORY_RETENTION_DAYS", Some("7")),
            ("CLAUDE_SKILLS_RAW_RETENTION_DAYS", None),
            ("CLAUDE_SKILLS_TIMINGS_RETENTION_DAYS", None),
            ("CLAUDE_SKILLS_OBSERVATION_RETENTION_DAYS", None),
        ],
        || {
            assert_eq!(
                user_config_or_env_u64(
                    PLUGIN_MEMORY_RETENTION_DAYS,
                    "CLAUDE_SKILLS_RAW_RETENTION_DAYS",
                    RAW_OUTPUT_DEFAULT_RETENTION_DAYS,
                ),
                7
            );
        },
    );
}

#[test]
fn user_config_numeric_knob_ignores_garbage_and_falls_back() {
    // A non-numeric userConfig value must not panic or zero-out; it falls
    // through to the operator var/default.
    with_env_vars(
        &[
            (
                "CLAUDE_PLUGIN_OPTION_SYSTEM_MAP_REFRESH_INTERVAL",
                Some("not-a-number"),
            ),
            ("CLAUDE_SKILLS_SYSTEM_MAP_REFRESH_INTERVAL", Some("33")),
        ],
        || {
            assert_eq!(system_map_refresh_threshold(), 33);
        },
    );
}
