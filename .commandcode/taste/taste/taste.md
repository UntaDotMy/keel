# Taste
- Prefers findings and analysis presented as a table (e.g., columns for severity, issue, evidence, impact) rather than prose. Confidence: 0.7
- Expects terminal/CLI output to be human-readable on Windows: real newlines instead of literal `\n` escapes, and no internal path prefixes (e.g. `\\?\`) leaking into user-visible text. Confidence: 0.6
- Wants the agent to investigate and report gaps/issues but NOT apply fixes on its own; expects to review the findings and decide what to scope/fix. Confidence: 0.7
- Wants the plan written and presented for approval, and re-issued as an updated plan when new findings or symptoms arrive, before implementation continues. Confidence: 0.8
- Expects exhaustive "no gaps missing" verification across everything in scope, and dislikes having to iterate the same fix many times. Confidence: 0.75
- Treats findings handed to the agent (e.g. another model's audit) as claims to verify against the source before acting on them. Confidence: 0.6
- Wants "never hardcode / shared is must" rules actively enforced by the tooling itself for any project that uses it, not merely documented, so a fix lands in one file instead of many hardcoded copies. Confidence: 0.85
- Values modular structure with one owner per fact, for easier review, maintenance, and checking. Confidence: 0.8
- Expects long-lived servers/processes to serve multiple concurrent clients and to clean up after themselves, with no orphaned/zombie processes left behind. Confidence: 0.6
