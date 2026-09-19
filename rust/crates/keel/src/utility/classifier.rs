//! Purpose: Native multi-dimensional zero-shot classification engine (K-Native-1).
//! Caller: `utility::decision`, `utility::skill_match`, `commands`.
//! Dependencies: std, serde, serde_json, crate::utility::calibration.
//! Main Functions: MultiDimClassifier, ClassificationAxis, AxisLabel, MultiDimResult.
//! Side Effects: None. Pure in-memory deterministic classification.

use crate::utility::calibration::laplace_rate;
use serde::{Deserialize, Serialize};

/// A label candidate within a classification axis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisLabel {
    pub name: String,
    pub description: String,
    pub anchors: Vec<String>,
    pub negative_anchors: Vec<String>,
    pub weight: f64,
}

impl AxisLabel {
    pub fn new(name: &str, description: &str, anchors: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            anchors: anchors.iter().map(|s| s.to_ascii_lowercase()).collect(),
            negative_anchors: Vec::new(),
            weight: 1.0,
        }
    }

    pub fn with_negatives(mut self, negatives: &[&str]) -> Self {
        self.negative_anchors = negatives.iter().map(|s| s.to_ascii_lowercase()).collect();
        self
    }

    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight.max(0.1);
        self
    }
}

/// A single orthogonal dimension / axis of classification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassificationAxis {
    pub name: String,
    pub description: String,
    pub labels: Vec<AxisLabel>,
    pub allow_multi_label: bool,
}

impl ClassificationAxis {
    pub fn new(name: &str, description: &str, labels: Vec<AxisLabel>) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            labels,
            allow_multi_label: false,
        }
    }

    pub fn new_multi_label(name: &str, description: &str, labels: Vec<AxisLabel>) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            labels,
            allow_multi_label: true,
        }
    }
}

/// Result of evaluating an input against a single axis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisMatch {
    pub axis_name: String,
    pub top_label: String,
    pub confidence: f64,
    pub calibrated_rate: f64,
    pub distribution: Vec<(String, f64)>,
    pub secondary_labels: Vec<String>,
}

/// Full multi-dimensional classification result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MultiDimResult {
    pub input_length: usize,
    pub axes: Vec<AxisMatch>,
    pub joint_confidence: f64,
    pub execution_time_micros: u64,
}

impl MultiDimResult {
    pub fn top_label_for(&self, axis_name: &str) -> Option<&str> {
        self.axes
            .iter()
            .find(|a| a.axis_name.eq_ignore_ascii_case(axis_name))
            .map(|a| a.top_label.as_str())
    }

    pub fn confidence_for(&self, axis_name: &str) -> Option<f64> {
        self.axes
            .iter()
            .find(|a| a.axis_name.eq_ignore_ascii_case(axis_name))
            .map(|a| a.confidence)
    }

    pub fn is_confident(&self, threshold: f64) -> bool {
        self.axes.iter().all(|a| a.confidence >= threshold)
    }
}

/// The multi-dimensional classification engine.
#[derive(Debug, Clone)]
pub struct MultiDimClassifier {
    axes: Vec<ClassificationAxis>,
}

impl Default for MultiDimClassifier {
    fn default() -> Self {
        Self::new_default()
    }
}

impl MultiDimClassifier {
    pub fn new(axes: Vec<ClassificationAxis>) -> Self {
        Self { axes }
    }

    /// Construct classifier with the canonical default developer axes:
    /// 1. Domain (Rust, Flutter/Dart, Web, Python, DevOps, General)
    /// 2. Action (Design, Implement, Refactor, Debug, Test, Review)
    /// 3. BlastRadius (Readonly, WorkspaceEdit, Destructive, GlobalEnv)
    pub fn new_default() -> Self {
        let domain_axis = ClassificationAxis::new(
            "domain",
            "Target programming language, technology stack, or subsystem",
            vec![
                AxisLabel::new(
                    "rust",
                    "Rust systems programming, Cargo, crates, memory safety",
                    &[
                        "rust", "cargo", "tokio", "serde", "crates", "borrow", "lifetime", "impl",
                        "struct", "enum", "clippy", "rustc", "unsafe", "trait", "async", "syn",
                    ],
                ),
                AxisLabel::new(
                    "flutter_dart",
                    "Flutter UI framework and Dart language",
                    &[
                        "flutter",
                        "dart",
                        "widget",
                        "pubspec",
                        "stateful",
                        "stateless",
                        "bloc",
                        "riverpod",
                        "material",
                        "scaffold",
                        "navigator",
                        "pub",
                        "provider",
                        "renderflex",
                        "column",
                        "row",
                        "padding",
                    ],
                ),
                AxisLabel::new(
                    "web_typescript",
                    "Web frontend, TypeScript, JavaScript, React, Node",
                    &[
                        "typescript",
                        "javascript",
                        "react",
                        "vue",
                        "angular",
                        "html",
                        "css",
                        "tailwind",
                        "vite",
                        "nextjs",
                        "npm",
                        "node",
                        "webpack",
                        "jsx",
                        "tsx",
                    ],
                ),
                AxisLabel::new(
                    "python_data",
                    "Python, machine learning, data engineering, backend",
                    &[
                        "python", "pandas", "numpy", "pytorch", "fastapi", "django", "sklearn",
                        "jupyter", "notebook", "pip", "uv", "pytest", "pydantic",
                    ],
                ),
                AxisLabel::new(
                    "devops_infra",
                    "Containers, CI/CD, orchestration, cloud infra, shell scripts",
                    &[
                        "docker",
                        "k8s",
                        "kubernetes",
                        "bash",
                        "ci",
                        "github actions",
                        "workflow",
                        "terraform",
                        "ansible",
                        "nginx",
                        "helm",
                        "linux",
                        "powershell",
                        "env",
                    ],
                ),
                AxisLabel::new(
                    "general",
                    "General software development or documentation",
                    &[
                        "docs", "readme", "markdown", "general", "text", "comment", "guide",
                    ],
                ),
            ],
        );

        let action_axis = ClassificationAxis::new(
            "action",
            "Intent and lifecycle stage of the task",
            vec![
                AxisLabel::new(
                    "design",
                    "Architecture, system design, RFC, blueprints, planning",
                    &[
                        "design",
                        "architecture",
                        "rfc",
                        "blueprint",
                        "pattern",
                        "plan",
                        "system map",
                        "strategy",
                        "schema",
                        "propose",
                        "spec",
                    ],
                ),
                AxisLabel::new(
                    "implement",
                    "Creating new features, modules, endpoints, or implementations",
                    &[
                        "implement",
                        "create",
                        "build",
                        "add",
                        "feature",
                        "new",
                        "write",
                        "introduce",
                        "generate",
                        "produce",
                    ],
                ),
                AxisLabel::new(
                    "refactor",
                    "Restructuring, optimizing, simplifying, or modernizing existing code",
                    &[
                        "refactor",
                        "restructure",
                        "clean up",
                        "reorganize",
                        "simplify",
                        "modernize",
                        "migrate",
                        "split",
                        "extract",
                        "deduplicate",
                    ],
                ),
                AxisLabel::new(
                    "debug",
                    "Diagnosing and fixing bugs, crashes, panics, or regressions",
                    &[
                        "fix",
                        "debug",
                        "bug",
                        "error",
                        "panic",
                        "issue",
                        "crash",
                        "failure",
                        "traceback",
                        "repair",
                        "resolve",
                        "broken",
                        "flake",
                        "regression",
                    ],
                ),
                AxisLabel::new(
                    "test",
                    "Writing, updating, running, or analyzing tests and benchmarks",
                    &[
                        "test",
                        "benchmark",
                        "verify",
                        "assert",
                        "coverage",
                        "integration test",
                        "unit test",
                        "spec",
                        "mock",
                        "validation",
                        "testing",
                    ],
                ),
                AxisLabel::new(
                    "review",
                    "Code review, auditing, static analysis, linting, quality gating",
                    &[
                        "review",
                        "audit",
                        "critique",
                        "inspect",
                        "lint",
                        "vulnerability",
                        "security",
                        "check",
                        "examine",
                        "assessment",
                        "gate",
                    ],
                ),
            ],
        );

        let blast_radius_axis = ClassificationAxis::new(
            "blast_radius",
            "Operational risk and mutation scope",
            vec![
                AxisLabel::new(
                    "readonly",
                    "Non-mutating inquiry, search, read, explain, or inspection",
                    &[
                        "read",
                        "view",
                        "search",
                        "grep",
                        "list",
                        "find",
                        "check",
                        "explain",
                        "investigate",
                        "show",
                        "describe",
                        "look up",
                        "diff",
                        "status",
                    ],
                )
                .with_negatives(&[
                    "delete",
                    "rm",
                    "overwrite",
                    "write",
                    "update",
                    "modify",
                ]),
                AxisLabel::new(
                    "workspace_edit",
                    "Standard development edits within the project workspace",
                    &[
                        "edit",
                        "modify",
                        "update",
                        "write",
                        "add",
                        "create",
                        "refactor",
                        "implement",
                        "fix",
                        "patch",
                        "format",
                    ],
                ),
                AxisLabel::new(
                    "destructive",
                    "High-risk deletions, terminations, drops, wipes, or force overrides",
                    &[
                        "delete",
                        "remove",
                        "drop",
                        "purge",
                        "truncate",
                        "kill",
                        "terminate",
                        "wipe",
                        "force",
                        "destroy",
                        "prune",
                        "rmdir",
                        "clean -f",
                    ],
                ),
                AxisLabel::new(
                    "global_env",
                    "System-wide mutations, global installs, production deployments, releases",
                    &[
                        "sudo",
                        "install -g",
                        "global",
                        "deploy",
                        "publish",
                        "release",
                        "production",
                        "cluster",
                        "systemctl",
                        "registry",
                    ],
                ),
            ],
        );

        Self::new(vec![domain_axis, action_axis, blast_radius_axis])
    }

    /// Perform multi-dimensional classification on arbitrary input text.
    pub fn classify(&self, input: &str) -> MultiDimResult {
        let start = std::time::Instant::now();
        let tokens = tokenize_text(input);

        let mut axis_matches = Vec::with_capacity(self.axes.len());
        let mut joint_conf = 1.0;

        for axis in &self.axes {
            let axis_match = self.classify_axis(axis, &tokens, input);
            joint_conf *= axis_match.confidence;
            axis_matches.push(axis_match);
        }

        let elapsed = start.elapsed().as_micros() as u64;

        MultiDimResult {
            input_length: input.len(),
            axes: axis_matches,
            joint_confidence: joint_conf.clamp(0.0, 1.0),
            execution_time_micros: elapsed,
        }
    }

    fn classify_axis(
        &self,
        axis: &ClassificationAxis,
        tokens: &[String],
        raw_text: &str,
    ) -> AxisMatch {
        let lower_raw = raw_text.to_ascii_lowercase();
        let mut raw_scores: Vec<(String, f64)> = Vec::with_capacity(axis.labels.len());

        for label in &axis.labels {
            let mut score = 0.0;

            // 1. Direct anchor token and substring match
            for anchor in &label.anchors {
                let anchor_tokens: Vec<&str> = anchor.split_whitespace().collect();
                if anchor_tokens.len() == 1 {
                    let single = anchor_tokens[0];
                    let count = tokens.iter().filter(|t| t.as_str() == single).count();
                    if count > 0 {
                        score += 2.0 * (count as f64).min(3.0);
                    } else if single.len() >= 4 && lower_raw.contains(single) {
                        score += 1.0;
                    }
                } else {
                    // Multi-token phrase
                    if lower_raw.contains(anchor) {
                        score += 3.5;
                    }
                }
            }

            // 2. Negative anchor penalty
            for neg in &label.negative_anchors {
                if lower_raw.contains(neg) {
                    score -= 4.0;
                }
            }

            // Apply label base weight
            score = (score * label.weight).max(0.0);
            raw_scores.push((label.name.clone(), score));
        }

        // Laplace smoothing across all candidates in this axis
        let total_score: f64 = raw_scores.iter().map(|(_, s)| *s).sum();
        let k = axis.labels.len() as f64;
        let mut distribution: Vec<(String, f64)> = raw_scores
            .iter()
            .map(|(name, s)| {
                let smoothed_prob = (s + 1.0) / (total_score + k);
                (name.clone(), smoothed_prob)
            })
            .collect();

        // Sort descending by probability
        distribution.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let top_label = distribution
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let top_prob = distribution.first().map(|(_, p)| *p).unwrap_or(0.5);

        // Calibrated confidence using Laplace rate against sample pseudo-trials
        let pseudo_total = (total_score * 2.0).round() as usize;
        let pseudo_correct = ((total_score * 2.0) * top_prob).round() as usize;
        let calibrated = laplace_rate(pseudo_total, pseudo_correct);

        let secondary_labels = if axis.allow_multi_label {
            distribution
                .iter()
                .skip(1)
                .filter(|(_, p)| *p >= 0.25)
                .map(|(n, _)| n.clone())
                .collect()
        } else {
            Vec::new()
        };

        AxisMatch {
            axis_name: axis.name.clone(),
            top_label,
            confidence: top_prob.clamp(0.0, 1.0),
            calibrated_rate: calibrated.clamp(0.0, 1.0),
            distribution,
            secondary_labels,
        }
    }
}

/// Tokenize raw text into lowercase words stripped of punctuation.
pub fn tokenize_text(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multidim_default_classification() {
        let classifier = MultiDimClassifier::new_default();

        let rust_query =
            "Please refactor the Cargo crate and write unit tests for borrow checker lifetimes";
        let res = classifier.classify(rust_query);

        assert_eq!(res.top_label_for("domain"), Some("rust"));
        assert!(res.confidence_for("domain").unwrap() > 0.3);

        // Action can be refactor or test
        let action = res.top_label_for("action").unwrap();
        assert!(action == "refactor" || action == "test");

        // Blast radius should be workspace_edit
        assert_eq!(res.top_label_for("blast_radius"), Some("workspace_edit"));
        assert!(res.execution_time_micros < 5000); // Super fast (<5ms)
    }

    #[test]
    fn test_flutter_destructive_query() {
        let classifier = MultiDimClassifier::new_default();
        let query = "Delete the Flutter pubspec lock and wipe the widget build directory";
        let res = classifier.classify(query);

        assert_eq!(res.top_label_for("domain"), Some("flutter_dart"));
        assert_eq!(res.top_label_for("blast_radius"), Some("destructive"));
    }

    #[test]
    fn test_readonly_search_query() {
        let classifier = MultiDimClassifier::new_default();
        let query = "Search and grep for all occurrences of Error enum across the codebase";
        let res = classifier.classify(query);

        assert_eq!(res.top_label_for("blast_radius"), Some("readonly"));
    }
}
