//! Purpose: 100% offline, zero-network sub-word semantic centroid engine (K-Native-3).
//! Caller: `utility::skill_match`, `utility::classifier`, `utility::decision`.
//! Dependencies: std, serde, serde_json.
//! Main Functions: SemanticCentroidEngine, compute_subword_profile, cosine_similarity.
//! Side Effects: None. Pure deterministic in-memory calculation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sparse sub-word n-gram frequency profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SubwordProfile {
    pub ngrams: BTreeMap<String, f32>,
    pub norm: f32,
}

impl SubwordProfile {
    pub fn from_text(text: &str) -> Self {
        let normalized = text.to_ascii_lowercase();
        let mut ngrams: BTreeMap<String, f32> = BTreeMap::new();

        // Extract 3-grams and 4-grams
        let chars: Vec<char> = normalized.chars().collect();
        let len = chars.len();

        if len >= 3 {
            for i in 0..=len - 3 {
                let g3: String = chars[i..i + 3].iter().collect();
                *ngrams.entry(g3).or_insert(0.0) += 1.0;
            }
        }

        if len >= 4 {
            for i in 0..=len - 4 {
                let g4: String = chars[i..i + 4].iter().collect();
                *ngrams.entry(g4).or_insert(0.0) += 1.5; // Slightly higher weight for 4-grams
            }
        }

        // Word-level token boost
        for word in normalized.split_whitespace() {
            let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            if clean.len() >= 3 {
                *ngrams.entry(format!("${clean}$")).or_insert(0.0) += 3.0;
            }
        }

        // Calculate L2 norm
        let sum_sq: f32 = ngrams.values().map(|v| v * v).sum();
        let norm = sum_sq.sqrt().max(1e-6);

        Self { ngrams, norm }
    }

    /// Compute cosine similarity against another subword profile.
    pub fn cosine_similarity(&self, other: &SubwordProfile) -> f32 {
        if self.ngrams.is_empty() || other.ngrams.is_empty() {
            return 0.0;
        }

        let mut dot_product = 0.0;

        // Iterate over smaller map for efficiency
        let (smaller, larger) = if self.ngrams.len() <= other.ngrams.len() {
            (&self.ngrams, &other.ngrams)
        } else {
            (&other.ngrams, &self.ngrams)
        };

        for (ngram, val_a) in smaller {
            if let Some(val_b) = larger.get(ngram) {
                dot_product += val_a * val_b;
            }
        }

        (dot_product / (self.norm * other.norm)).clamp(0.0, 1.0)
    }
}

/// A registered semantic target (skill or domain concept).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticTarget {
    pub identifier: String,
    pub description: String,
    pub synonyms: Vec<String>,
    pub profile: SubwordProfile,
}

impl SemanticTarget {
    pub fn new(identifier: &str, description: &str, synonyms: &[&str]) -> Self {
        let mut combined_text = format!("{} {}", identifier, description);
        for syn in synonyms {
            combined_text.push(' ');
            combined_text.push_str(syn);
        }

        let profile = SubwordProfile::from_text(&combined_text);

        Self {
            identifier: identifier.to_string(),
            description: description.to_string(),
            synonyms: synonyms.iter().map(|s| s.to_string()).collect(),
            profile,
        }
    }
}

/// Match score for a semantic candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticMatch {
    pub identifier: String,
    pub similarity: f32,
    pub confidence: f64,
}

/// Pure offline semantic centroid engine.
#[derive(Debug, Clone)]
pub struct SemanticCentroidEngine {
    targets: Vec<SemanticTarget>,
}

impl Default for SemanticCentroidEngine {
    fn default() -> Self {
        Self::new_builtins()
    }
}

impl SemanticCentroidEngine {
    pub fn new(targets: Vec<SemanticTarget>) -> Self {
        Self { targets }
    }

    /// Pre-compiled built-in targets for developer skill routing and semantic search.
    pub fn new_builtins() -> Self {
        let targets = vec![
            SemanticTarget::new(
                "flutter-build-responsive-layout",
                "Build responsive adaptive layouts across screen sizes",
                &[
                    "layout constraints",
                    "adaptive ui",
                    "flexbox",
                    "mediaquery",
                    "expanded flexible",
                    "screen orientation",
                    "breakpoint",
                    "resize",
                    "mobile tablet desktop layout",
                ],
            ),
            SemanticTarget::new(
                "memory-leak-debugging",
                "Diagnose memory leaks heap profiles and out of memory errors",
                &[
                    "oom",
                    "heap snapshot",
                    "garbage collection",
                    "memory retention",
                    "leak detection",
                    "memlab",
                    "v8 heap",
                    "unbounded growth",
                    "dangling references",
                ],
            ),
            SemanticTarget::new(
                "a11y-debugging",
                "Audit accessibility semantic html screen reader contrast",
                &[
                    "accessibility",
                    "wcag",
                    "aria label",
                    "color contrast",
                    "focus state",
                    "keyboard navigation",
                    "screenreader",
                    "alt text",
                    "semantic tags",
                ],
            ),
            SemanticTarget::new(
                "dart-add-unit-test",
                "Write unit tests test fixtures and test assertions",
                &[
                    "unit test",
                    "test suite",
                    "mocking",
                    "matcher",
                    "expect",
                    "regression test",
                    "test coverage",
                    "automated assertion",
                ],
            ),
            SemanticTarget::new(
                "conformal-calibration",
                "Statistical calibration conformal risk control error coverage",
                &[
                    "conformal prediction",
                    "nonconformity",
                    "brier score",
                    "expected calibration error",
                    "laplace rate",
                    "quantile threshold",
                    "coverage guarantee",
                ],
            ),
            SemanticTarget::new(
                "rust-borrow-lifetimes",
                "Rust borrow checker lifetime annotations and ownership",
                &[
                    "borrow checker",
                    "lifetime",
                    "dangling pointer",
                    "use after free",
                    "deref",
                    "reference counting",
                    "rc arc mutex",
                    "move semantics",
                ],
            ),
        ];

        Self::new(targets)
    }

    /// Add a dynamic target (e.g. from local workspace skills).
    pub fn add_target(&mut self, identifier: &str, description: &str, synonyms: &[&str]) {
        self.targets
            .push(SemanticTarget::new(identifier, description, synonyms));
    }

    /// Find the top matching targets for a query using pure subword cosine similarity.
    pub fn match_query(&self, query: &str, top_k: usize) -> Vec<SemanticMatch> {
        let query_profile = SubwordProfile::from_text(query);
        let mut matches = Vec::with_capacity(self.targets.len());

        for target in &self.targets {
            let sim = query_profile.cosine_similarity(&target.profile);
            let conf = sim as f64;
            matches.push(SemanticMatch {
                identifier: target.identifier.clone(),
                similarity: sim,
                confidence: conf,
            });
        }

        // Sort descending by similarity
        matches.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(top_k);
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subword_synonym_matching() {
        let engine = SemanticCentroidEngine::new_builtins();

        // Query uses synonyms: "re-align layout constraints across different screen sizes"
        // Target is "flutter-build-responsive-layout"
        let matches = engine.match_query(
            "re-align layout constraints across different screen sizes",
            3,
        );
        assert!(!matches.is_empty());
        assert_eq!(matches[0].identifier, "flutter-build-responsive-layout");
        assert!(matches[0].similarity > 0.2);
    }

    #[test]
    fn test_memory_leak_synonym_matching() {
        let engine = SemanticCentroidEngine::new_builtins();

        let matches = engine.match_query(
            "App crashes with out of memory and heap allocation spikes",
            3,
        );
        assert!(!matches.is_empty());
        assert_eq!(matches[0].identifier, "memory-leak-debugging");
    }

    #[test]
    fn test_accessibility_synonym_matching() {
        let engine = SemanticCentroidEngine::new_builtins();

        let matches = engine.match_query("Check contrast ratios and screen reader focus labels", 3);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].identifier, "a11y-debugging");
    }
}
