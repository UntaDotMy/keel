//! Purpose: Validate the canonical planner architecture note before implementation.
//! Caller: utility::plan lifecycle and review::diff_gates pre-PR enforcement.
//! Dependencies: serde_json research claims and planner REQ/AC identifier sets.
//! Main Functions: validate_architecture_note.
//! Side Effects: None; validation is deterministic and read-only.

use std::collections::BTreeSet;

use serde_json::Value;

pub(crate) const MAX_ARCHITECTURE_BYTES: u64 = 65_536;

const SECTION_TITLES: [&str; 14] = [
    "Current architecture relevant to scope",
    "Proposed architecture",
    "Components/files/interfaces changed",
    "Data/control flow",
    "Alternatives considered",
    "Why the chosen option fits requirements",
    "Risks and mitigations",
    "Backward compatibility",
    "Error handling and fallback semantics",
    "Security/privacy implications",
    "Performance/token impact",
    "Test strategy",
    "Rollback strategy",
    "Requirement and research references",
];

type Issues = Vec<String>;

struct ArchitectureEvidence<'a> {
    requirement_ids: &'a BTreeSet<&'a str>,
    acceptance_ids: &'a BTreeSet<&'a str>,
    research: Option<&'a Value>,
}

pub(crate) fn validate_architecture_note(
    note: &str,
    requirement_ids: &BTreeSet<&str>,
    acceptance_ids: &BTreeSet<&str>,
    research: Option<&Value>,
) -> Issues {
    let mut issues = Vec::new();
    let evidence = ArchitectureEvidence {
        requirement_ids,
        acceptance_ids,
        research,
    };
    validate_status(note, &mut issues);
    let sections = collect_sections(note, &mut issues);
    if sections.iter().any(Option::is_none) {
        return issues;
    }

    let proposed = sections[1].expect("all architecture sections exist");
    let components = sections[2].expect("all architecture sections exist");
    let alternatives = sections[4].expect("all architecture sections exist");
    let rationale = sections[5].expect("all architecture sections exist");
    let risks = sections[6].expect("all architecture sections exist");
    let compatibility = sections[7].expect("all architecture sections exist");
    let failure = sections[8].expect("all architecture sections exist");
    let security = sections[9].expect("all architecture sections exist");
    let performance = sections[10].expect("all architecture sections exist");
    let tests = sections[11].expect("all architecture sections exist");
    let rollback = sections[12].expect("all architecture sections exist");
    let references = sections[13].expect("all architecture sections exist");

    validate_proposal(proposed, &mut issues);
    validate_component_mappings(components, &evidence, &mut issues);
    require_label_pairs(alternatives, 5, "Alternative:", "Tradeoff:", &mut issues);
    validate_rationale(rationale, &mut issues);
    require_label_pairs(risks, 7, "Risk:", "Mitigation:", &mut issues);
    validate_compatibility(compatibility, &mut issues);
    validate_failure_semantics(failure, &mut issues);
    require_labels(security, 10, &["Security/privacy:"], &mut issues);
    require_labels(
        performance,
        11,
        &["Token impact:", "Measurement plan:"],
        &mut issues,
    );
    require_labels(
        tests,
        12,
        &["Verification:", "Acceptance references:"],
        &mut issues,
    );
    require_labels(rollback, 13, &["Rollback:"], &mut issues);
    validate_reference_section(note, references, &evidence, &mut issues);
    if note
        .lines()
        .filter(|line| line.trim().starts_with("Policy owner:"))
        .count()
        > 1
    {
        issues.push("architecture.md defines more than one Policy owner".to_string());
    }
    issues
}

fn validate_status(note: &str, issues: &mut Issues) {
    let statuses: Vec<&str> = note
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Status:"))
        .map(str::trim)
        .collect();
    match statuses.as_slice() {
        ["complete"] => {}
        [] => issues.push("architecture.md has no Status field".to_string()),
        [_] => issues.push("architecture.md status is not complete".to_string()),
        _ => issues.push("architecture.md has duplicate Status fields".to_string()),
    }
}

fn collect_sections<'a>(note: &'a str, issues: &mut Issues) -> Vec<Option<&'a str>> {
    let starts: Vec<Option<(usize, String)>> = SECTION_TITLES
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let number = index + 1;
            let header = format!("## {number}. {title}");
            let positions: Vec<usize> = note
                .match_indices(header.as_str())
                .filter_map(|(position, _)| {
                    is_heading_line(note, position, header.len()).then_some(position)
                })
                .collect();
            match positions.as_slice() {
                [start] => Some((*start, header)),
                [] => {
                    issues.push(format!(
                        "architecture.md is missing section {number}. {title}"
                    ));
                    None
                }
                _ => {
                    issues.push(format!(
                        "architecture.md duplicates section {number}. {title}"
                    ));
                    None
                }
            }
        })
        .collect();
    let ordered_starts: Vec<usize> = starts
        .iter()
        .filter_map(|entry| entry.as_ref().map(|(start, _)| *start))
        .collect();
    if ordered_starts.len() == SECTION_TITLES.len()
        && ordered_starts.windows(2).any(|pair| pair[0] >= pair[1])
    {
        issues.push("architecture.md sections are not in the required order".to_string());
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let (start, header) = entry.as_ref()?;
            let after_header = &note[*start + header.len()..];
            let end = after_header.find("\n## ").unwrap_or(after_header.len());
            let body = after_header[..end].trim();
            if body.is_empty() || is_placeholder(body) {
                issues.push(format!(
                    "architecture.md section {} is incomplete",
                    index + 1
                ));
                None
            } else {
                Some(body)
            }
        })
        .collect()
}

fn is_heading_line(note: &str, start: usize, length: usize) -> bool {
    let bytes = note.as_bytes();
    let before = start == 0 || bytes.get(start.wrapping_sub(1)) == Some(&b'\n');
    let end = start + length;
    let after = end == bytes.len() || matches!(bytes.get(end), Some(b'\r' | b'\n'));
    before && after
}

fn validate_proposal(section: &str, issues: &mut Issues) {
    let input_bound = required_label(section, 2, "Input bound:", issues);
    required_label(section, 2, "Policy owner:", issues);
    if let Some(value) = input_bound {
        let input_bound = normalized(value);
        let bounded = input_bound.contains("bounded")
            || input_bound.contains("maximum")
            || input_bound.contains("at most")
            || input_bound.starts_with("one ")
            || input_bound.starts_with("none");
        if !bounded || (input_bound.contains("unbounded") && !input_bound.contains("no unbounded"))
        {
            issues.push("architecture.md Input bound does not define a bounded input".to_string());
        }
    }
}

fn validate_component_mappings(
    section: &str,
    mappings: &ArchitectureEvidence<'_>,
    issues: &mut Issues,
) {
    let mut component_names = BTreeSet::new();
    let mut mapped_requirements = BTreeSet::new();
    let mut mapped_acceptance = BTreeSet::new();
    for line in section.lines().map(str::trim) {
        let Some(component_line) = line.strip_prefix("- Component:") else {
            continue;
        };
        let parts: Vec<&str> = component_line.split('|').map(str::trim).collect();
        if parts.len() != 3
            || parts
                .iter()
                .filter(|part| part.starts_with("Requirements:"))
                .count()
                != 1
            || parts
                .iter()
                .filter(|part| part.starts_with("Acceptance:"))
                .count()
                != 1
        {
            issues.push(
                "architecture.md component mappings must use `- Component: <name> | Requirements: <REQ IDs> | Acceptance: <AC IDs>`"
                    .to_string(),
            );
        }
        let name = parts.first().copied().unwrap_or_default().trim();
        let label = if name.is_empty() {
            "component without name"
        } else {
            name
        };
        if is_placeholder(name) {
            issues.push("architecture.md has an incomplete component mapping".to_string());
        } else if !component_names.insert(name) {
            issues.push(format!("architecture.md duplicates component {name}"));
        }
        let requirements = component_reference_values(&parts, "Requirements:");
        let acceptance = component_reference_values(&parts, "Acceptance:");
        if requirements.is_empty() {
            issues.push(format!(
                "architecture.md component {label} has no requirement mappings"
            ));
        }
        if acceptance.is_empty() {
            issues.push(format!(
                "architecture.md component {label} has no acceptance mappings"
            ));
        }
        for requirement in requirements {
            if mappings.requirement_ids.contains(requirement.as_str()) {
                mapped_requirements.insert(requirement);
            } else {
                issues.push(format!("architecture.md component {label} references unknown requirement {requirement}"));
            }
        }
        for criterion in acceptance {
            if mappings.acceptance_ids.contains(criterion.as_str()) {
                mapped_acceptance.insert(criterion);
            } else {
                issues.push(format!("architecture.md component {label} references unknown acceptance criterion {criterion}"));
            }
        }
    }
    if component_names.is_empty() {
        issues.push("architecture.md has no component mappings".to_string());
    }
    for requirement in mappings.requirement_ids {
        if !mapped_requirements.contains(*requirement) {
            issues.push(format!(
                "architecture.md has no component mapping for {requirement}"
            ));
        }
    }
    for criterion in mappings.acceptance_ids {
        if !mapped_acceptance.contains(*criterion) {
            issues.push(format!(
                "architecture.md has no component mapping for {criterion}"
            ));
        }
    }
}

fn validate_rationale(section: &str, issues: &mut Issues) {
    required_label(section, 6, "Chosen option:", issues);
    let infrastructure = required_label(section, 6, "Infrastructure reuse:", issues);
    required_label(section, 6, "Constraint fit:", issues);
    if infrastructure
        .map(|value| value.trim().to_ascii_lowercase().starts_with("none"))
        .unwrap_or(false)
    {
        issues.push("architecture.md must name reused Keel infrastructure".to_string());
    }
}

fn component_reference_values(parts: &[&str], label: &str) -> Vec<String> {
    parts
        .iter()
        .find_map(|part| part.strip_prefix(label))
        .map(split_ids)
        .unwrap_or_default()
}

fn validate_compatibility(section: &str, issues: &mut Issues) {
    required_label(section, 8, "Compatibility:", issues);
    let host_impact = required_label(section, 8, "Host impact:", issues);
    if let Some(value) = host_impact {
        let host_impact = normalized(value);
        let unchanged = host_impact.starts_with("none") || host_impact.contains("unchanged");
        if !unchanged && !host_impact.contains("11 adapter contracts") {
            issues.push(
                "architecture.md host changes must preserve all 11 adapter contracts".to_string(),
            );
        }
    }
}

fn validate_failure_semantics(section: &str, issues: &mut Issues) {
    required_label(section, 9, "Failure status:", issues);
    required_label(section, 9, "Fallback:", issues);
    let visibility = required_label(section, 9, "Visibility:", issues);
    if let Some(value) = visibility {
        let visibility = normalized(value);
        if !["operator", "user", "reviewer", "status"]
            .iter()
            .any(|audience| visibility.contains(audience))
        {
            issues.push(
                "architecture.md Visibility must name operator, user, reviewer, or status output"
                    .to_string(),
            );
        }
    }
}

fn validate_reference_section(
    note: &str,
    section: &str,
    traceability: &ArchitectureEvidence<'_>,
    issues: &mut Issues,
) {
    let requirement_refs = required_id_list(section, 14, "Requirement references:", issues);
    validate_complete_references(
        "requirement",
        &requirement_refs,
        traceability.requirement_ids,
        issues,
    );
    let acceptance_refs = required_id_list(section, 14, "Acceptance references:", issues);
    validate_complete_references(
        "acceptance criterion",
        &acceptance_refs,
        traceability.acceptance_ids,
        issues,
    );
    let claim_refs = required_id_list(section, 14, "Claim references:", issues);
    validate_claim_references(note, &claim_refs, traceability, issues);
}

fn validate_complete_references(
    kind: &str,
    references: &[String],
    expected: &BTreeSet<&str>,
    issues: &mut Issues,
) {
    for reference in references {
        if !expected.contains(reference.as_str()) {
            issues.push(format!(
                "architecture.md references unknown {kind} {reference}"
            ));
        }
    }
    for identifier in expected {
        if !references.iter().any(|reference| reference == identifier) {
            issues.push(format!(
                "architecture.md does not reference {kind} {identifier}"
            ));
        }
    }
}

fn validate_claim_references(
    section: &str,
    references: &[String],
    claims_context: &ArchitectureEvidence<'_>,
    issues: &mut Issues,
) {
    let claims: Vec<&Value> = claims_context
        .research
        .and_then(|value| value.get("claims"))
        .and_then(Value::as_array)
        .map(|claims| claims.iter().collect())
        .unwrap_or_default();
    let research_ids: BTreeSet<&str> = claims
        .iter()
        .filter_map(|claim| claim.get("claimId").and_then(Value::as_str))
        .collect();
    for claim_id in references {
        if !research_ids.contains(claim_id.as_str()) {
            issues.push(format!(
                "architecture.md references unknown research claim {claim_id}"
            ));
        }
        if !["verified", "assumption", "derived"]
            .iter()
            .any(|classification| section.contains(&format!("[{classification}: {claim_id}]")))
        {
            issues.push(format!("architecture.md claim {claim_id} is unclassified"));
        }
    }
    for claim_id in research_ids {
        if !references.iter().any(|reference| reference == claim_id) {
            issues.push(format!(
                "architecture.md does not reference research claim {claim_id}"
            ));
        }
    }
}

fn require_labels(section: &str, number: usize, labels: &[&str], issues: &mut Issues) {
    for label in labels {
        required_label(section, number, label, issues);
    }
}

fn require_label_pairs(
    section: &str,
    number: usize,
    first_label: &str,
    second_label: &str,
    issues: &mut Issues,
) {
    let first_values = raw_label_values(section, first_label);
    let second_values = raw_label_values(section, second_label);
    if first_values.iter().any(|value| is_placeholder(value)) {
        issues.push(format!(
            "architecture.md section {number} has incomplete {first_label}"
        ));
    }
    if second_values.iter().any(|value| is_placeholder(value)) {
        issues.push(format!(
            "architecture.md section {number} has incomplete {second_label}"
        ));
    }
    let first: Vec<&str> = first_values
        .into_iter()
        .filter(|value| !is_placeholder(value))
        .collect();
    let second: Vec<&str> = second_values
        .into_iter()
        .filter(|value| !is_placeholder(value))
        .collect();
    if first.is_empty() {
        issues.push(format!(
            "architecture.md section {number} has no {first_label}"
        ));
    }
    if second.is_empty() {
        issues.push(format!(
            "architecture.md section {number} has no {second_label}"
        ));
    }
    if !first.is_empty() && !second.is_empty() && first.len() != second.len() {
        issues.push(format!(
            "architecture.md section {number} must pair each {first_label} with one {second_label}"
        ));
    }
}

fn raw_label_values<'a>(section: &'a str, label: &str) -> Vec<&'a str> {
    section
        .lines()
        .filter_map(|line| line.trim().strip_prefix(label))
        .map(str::trim)
        .collect()
}

fn required_id_list(section: &str, number: usize, label: &str, issues: &mut Issues) -> Vec<String> {
    required_label(section, number, label, issues)
        .map(split_ids)
        .unwrap_or_default()
}

fn required_label<'a>(
    section: &'a str,
    number: usize,
    label: &str,
    issues: &mut Issues,
) -> Option<&'a str> {
    let values: Vec<&str> = section
        .lines()
        .filter_map(|line| line.trim().strip_prefix(label))
        .map(str::trim)
        .collect();
    match values.as_slice() {
        [value] if !is_placeholder(value) => Some(value),
        [_] | [] => {
            issues.push(format!("architecture.md section {number} has no {label}"));
            None
        }
        _ => {
            issues.push(format!(
                "architecture.md section {number} duplicates {label}"
            ));
            None
        }
    }
}

fn is_placeholder(value: &str) -> bool {
    let candidate = normalized(value);
    candidate.is_empty()
        || matches!(candidate.as_str(), "pending" | "tbd" | "todo")
        || candidate.starts_with("pending ")
        || candidate.starts_with("pending.")
        || (candidate.starts_with('<') && candidate.ends_with('>'))
}

fn normalized(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn split_ids(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}
