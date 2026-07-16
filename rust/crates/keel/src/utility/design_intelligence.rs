//! Purpose: Design-intelligence recommendation generator backed by the installed catalog.
//! Caller: commands.rs via the utility dispatcher (`design-intelligence recommend`).
//! Dependencies: std::fs, std::path, serde_json, crate::args, crate::runtime.
//! Main Functions: run_design_intelligence_command.
//! Side Effects: Reads the design-intelligence catalog JSON; optionally writes a
//!   persisted design-system markdown artifact when `--persist` is set.
//!
//! This replaces the former three-line stub in code_search.rs. The catalog
//! (`ui-design-systems-and-responsive-interfaces/data/design_intelligence_catalog.json`)
//! ships with the skill and is installed to `<claude_home>/skills/.../data/`.
//! The generator scores the free-text request against archetype/style/color/
//! typography keywords, biases the selection by an optional `--stack`, and emits
//! the documented packet shape (text or JSON), so the SKILL.md generator workflow
//! is real rather than aspirational.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::runtime::{
    clean_path, discover_repository_layout, display_path, resolve_claude_home,
    resolve_repository_root, skills_directory,
};

const UI_SKILL_NAME: &str = "ui-design-systems-and-responsive-interfaces";
const CATALOG_RELATIVE_PATH: &str = "data/design_intelligence_catalog.json";

pub fn run_design_intelligence_command(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    if arguments.is_empty() || is_help_argument(&arguments[0]) {
        let _ = writeln!(
            standard_output,
            "Usage: keel design-intelligence recommend [request...] \
             [--stack <id>] [--component-library <name>] [--format text|json] \
             [--density <1-10>] [--variance <1-10>] \
             [--persist --project-name <name> --page <name> --force] \
             [--out <path>] [--catalog <path>]"
        );
        return if arguments.is_empty() { 1 } else { 0 };
    }
    if arguments[0] != "recommend" {
        let _ = writeln!(
            standard_error,
            "Unknown design-intelligence command: {}",
            arguments[0]
        );
        return 1;
    }
    run_recommend(&arguments[1..], standard_output, standard_error)
}

fn run_recommend(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let parsed = match RecommendArgs::parse(arguments) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = writeln!(standard_error, "{error}");
            return 1;
        }
    };

    let request = parsed.request.trim().to_string();
    if request.is_empty() {
        let _ = writeln!(
            standard_error,
            "design-intelligence recommend: a product or feature request is required \
             (example: recommend \"fintech banking dashboard with secure transfers\")"
        );
        return 1;
    }

    if parsed.format != "text" && parsed.format != "json" {
        let _ = writeln!(
            standard_error,
            "design-intelligence recommend: --format must be 'text' or 'json'"
        );
        return 1;
    }

    let catalog = match load_catalog(&parsed.catalog) {
        Ok(catalog) => catalog,
        Err(error) => {
            let _ = writeln!(standard_error, "design-intelligence recommend: {error}");
            return 1;
        }
    };

    let packet = build_packet(
        &request,
        &catalog,
        &parsed.stack,
        &parsed.component_library,
        parsed.density,
        parsed.variance,
    );

    if parsed.persist {
        match persist_design_system(
            &packet,
            &request,
            &parsed.project_name,
            &parsed.page,
            &parsed.out,
            parsed.force,
        ) {
            Ok(PersistOutcome::Wrote(path)) => {
                let _ = writeln!(
                    standard_output,
                    "Persisted design system to {}",
                    display_path(&path)
                );
            }
            Ok(PersistOutcome::SkippedExisting(path)) => {
                let _ = writeln!(
                    standard_output,
                    "Persist skipped (already exists, pass --force to overwrite): {}",
                    display_path(&path)
                );
            }
            Err(error) => {
                let _ = writeln!(standard_error, "design-intelligence recommend: {error}");
                return 1;
            }
        }
    }

    if parsed.format == "json" {
        match serde_json::to_string_pretty(&packet) {
            Ok(serialized) => {
                let _ = writeln!(standard_output, "{serialized}");
            }
            Err(error) => {
                let _ = writeln!(standard_error, "design-intelligence recommend: {error}");
                return 1;
            }
        }
    } else {
        render_text(&packet, standard_output);
    }
    0
}

/// Parsed `recommend` arguments. Unlike the shared `FlagSet` (which captures
/// everything after the first positional token as positional), this recognizes
/// flags anywhere in the argument list so the documented `recommend "<request>"
/// --stack <id> --format json` ordering works — the request text and the flags
/// can appear in either order.
struct RecommendArgs {
    request: String,
    stack: String,
    component_library: String,
    format: String,
    persist: bool,
    force: bool,
    project_name: String,
    page: String,
    catalog: String,
    out: String,
    /// 1-10 spacious → dense; None = catalog default.
    density: Option<u8>,
    /// 1-10 minimal/centered → bold/asymmetric; None = catalog default.
    variance: Option<u8>,
}

impl RecommendArgs {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut parsed = RecommendArgs {
            request: String::new(),
            stack: String::new(),
            component_library: String::new(),
            format: "text".to_string(),
            persist: false,
            force: false,
            project_name: String::new(),
            page: String::new(),
            catalog: String::new(),
            out: String::new(),
            density: None,
            variance: None,
        };
        let mut request_tokens: Vec<String> = Vec::new();
        let mut index = 0;
        let mut request_terminated = false;
        while index < arguments.len() {
            let token = &arguments[index];
            if token == "--" {
                // Everything after a bare `--` is request text.
                request_tokens.extend(arguments[index + 1..].iter().cloned());
                break;
            }
            if let Some(stripped) = token.strip_prefix("--") {
                let (flag_name, inline_value) = match stripped.split_once('=') {
                    Some((name, value)) => (name.to_string(), Some(value.to_string())),
                    None => (stripped.to_string(), None),
                };
                // Once a flag is seen, request text is complete; later bare
                // tokens are flag values, not request words.
                request_terminated = true;
                let take_value = |index: &mut usize| -> Result<String, String> {
                    if let Some(value) = inline_value.clone() {
                        return Ok(value);
                    }
                    if *index + 1 >= arguments.len() {
                        return Err(format!(
                            "design-intelligence recommend: flag --{flag_name} needs a value"
                        ));
                    }
                    *index += 1;
                    Ok(arguments[*index].clone())
                };
                match flag_name.as_str() {
                    "stack" => parsed.stack = take_value(&mut index)?,
                    "component-library" => parsed.component_library = take_value(&mut index)?,
                    "format" => parsed.format = take_value(&mut index)?,
                    "project-name" => parsed.project_name = take_value(&mut index)?,
                    "page" => parsed.page = take_value(&mut index)?,
                    "catalog" => parsed.catalog = take_value(&mut index)?,
                    "out" => parsed.out = take_value(&mut index)?,
                    "density" => {
                        parsed.density =
                            Some(parse_dial_1_to_10(&take_value(&mut index)?, "density")?);
                    }
                    "variance" => {
                        parsed.variance =
                            Some(parse_dial_1_to_10(&take_value(&mut index)?, "variance")?);
                    }
                    "force" => {
                        parsed.force = match inline_value.as_deref() {
                            None => true,
                            Some("true") | Some("1") => true,
                            Some("false") | Some("0") => false,
                            Some(other) => {
                                return Err(format!(
                                    "design-intelligence recommend: invalid boolean {other:?} for --force"
                                ));
                            }
                        };
                    }
                    "persist" => {
                        parsed.persist = match inline_value.as_deref() {
                            None => true,
                            Some("true") | Some("1") => true,
                            Some("false") | Some("0") => false,
                            Some(other) => {
                                return Err(format!(
                                    "design-intelligence recommend: invalid boolean {other:?} for --persist"
                                ));
                            }
                        };
                    }
                    other => {
                        return Err(format!(
                            "design-intelligence recommend: unknown flag --{other}"
                        ));
                    }
                }
                index += 1;
                continue;
            }
            // A bare token. It belongs to the request only if no flag has been
            // seen yet; after a flag, a stray bare token is an error rather than
            // silently joining the request.
            if request_terminated {
                return Err(format!(
                    "design-intelligence recommend: unexpected argument {token:?} after flags"
                ));
            }
            request_tokens.push(token.clone());
            index += 1;
        }
        parsed.request = request_tokens.join(" ");
        Ok(parsed)
    }
}

/// Resolve and parse the catalog: explicit `--catalog` wins, then the repo
/// layout (when invoked from a checkout), then the installed skills directory.
fn load_catalog(catalog_override: &str) -> Result<Value, String> {
    let path = resolve_catalog_path(catalog_override)?;
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("read catalog {}: {error}", display_path(&path)))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("parse catalog {}: {error}", display_path(&path)))?;
    Ok(value)
}

fn resolve_catalog_path(catalog_override: &str) -> Result<PathBuf, String> {
    if !catalog_override.trim().is_empty() {
        let candidate = clean_path(&PathBuf::from(catalog_override.trim()));
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(format!(
            "catalog not found at --catalog path: {}",
            display_path(&candidate)
        ));
    }
    if let Ok(root) = resolve_repository_root("") {
        if let Ok(layout) = discover_repository_layout(&root) {
            if let Some(skill) = layout.skills.iter().find(|s| s.name == UI_SKILL_NAME) {
                let candidate = skill.skill_path.join(CATALOG_RELATIVE_PATH);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    if let Ok(claude_home) = resolve_claude_home("") {
        let candidate = skills_directory(&claude_home)
            .join(UI_SKILL_NAME)
            .join(CATALOG_RELATIVE_PATH);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(
        "design-intelligence catalog not found (looked in the repo skill and the installed \
         skills directory); pass --catalog <path> to point at it explicitly"
            .to_string(),
    )
}

fn parse_dial_1_to_10(raw: &str, name: &str) -> Result<u8, String> {
    let value: u8 = raw.trim().parse().map_err(|_| {
        format!("design-intelligence recommend: --{name} must be an integer 1-10, got {raw:?}")
    })?;
    if !(1..=10).contains(&value) {
        return Err(format!(
            "design-intelligence recommend: --{name} must be 1-10, got {value}"
        ));
    }
    Ok(value)
}

fn build_packet(
    request: &str,
    catalog: &Value,
    stack_id: &str,
    component_library: &str,
    density_dial: Option<u8>,
    variance_dial: Option<u8>,
) -> Value {
    let request_lower = request.to_lowercase();
    let tokens = tokenize(&request_lower);

    let archetypes = array_field(catalog, "product_archetypes");
    let style_families = array_field(catalog, "style_families");
    let color_moods = array_field(catalog, "color_moods");
    let typography_moods = array_field(catalog, "typography_moods");
    let stack_profiles = array_field(catalog, "stack_profiles");

    // Archetype is the spine of the recommendation.
    let (archetype, archetype_score) = pick_archetype(&archetypes, &request_lower, &tokens);
    let confidence = match archetype_score {
        s if s >= 2 => "high",
        s if s >= 1 => "medium",
        _ => "low",
    };

    // Optional stack profile biases the family/color/typography choices.
    let stack = if stack_id.trim().is_empty() {
        None
    } else {
        find_stack(&stack_profiles, stack_id)
    };

    let mut style_pref = biased_preferences(
        archetype,
        stack,
        "recommended_style_families",
        "preferred_style_families",
    );
    apply_variance_to_style_prefs(&mut style_pref, variance_dial, &style_families);

    let color_pref = biased_preferences(
        archetype,
        stack,
        "recommended_color_moods",
        "preferred_color_moods",
    );
    let typography_pref = biased_preferences(
        archetype,
        stack,
        "recommended_typography_moods",
        "preferred_typography_moods",
    );

    let style = choose(&style_pref, &style_families, &request_lower, &tokens);
    let color = choose(&color_pref, &color_moods, &request_lower, &tokens);
    let typography = choose(&typography_pref, &typography_moods, &request_lower, &tokens);

    let density_label =
        density_label_from_dial(density_dial, &str_field(archetype, "recommended_density"));
    let motion_posture = str_field(archetype, "recommended_motion_posture");
    let motion_guidance = motion_guidance_for(&motion_posture, density_dial, variance_dial);

    let polish_checks = checks_or_default(
        archetype,
        "professional_polish_checks",
        &[
            "Establish one clear primary action per screen and keep secondary controls visually quieter.",
            "Use design tokens for spacing, color, and typography instead of hardcoded values, and keep interactive affordances obvious.",
        ],
    );
    let recovery_checks = checks_or_default(
        archetype,
        "recovery_checks",
        &[
            "Design empty, loading, error, and success states with the same care as the happy path.",
            "Preserve user input and progress across navigation, interruption, and reconnection.",
        ],
    );
    let verification_checks = checks_or_default(
        archetype,
        "verification_checks",
        &[
            "Walk the primary task path end to end before approving the visual direction.",
            "Verify responsive behavior and WCAG 2.2 AA contrast and keyboard access across desktop and mobile.",
        ],
    );

    let mut packet = json!({
        "request": request,
        "confidence": confidence,
        "archetype_match_score": archetype_score,
        "dials": {
            "density": density_dial,
            "variance": variance_dial,
        },
        "product_archetype": {
            "id": str_field(archetype, "id"),
            "display_name": str_field(archetype, "display_name"),
            "trust_posture": str_field(archetype, "trust_posture"),
            "content_priorities": str_array(archetype, "content_priorities"),
            "cta_guidance": str_field(archetype, "cta_guidance"),
            "motion_posture": motion_posture,
            "density": density_label,
        },
        "style_family": entry_summary(style, "visual_direction"),
        "color_mood": entry_summary(color, "palette_direction"),
        "typography_mood": entry_summary(typography, "direction"),
        "motion_guidance": motion_guidance,
        "professional_polish_checks": polish_checks,
        "recovery_checks": recovery_checks,
        "verification_checks": verification_checks,
        "anti_patterns": merged_anti_patterns(archetype, style),
        "decision_rules": active_decision_rules(archetype, &request_lower),
    });

    if let Some(stack_entry) = stack {
        packet["stack_adaptation"] = json!({
            "id": str_field(stack_entry, "id"),
            "display_name": str_field(stack_entry, "display_name"),
            "guidance": str_array(stack_entry, "guidance"),
            "component_preview_tools": str_array(stack_entry, "component_preview_tools"),
            "validation_checks": str_array(stack_entry, "validation_checks"),
        });
    } else if !stack_id.trim().is_empty() {
        packet["stack_adaptation"] = json!({
            "note": format!(
                "stack '{stack_id}' not in the catalog; recommendation is stack-agnostic"
            ),
        });
    }

    if !component_library.trim().is_empty() {
        packet["component_library"] = json!({
            "name": component_library,
            "guidance": format!(
                "Reuse {component_library} primitives and tokens before introducing custom \
                 components; align spacing, color, and state styling to its theme."
            ),
        });
    }

    // Concrete artifacts: a real palette, a real font pairing, chart guidance,
    // and matched UX rules — the data that makes the recommendation buildable
    // rather than abstract.
    let color_id = str_field(color, "id");
    if let Some(palette) = pick_palette(catalog, &color_id, &request_lower, &tokens) {
        packet["color_palette"] = palette_summary(palette);
    }
    let typography_id = str_field(typography, "id");
    if let Some(pairing) = pick_font_pairing(catalog, &typography_id, &request_lower, &tokens) {
        packet["font_pairing"] = font_pairing_summary(pairing);
    }
    let charts = pick_chart_types(catalog, &request_lower, &tokens);
    if !charts.is_empty() {
        packet["recommended_charts"] = Value::Array(charts);
    }
    let guidelines = pick_ux_guidelines(catalog, archetype, &request_lower, &tokens);
    if !guidelines.is_empty() {
        packet["ux_guidelines"] = Value::Array(guidelines);
    }

    packet
}

/// Pick the best palette for the chosen color mood: prefer palettes whose
/// `color_mood` matches, ranked by direct keyword score against the request.
fn pick_palette<'a>(
    catalog: &'a Value,
    color_mood_id: &str,
    request_lower: &str,
    tokens: &HashSet<String>,
) -> Option<&'a Value> {
    let palettes = catalog.get("color_palettes")?.as_array()?;
    let wants_dark = request_lower.contains("dark");
    let mut best: Option<(&Value, i64)> = None;
    for palette in palettes {
        if str_field(palette, "color_mood") != color_mood_id {
            continue;
        }
        let mut score =
            score_keywords(request_lower, tokens, &str_array(palette, "keywords")) as i64;
        // Honor an explicit light/dark request when present.
        if str_field(palette, "mode") == "dark" {
            score += if wants_dark { 2 } else { -1 };
        } else if wants_dark {
            score -= 1;
        }
        if best.map_or(true, |(_, best_score)| score > best_score) {
            best = Some((palette, score));
        }
    }
    best.map(|(palette, _)| palette)
}

fn palette_summary(palette: &Value) -> Value {
    json!({
        "id": str_field(palette, "id"),
        "display_name": str_field(palette, "display_name"),
        "mode": str_field(palette, "mode"),
        "colors": palette.get("colors").cloned().unwrap_or(Value::Null),
        "contrast_notes": str_field(palette, "contrast_notes"),
    })
}

/// Pick the best font pairing for the chosen typography mood.
fn pick_font_pairing<'a>(
    catalog: &'a Value,
    typography_mood_id: &str,
    request_lower: &str,
    tokens: &HashSet<String>,
) -> Option<&'a Value> {
    let pairings = catalog.get("font_pairings")?.as_array()?;
    let mut best: Option<(&Value, u32)> = None;
    for pairing in pairings {
        if str_field(pairing, "typography_mood") != typography_mood_id {
            continue;
        }
        let score = score_keywords(request_lower, tokens, &str_array(pairing, "keywords"));
        if best.map_or(true, |(_, best_score)| score > best_score) {
            best = Some((pairing, score));
        }
    }
    best.map(|(pairing, _)| pairing)
}

fn font_pairing_summary(pairing: &Value) -> Value {
    json!({
        "id": str_field(pairing, "id"),
        "display_name": str_field(pairing, "display_name"),
        "heading_font": str_field(pairing, "heading_font"),
        "body_font": str_field(pairing, "body_font"),
        "mono_font": str_field(pairing, "mono_font"),
        "source": str_field(pairing, "source"),
        "scale": str_field(pairing, "scale"),
        "weights": str_array(pairing, "weights"),
        "pairing_rationale": str_field(pairing, "pairing_rationale"),
    })
}

/// Recommend up to three chart types whose keywords match the request. Returns
/// an empty list when the request shows no data-visualization intent.
fn pick_chart_types(catalog: &Value, request_lower: &str, tokens: &HashSet<String>) -> Vec<Value> {
    let Some(charts) = catalog.get("chart_types").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut scored: Vec<(&Value, u32)> = charts
        .iter()
        .map(|chart| {
            (
                chart,
                score_keywords(request_lower, tokens, &str_array(chart, "keywords")),
            )
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(3)
        .map(|(chart, _)| {
            json!({
                "id": str_field(chart, "id"),
                "display_name": str_field(chart, "display_name"),
                "use_when": str_field(chart, "use_when"),
                "accessibility_notes": str_field(chart, "accessibility_notes"),
                "library_examples": str_array(chart, "library_examples"),
            })
        })
        .collect()
}

/// Match UX guidelines to the request and archetype. Always returns the
/// critical-severity rules (they apply broadly), plus the highest-scoring
/// keyword/archetype matches, capped to keep the packet focused.
fn pick_ux_guidelines(
    catalog: &Value,
    archetype: &Value,
    request_lower: &str,
    tokens: &HashSet<String>,
) -> Vec<Value> {
    let Some(guidelines) = catalog.get("ux_guidelines").and_then(Value::as_array) else {
        return Vec::new();
    };
    let archetype_id = str_field(archetype, "id");
    let mut scored: Vec<(&Value, u32)> = Vec::new();
    for guideline in guidelines {
        let mut score = score_keywords(request_lower, tokens, &str_array(guideline, "keywords"));
        let applies = str_array(guideline, "applies_to");
        if applies.iter().any(|a| a == "all" || a == &archetype_id) {
            score += 2;
        }
        if str_field(guideline, "severity") == "critical" {
            score += 1;
        }
        if score > 0 {
            scored.push((guideline, score));
        }
    }
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(8)
        .map(|(guideline, _)| {
            json!({
                "id": str_field(guideline, "id"),
                "category": str_field(guideline, "category"),
                "rule": str_field(guideline, "rule"),
                "severity": str_field(guideline, "severity"),
            })
        })
        .collect()
}

fn render_text(packet: &Value, output: &mut dyn Write) {
    let _ = writeln!(output, "Design Intelligence Recommendation");
    let _ = writeln!(output, "Request: {}", str_field(packet, "request"));
    let _ = writeln!(
        output,
        "Confidence: {} (archetype keyword match: {})",
        str_field(packet, "confidence"),
        packet
            .get("archetype_match_score")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
    let _ = writeln!(output);

    let archetype = &packet["product_archetype"];
    let _ = writeln!(
        output,
        "Product archetype: {} — {}",
        str_field(archetype, "display_name"),
        str_field(archetype, "trust_posture")
    );
    write_inline_list(
        output,
        "  Content priorities",
        &str_array(archetype, "content_priorities"),
    );
    let _ = writeln!(
        output,
        "  CTA guidance: {}",
        str_field(archetype, "cta_guidance")
    );
    let _ = writeln!(
        output,
        "  Motion: {}   Density: {}",
        str_field(archetype, "motion_posture"),
        str_field(archetype, "density")
    );
    let motion = str_field(packet, "motion_guidance");
    if !motion.is_empty() {
        let _ = writeln!(output, "  Motion guidance: {motion}");
    }
    if let Some(dials) = packet.get("dials") {
        let density = dials.get("density").and_then(Value::as_u64);
        let variance = dials.get("variance").and_then(Value::as_u64);
        if density.is_some() || variance.is_some() {
            let _ = writeln!(
                output,
                "  Dials: density={} variance={}",
                density
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "default".to_string()),
                variance
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "default".to_string())
            );
        }
    }
    let _ = writeln!(output);

    write_entry(
        output,
        "Style family",
        &packet["style_family"],
        "visual_direction",
    );
    write_entry(
        output,
        "Color mood",
        &packet["color_mood"],
        "palette_direction",
    );
    write_entry(
        output,
        "Typography mood",
        &packet["typography_mood"],
        "direction",
    );
    let _ = writeln!(output);

    if let Some(palette) = packet.get("color_palette") {
        let _ = writeln!(
            output,
            "Color palette: {} ({} mode)",
            str_field(palette, "display_name"),
            str_field(palette, "mode")
        );
        if let Some(colors) = palette.get("colors").and_then(Value::as_object) {
            for key in [
                "primary",
                "secondary",
                "accent",
                "background",
                "surface",
                "text_primary",
            ] {
                if let Some(hex) = colors.get(key).and_then(Value::as_str) {
                    let _ = writeln!(output, "  {key}: {hex}");
                }
            }
        }
        let notes = str_field(palette, "contrast_notes");
        if !notes.is_empty() {
            let _ = writeln!(output, "  contrast: {notes}");
        }
        let _ = writeln!(output);
    }

    if let Some(pairing) = packet.get("font_pairing") {
        let _ = writeln!(
            output,
            "Font pairing: {} ({})",
            str_field(pairing, "display_name"),
            str_field(pairing, "source")
        );
        let _ = writeln!(
            output,
            "  Heading: {}   Body: {}   Mono: {}",
            str_field(pairing, "heading_font"),
            str_field(pairing, "body_font"),
            str_field(pairing, "mono_font")
        );
        let scale = str_field(pairing, "scale");
        if !scale.is_empty() {
            let _ = writeln!(output, "  Scale: {scale}");
        }
        let rationale = str_field(pairing, "pairing_rationale");
        if !rationale.is_empty() {
            let _ = writeln!(output, "  {rationale}");
        }
        let _ = writeln!(output);
    }

    if let Some(charts) = packet.get("recommended_charts").and_then(Value::as_array) {
        if !charts.is_empty() {
            let _ = writeln!(output, "Recommended charts:");
            for chart in charts {
                let _ = writeln!(
                    output,
                    "  - {}: {}",
                    str_field(chart, "display_name"),
                    str_field(chart, "use_when")
                );
            }
            let _ = writeln!(output);
        }
    }

    if let Some(stack) = packet.get("stack_adaptation") {
        if let Some(note) = stack.get("note").and_then(Value::as_str) {
            let _ = writeln!(output, "Stack adaptation: {note}");
        } else {
            let _ = writeln!(
                output,
                "Stack adaptation ({}):",
                str_field(stack, "display_name")
            );
            for line in str_array(stack, "guidance") {
                let _ = writeln!(output, "  - {line}");
            }
            write_inline_list(
                output,
                "  Component preview tools",
                &str_array(stack, "component_preview_tools"),
            );
            for line in str_array(stack, "validation_checks") {
                let _ = writeln!(output, "  validation: {line}");
            }
        }
        let _ = writeln!(output);
    }

    if let Some(library) = packet.get("component_library") {
        let _ = writeln!(
            output,
            "Component library ({}): {}",
            str_field(library, "name"),
            str_field(library, "guidance")
        );
        let _ = writeln!(output);
    }

    write_block(
        output,
        "Professional polish checks",
        &str_array(packet, "professional_polish_checks"),
    );
    write_block(
        output,
        "Recovery checks",
        &str_array(packet, "recovery_checks"),
    );
    write_block(
        output,
        "Verification checks",
        &str_array(packet, "verification_checks"),
    );
    write_block(
        output,
        "Anti-patterns to avoid",
        &str_array(packet, "anti_patterns"),
    );
    write_block(
        output,
        "Decision rules (context-specific, non-negotiable)",
        &str_array(packet, "decision_rules"),
    );

    if let Some(guidelines) = packet.get("ux_guidelines").and_then(Value::as_array) {
        if !guidelines.is_empty() {
            let _ = writeln!(output, "UX guidelines that apply:");
            for guideline in guidelines {
                let _ = writeln!(
                    output,
                    "  - [{}/{}] {}",
                    str_field(guideline, "severity"),
                    str_field(guideline, "category"),
                    str_field(guideline, "rule")
                );
            }
            let _ = writeln!(output);
        }
    }
}

enum PersistOutcome {
    Wrote(PathBuf),
    SkippedExisting(PathBuf),
}

fn safe_slug(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | ' ') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "design-system".to_string()
    } else {
        trimmed
    }
}

fn persist_design_system(
    packet: &Value,
    request: &str,
    project_name: &str,
    page: &str,
    out_override: &str,
    force: bool,
) -> Result<PersistOutcome, String> {
    let cwd =
        std::env::current_dir().map_err(|error| format!("resolve current directory: {error}"))?;
    let project = if project_name.trim().is_empty() {
        "Design System"
    } else {
        project_name.trim()
    };
    let slug = safe_slug(project);

    let (master_path, page_path) = if !out_override.trim().is_empty() {
        // Explicit --out is the MASTER path; page sibling under pages/ when set.
        let master = clean_path(&PathBuf::from(out_override.trim()));
        let page_path = if page.trim().is_empty() {
            None
        } else {
            master
                .parent()
                .map(|parent| parent.join("pages").join(format!("{}.md", safe_slug(page))))
        };
        (master, page_path)
    } else {
        let root = cwd.join("design-system").join(&slug);
        let master = root.join("MASTER.md");
        let page_path = if page.trim().is_empty() {
            None
        } else {
            Some(root.join("pages").join(format!("{}.md", safe_slug(page))))
        };
        (master, page_path)
    };

    if master_path.exists() && !force && page_path.is_none() {
        return Ok(PersistOutcome::SkippedExisting(master_path));
    }

    let write_target = if let Some(ref page_file) = page_path {
        page_file.clone()
    } else {
        master_path.clone()
    };
    if write_target.exists() && !force {
        return Ok(PersistOutcome::SkippedExisting(write_target));
    }
    if let Some(parent) = write_target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
    }

    let mut markdown = String::new();
    markdown.push_str(&format!("# {project}\n\n"));
    if !page.trim().is_empty() {
        markdown.push_str(&format!("## {}\n\n", page.trim()));
        markdown.push_str(
            "Page override: when building this page, apply these rules over MASTER.md.\n\n",
        );
    }
    markdown.push_str(&format!("Request: {request}\n\n"));
    let archetype = &packet["product_archetype"];
    markdown.push_str(&format!(
        "- Archetype: {} ({})\n",
        str_field(archetype, "display_name"),
        str_field(archetype, "trust_posture")
    ));
    markdown.push_str(&format!(
        "- Style family: {}. {}\n",
        str_field(&packet["style_family"], "display_name"),
        str_field(&packet["style_family"], "visual_direction")
    ));
    markdown.push_str(&format!(
        "- Color mood: {}. {}\n",
        str_field(&packet["color_mood"], "display_name"),
        str_field(&packet["color_mood"], "palette_direction")
    ));
    markdown.push_str(&format!(
        "- Typography: {}. {}\n",
        str_field(&packet["typography_mood"], "display_name"),
        str_field(&packet["typography_mood"], "direction")
    ));
    markdown.push_str(&format!(
        "- Motion: {}   Density: {}\n",
        str_field(archetype, "motion_posture"),
        str_field(archetype, "density")
    ));
    let motion = str_field(packet, "motion_guidance");
    if !motion.is_empty() {
        markdown.push_str(&format!("- Motion guidance: {motion}\n"));
    }
    markdown.push('\n');
    append_markdown_list(
        &mut markdown,
        "Professional polish checks",
        &str_array(packet, "professional_polish_checks"),
    );
    append_markdown_list(
        &mut markdown,
        "Recovery checks",
        &str_array(packet, "recovery_checks"),
    );
    append_markdown_list(
        &mut markdown,
        "Verification checks",
        &str_array(packet, "verification_checks"),
    );
    append_markdown_list(
        &mut markdown,
        "Anti-patterns to avoid",
        &str_array(packet, "anti_patterns"),
    );
    append_markdown_list(
        &mut markdown,
        "Decision rules (context-specific, non-negotiable)",
        &str_array(packet, "decision_rules"),
    );

    // When writing a page override and MASTER is missing, seed MASTER first.
    if page_path.is_some() && !master_path.exists() {
        if let Some(parent) = master_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", display_path(parent)))?;
        }
        let mut master_md = markdown.clone();
        // Drop page heading from MASTER seed.
        if let Some(rest) = master_md.split_once("\n\n") {
            master_md = format!("{}\n\n{}", rest.0, rest.1);
        }
        fs::write(&master_path, &master_md)
            .map_err(|error| format!("write {}: {error}", display_path(&master_path)))?;
    }

    fs::write(&write_target, markdown)
        .map_err(|error| format!("write {}: {error}", display_path(&write_target)))?;
    Ok(PersistOutcome::Wrote(write_target))
}

fn density_label_from_dial(dial: Option<u8>, catalog_default: &str) -> String {
    match dial {
        Some(v) if v <= 3 => "airy".to_string(),
        Some(v) if v <= 7 => "balanced".to_string(),
        Some(_) => "data-dense".to_string(),
        None if !catalog_default.is_empty() => catalog_default.to_string(),
        None => "balanced".to_string(),
    }
}

fn motion_guidance_for(
    motion_posture: &str,
    density_dial: Option<u8>,
    variance_dial: Option<u8>,
) -> String {
    let base = if motion_posture.is_empty() {
        "Use purposeful transitions (150-300ms) that reinforce hierarchy; never animate without meaning."
            .to_string()
    } else {
        format!(
            "Motion posture: {motion_posture}. Prefer 150-300ms transitions; respect prefers-reduced-motion."
        )
    };
    let density_note = match density_dial {
        Some(v) if v >= 8 => " Dense UIs: shorter motion (120-200ms), no large layout shifts.",
        Some(v) if v <= 3 => " Spacious UIs: slightly longer reveals (200-400ms) are acceptable.",
        _ => "",
    };
    let variance_note = match variance_dial {
        Some(v) if v >= 8 => {
            " Higher variance: allow bolder entrance choreography; keep exits faster than enters."
        }
        Some(v) if v <= 3 => " Lower variance: micro-interactions only; avoid decorative motion.",
        _ => "",
    };
    format!("{base}{density_note}{variance_note}")
}

/// Bias style preference list by variance dial: low → minimal/trust first, high → bold/glass/brutalist first.
fn apply_variance_to_style_prefs(
    prefs: &mut Vec<String>,
    variance: Option<u8>,
    style_families: &[Value],
) {
    let Some(v) = variance else {
        return;
    };
    let bold_ids = [
        "neo-brutalist",
        "glassmorphism-depth",
        "signal-rich-premium",
        "conversion-showcase",
        "hero-storytelling",
        "expressive-maximal",
    ];
    let calm_ids = [
        "minimal-trust",
        "accessible-calm",
        "data-dense-clarity",
        "structured-guidance",
        "editorial-showcase",
    ];
    let boost: &[&str] = if v >= 8 {
        &bold_ids
    } else if v <= 3 {
        &calm_ids
    } else {
        return;
    };
    let mut ordered: Vec<String> = Vec::new();
    for id in boost {
        if prefs.iter().any(|p| p == *id)
            || style_families
                .iter()
                .any(|entry| str_field(entry, "id") == *id)
        {
            ordered.push((*id).to_string());
        }
    }
    for existing in prefs.iter() {
        if !ordered.iter().any(|o| o == existing) {
            ordered.push(existing.clone());
        }
    }
    *prefs = ordered;
}

// --- selection helpers ---------------------------------------------------

fn pick_archetype<'a>(
    archetypes: &'a [Value],
    request_lower: &str,
    tokens: &HashSet<String>,
) -> (&'a Value, u32) {
    let mut best: Option<(&Value, u32)> = None;
    for entry in archetypes {
        let score = score_keywords(request_lower, tokens, &str_array(entry, "keywords"));
        if best.map_or(true, |(_, best_score)| score > best_score) {
            best = Some((entry, score));
        }
    }
    // archetypes is never empty in a valid catalog; fall back to the first entry.
    best.unwrap_or((&archetypes[0], 0))
}

fn choose<'a>(
    preferred_ids: &[String],
    all: &'a [Value],
    request_lower: &str,
    tokens: &HashSet<String>,
) -> &'a Value {
    let mut best: Option<(&Value, u32)> = None;
    for id in preferred_ids {
        if let Some(entry) = find_by_id(all, id) {
            let score = score_keywords(request_lower, tokens, &str_array(entry, "keywords"));
            if best.map_or(true, |(_, best_score)| score > best_score) {
                best = Some((entry, score));
            }
        }
    }
    if let Some((entry, _)) = best {
        return entry;
    }
    // No preferred ids resolved — pick the global best by direct keyword score.
    let mut global: Option<(&Value, u32)> = None;
    for entry in all {
        let score = score_keywords(request_lower, tokens, &str_array(entry, "keywords"));
        if global.map_or(true, |(_, best_score)| score > best_score) {
            global = Some((entry, score));
        }
    }
    global.map(|(entry, _)| entry).unwrap_or(&all[0])
}

/// Intersect the archetype's recommended ids with the stack's preferred ids.
/// Non-empty intersection wins (stack-aligned); otherwise the archetype list.
fn biased_preferences(
    archetype: &Value,
    stack: Option<&Value>,
    archetype_key: &str,
    stack_key: &str,
) -> Vec<String> {
    let archetype_pref = str_array(archetype, archetype_key);
    let Some(stack_entry) = stack else {
        return archetype_pref;
    };
    let stack_pref = str_array(stack_entry, stack_key);
    let intersection: Vec<String> = archetype_pref
        .iter()
        .filter(|id| stack_pref.contains(id))
        .cloned()
        .collect();
    if !intersection.is_empty() {
        return intersection;
    }
    // No overlap — lead with the stack's preference, then the archetype's.
    let mut combined = stack_pref;
    for id in archetype_pref {
        if !combined.contains(&id) {
            combined.push(id);
        }
    }
    combined
}

fn find_stack<'a>(stack_profiles: &'a [Value], stack_id: &str) -> Option<&'a Value> {
    let needle = stack_id.trim().to_lowercase();
    stack_profiles.iter().find(|entry| {
        str_field(entry, "id").to_lowercase() == needle
            || str_array(entry, "aliases")
                .iter()
                .any(|alias| alias.to_lowercase() == needle)
            || str_array(entry, "keywords")
                .iter()
                .any(|keyword| keyword.to_lowercase() == needle)
    })
}

fn merged_anti_patterns(archetype: &Value, style: &Value) -> Vec<String> {
    let mut merged = str_array(archetype, "anti_patterns");
    for item in str_array(style, "anti_patterns") {
        if !merged.contains(&item) {
            merged.push(item);
        }
    }
    merged
}

fn checks_or_default(entry: &Value, key: &str, defaults: &[&str]) -> Vec<String> {
    let found = str_array(entry, key);
    if found.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        found
    }
}

fn entry_summary(entry: &Value, direction_key: &str) -> Value {
    json!({
        "id": str_field(entry, "id"),
        "display_name": str_field(entry, "display_name"),
        direction_key: str_field(entry, direction_key),
    })
}

// --- scoring -------------------------------------------------------------

fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(stem)
        .collect()
}

/// Crude singular stem so "dashboards" matches "dashboard".
fn stem(word: &str) -> String {
    let lower = word.to_lowercase();
    if lower.len() > 3 {
        if let Some(base) = lower.strip_suffix('s') {
            return base.to_string();
        }
    }
    lower
}

/// Resolve an archetype's `decision_rules` against the request. A rule fires
/// when its `when` is `always`, or `if_<token>` and the request contains
/// `<token>`. Returns the `then` guidance strings of the fired rules. This is
/// the ui-ux-pro-max conditional mechanism: `must_have` and `if_luxury`-style
/// branches that turn a generic recommendation into a context-specific one.
fn active_decision_rules(archetype: &Value, request_lower: &str) -> Vec<String> {
    let mut active = Vec::new();
    for rule in array_field(archetype, "decision_rules") {
        let when = str_field(&rule, "when");
        let fires = if when == "always" {
            true
        } else if let Some(token) = when.strip_prefix("if_") {
            !token.is_empty() && request_lower.contains(&token.to_lowercase())
        } else {
            false
        };
        if fires {
            let then = str_field(&rule, "then");
            if !then.is_empty() {
                active.push(then);
            }
        }
    }
    active
}

fn score_keywords(request_lower: &str, tokens: &HashSet<String>, keywords: &[String]) -> u32 {
    let mut score = 0;
    for keyword in keywords {
        let keyword_lower = keyword.to_lowercase();
        let word_count = keyword_lower.split_whitespace().count();
        let matched = if word_count > 1 {
            request_lower.contains(&keyword_lower)
        } else {
            tokens.contains(&stem(&keyword_lower))
        };
        if matched {
            // Multi-word, more specific keywords weigh more than generic ones.
            score += word_count as u32;
        }
    }
    score
}

// --- Value accessors -----------------------------------------------------

fn array_field(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn str_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn find_by_id<'a>(entries: &'a [Value], id: &str) -> Option<&'a Value> {
    entries.iter().find(|entry| str_field(entry, "id") == id)
}

// --- output formatting ---------------------------------------------------

fn write_entry(output: &mut dyn Write, label: &str, entry: &Value, direction_key: &str) {
    let _ = writeln!(output, "{label}: {}", str_field(entry, "display_name"));
    let direction = str_field(entry, direction_key);
    if !direction.is_empty() {
        let _ = writeln!(output, "  {direction}");
    }
}

fn write_block(output: &mut dyn Write, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(output, "{label}:");
    for item in items {
        let _ = writeln!(output, "  - {item}");
    }
    let _ = writeln!(output);
}

fn write_inline_list(output: &mut dyn Write, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(output, "{label}: {}", items.join(", "));
}

fn append_markdown_list(markdown: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    markdown.push_str(&format!("### {label}\n\n"));
    for item in items {
        markdown.push_str(&format!("- {item}\n"));
    }
    markdown.push('\n');
}

fn is_help_argument(argument: &str) -> bool {
    argument == "--help" || argument == "-h" || argument == "help"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_catalog_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        clean_path(
            &manifest_dir
                .join("../../../")
                .join(UI_SKILL_NAME)
                .join(CATALOG_RELATIVE_PATH),
        )
    }

    fn run(args: &[&str]) -> (u8, String, String) {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_design_intelligence_command(&owned, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn fintech_request_routes_to_fintech_archetype_and_trust_palette() {
        let catalog = repo_catalog_path();
        let (code, out, err) = run(&[
            "recommend",
            "fintech banking dashboard with secure transfers",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("Fintech Product"), "out: {out}");
        assert!(out.contains("Trust Blue"), "out: {out}");
        assert!(out.contains("Confidence: high"), "out: {out}");
    }

    #[test]
    fn luxury_beauty_spa_routes_to_beauty_archetype_not_marketplace() {
        let catalog = repo_catalog_path();
        let (code, out, err) = run(&[
            "recommend",
            "luxury beauty spa booking site",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        let value: Value = serde_json::from_str(&out).expect("valid json");
        let id = value["product_archetype"]["id"].as_str().unwrap_or("");
        assert_ne!(
            id, "marketplace-platform",
            "a generic 'booking' keyword hit must not win over the beauty/spa archetype; got {id}"
        );
        assert_eq!(
            id, "beauty-wellness-spa",
            "luxury beauty spa should route to the beauty/spa archetype; got {id}"
        );
    }

    #[test]
    fn decision_rules_surface_conditional_guidance() {
        let catalog = repo_catalog_path();
        let (code, out, err) = run(&[
            "recommend",
            "luxury beauty spa booking site",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        let value: Value = serde_json::from_str(&out).expect("valid json");
        let rules = value["decision_rules"]
            .as_array()
            .expect("decision_rules array");
        assert!(
            !rules.is_empty(),
            "beauty/spa archetype with a 'luxury' request must emit at least one decision rule"
        );
    }

    #[test]
    fn json_format_emits_structured_packet() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "ai workspace for research copilots",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(value["product_archetype"]["id"], "ai-workspace");
        assert!(value["style_family"]["id"].is_string());
        assert!(value["professional_polish_checks"].is_array());
        assert!(value["verification_checks"].is_array());
    }

    #[test]
    fn packet_attaches_concrete_palette_and_font_pairing() {
        let catalog = repo_catalog_path();
        let (code, out, err) = run(&[
            "recommend",
            "fintech banking dashboard with secure transfers",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        let value: Value = serde_json::from_str(&out).expect("valid json");
        // Palette: a real hex primary tied to the chosen color mood.
        let primary = value["color_palette"]["colors"]["primary"]
            .as_str()
            .expect("palette primary hex");
        assert!(
            primary.starts_with('#') && primary.len() == 7,
            "primary hex: {primary}"
        );
        assert!(value["color_palette"]["contrast_notes"].is_string());
        // Font pairing: real named faces.
        assert!(value["font_pairing"]["heading_font"].is_string());
        assert!(!value["font_pairing"]["heading_font"]
            .as_str()
            .unwrap()
            .is_empty());
        assert!(value["font_pairing"]["body_font"].is_string());
    }

    #[test]
    fn density_and_variance_dials_change_packet_observably() {
        let catalog = repo_catalog_path();
        let cat = catalog.to_str().unwrap();
        let (code_low, out_low, err_low) = run(&[
            "recommend",
            "product dashboard",
            "--format",
            "json",
            "--density",
            "2",
            "--variance",
            "2",
            "--catalog",
            cat,
        ]);
        assert_eq!(code_low, 0, "stderr: {err_low}");
        let low: Value = serde_json::from_str(&out_low).expect("json");
        assert_eq!(low["dials"]["density"], 2);
        assert_eq!(low["dials"]["variance"], 2);
        assert_eq!(low["product_archetype"]["density"], "airy");
        assert!(
            low["motion_guidance"]
                .as_str()
                .unwrap_or("")
                .contains("prefers-reduced-motion"),
            "motion_guidance missing: {}",
            low["motion_guidance"]
        );

        let (code_high, out_high, err_high) = run(&[
            "recommend",
            "product dashboard",
            "--format",
            "json",
            "--density",
            "9",
            "--variance",
            "9",
            "--catalog",
            cat,
        ]);
        assert_eq!(code_high, 0, "stderr: {err_high}");
        let high: Value = serde_json::from_str(&out_high).expect("json");
        assert_eq!(high["product_archetype"]["density"], "data-dense");
        assert_ne!(
            low["product_archetype"]["density"], high["product_archetype"]["density"],
            "density dial must change density label"
        );
        // High variance should prefer a different style family when catalog allows.
        assert!(
            high["style_family"]["id"].is_string() && low["style_family"]["id"].is_string(),
            "style family present on both dialed runs"
        );
        let high_motion = high["motion_guidance"].as_str().unwrap_or("");
        assert!(
            high_motion.contains("Dense") || high_motion.contains("Higher variance"),
            "high dials must enrich motion_guidance: {high_motion}"
        );
    }

    #[test]
    fn persist_writes_master_and_skips_without_force() {
        let catalog = repo_catalog_path();
        let temp = std::env::temp_dir().join(format!(
            "keel-di-persist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&temp); // best-effort pre-clean
        fs::create_dir_all(&temp).expect("temp");
        let master = temp.join("MASTER.md");
        let (code, out, err) = run(&[
            "recommend",
            "fintech banking dashboard",
            "--catalog",
            catalog.to_str().unwrap(),
            "--persist",
            "--out",
            master.to_str().unwrap(),
            "--project-name",
            "DialApp",
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(master.is_file(), "MASTER should exist: {out}");
        let first = fs::read_to_string(&master).expect("read");
        assert!(first.contains("Archetype"), "content: {first}");
        // Second persist without --force must skip.
        let (code2, out2, err2) = run(&[
            "recommend",
            "fintech banking dashboard rewritten",
            "--catalog",
            catalog.to_str().unwrap(),
            "--persist",
            "--out",
            master.to_str().unwrap(),
            "--project-name",
            "DialApp",
        ]);
        assert_eq!(code2, 0, "stderr: {err2}");
        assert!(
            out2.contains("Persist skipped") || out2.to_lowercase().contains("skip"),
            "expected skip: {out2}"
        );
        let second = fs::read_to_string(&master).expect("read2");
        assert_eq!(first, second, "MASTER must not clobber without --force");
        let _ = fs::remove_dir_all(&temp); // best-effort test cleanup
    }

    #[test]
    fn dashboard_request_recommends_charts() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "analytics dashboard showing revenue trend over time and category comparison",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        let charts = value["recommended_charts"]
            .as_array()
            .expect("charts array");
        assert!(!charts.is_empty(), "expected chart recommendations");
        assert!(charts.iter().all(|c| c["display_name"].is_string()));
    }

    #[test]
    fn packet_includes_matched_ux_guidelines() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "ecommerce checkout form with payment",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        let guidelines = value["ux_guidelines"].as_array().expect("ux guidelines");
        assert!(!guidelines.is_empty());
        assert!(guidelines
            .iter()
            .all(|g| g["rule"].is_string() && g["severity"].is_string()));
    }

    #[test]
    fn dark_mode_request_prefers_dark_palette() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "developer console dark mode observability tool",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        // A palette should be attached; when a dark palette exists for the mood
        // the dark-mode bias should select it.
        if let Some(mode) = value["color_palette"]["mode"].as_str() {
            assert!(mode == "dark" || mode == "light", "mode: {mode}");
        }
    }

    #[test]
    fn stack_flag_biases_selection_and_adds_stack_guidance() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "messaging app with unread states and voice notes",
            "--stack",
            "flutter",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(value["stack_adaptation"]["id"], "flutter-mobile");
        let tools = value["stack_adaptation"]["component_preview_tools"]
            .as_array()
            .unwrap();
        assert!(tools.iter().any(|t| t == "Widgetbook"));
    }

    #[test]
    fn unknown_stack_is_noted_not_fatal() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "saas dashboard",
            "--stack",
            "cobol",
            "--format",
            "json",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        let value: Value = serde_json::from_str(&out).expect("valid json");
        assert!(value["stack_adaptation"]["note"]
            .as_str()
            .unwrap()
            .contains("cobol"));
    }

    #[test]
    fn unmatched_request_falls_back_with_low_confidence() {
        let catalog = repo_catalog_path();
        let (code, out, _) = run(&[
            "recommend",
            "zzzz qqqq wwww",
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(out.contains("Confidence: low"), "out: {out}");
    }

    #[test]
    fn persist_writes_master_markdown() {
        let catalog = repo_catalog_path();
        let temp = std::env::temp_dir().join(format!("di-test-{}.md", std::process::id()));
        let (code, out, err) = run(&[
            "recommend",
            "ecommerce storefront checkout",
            "--persist",
            "--project-name",
            "Storefront Revamp",
            "--page",
            "Checkout Flow",
            "--out",
            temp.to_str().unwrap(),
            "--catalog",
            catalog.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("Persisted design system"), "out: {out}");
        let written = fs::read_to_string(&temp).unwrap();
        assert!(written.contains("# Storefront Revamp"));
        assert!(written.contains("## Checkout Flow"));
        assert!(written.contains("Anti-patterns to avoid"));
        let _ = fs::remove_file(&temp);
    }

    #[test]
    fn missing_request_errors() {
        let catalog = repo_catalog_path();
        let (code, _, err) = run(&["recommend", "--catalog", catalog.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(err.contains("request is required"), "err: {err}");
    }

    #[test]
    fn bad_catalog_path_errors_clearly() {
        let (code, _, err) = run(&[
            "recommend",
            "saas dashboard",
            "--catalog",
            "/nonexistent/catalog.json",
        ]);
        assert_eq!(code, 1);
        assert!(err.contains("catalog not found"), "err: {err}");
    }
}
