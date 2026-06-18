//! Purpose: Render CompactResult into the concise agent-facing proxy output.
//! Caller: proxy::run after adapters finish compaction.
//! Dependencies: CompactResult and display_path formatting.
//! Main Functions: render_compact_result.
//! Side Effects: None; caller writes the rendered text.

use crate::proxy::adapter::CompactResult;
use crate::runtime::display_path;

pub fn render_compact_result(result: &CompactResult) -> String {
    let mut rendered = String::new();

    if result.adapter_name == "generic" {
        rendered.push_str(&result.summary);
    } else if result.exit_code == 0 {
        rendered.push_str(&format!("PASS {}\n", result.summary));
    } else {
        rendered.push_str(&format!("FAIL {}\n", result.summary));
    }

    if !result.stdout.is_empty() {
        rendered.push('\n');
        rendered.push_str(&result.stdout);
        rendered.push('\n');
    }

    if !result.stderr.is_empty() {
        rendered.push('\n');
        rendered.push_str(&result.stderr);
        rendered.push('\n');
    }

    rendered.push_str(&format!(
        "\nraw: keel raw {}\nraw_path: {}\n",
        result.raw_id,
        display_path(&result.raw_path)
    ));
    rendered.push_str(&format!(
        "saved: {} tokens exact/o200k_base ({:.1}%)",
        result.estimated_tokens_saved.max(0),
        result.savings_pct
    ));
    rendered
}
