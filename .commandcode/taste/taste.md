# Taste
- Expects bug reports to be investigated to root cause and actually fixed/settled, not just explained or diagnosed ("check and fix this"). Confidence: 0.7
- When a problem could live in any of several layers of their stack (e.g., keel vs Command Code), expects the agent to determine which component is at fault rather than assuming one. Confidence: 0.6
- Wants commits made locally while working, but explicitly does NOT want the agent to push to the remote until the entire task/plan is fully finished. Confidence: 0.9
- Expects the full scope of a plan/spec to be implemented before declaring done, getting frustrated when large parts are left unimplemented or partial, and wants honest status on coverage rather than a green CI run standing in for completion. Confidence: 0.75
- Does not accept "green" results at face value, wanting passing checks independently reviewed and re-verified (including empirically reproducing behavior), not blindly trusted. Confidence: 0.8
- Wants work continuously checked against the reference plan/spec, treating the plan as the source of truth for what "done" means. Confidence: 0.75
- Expects claims (e.g., about external specs/standards) to be verified via web search against primary sources rather than asserted from memory. Confidence: 0.7
- Wants the project's own tooling/CLI (e.g., keel) used to drive and validate the work, rather than ad-hoc substitutes. Confidence: 0.7
