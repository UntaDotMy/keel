use serde_json::Value;

use crate::runtime::write_text;
use crate::utility::anvil::cache;
use crate::utility::anvil::job;

pub struct Report {
    /// Provider-reported cache metrics. `None` means the host did not expose
    /// machine-readable usage; it is not a measured zero.
    pub cache_hit_ratio: Option<f64>,
    pub tokens_uncached: Option<u64>,
    pub tokens_cached: Option<u64>,
    pub model_input_tokens: Option<u64>,
    pub model_output_tokens: Option<u64>,
    pub model_usage_source: Option<String>,
    /// Process output accounting is intentionally separate from model usage.
    pub output_bytes: u64,
    pub output_token_estimate: Option<u64>,
    pub critic_calls: u64,
    pub gate_pass_rate: f64,
    pub stamp_used: bool,
    pub winner_id: String,
    pub loop_iterations: u64,
    pub improvement_delta: f64,
    pub refinement_feedback: Option<String>,
}

pub fn empty_report() -> Report {
    Report {
        cache_hit_ratio: None,
        tokens_uncached: None,
        tokens_cached: None,
        model_input_tokens: None,
        model_output_tokens: None,
        model_usage_source: None,
        output_bytes: 0,
        output_token_estimate: None,
        critic_calls: 0,
        gate_pass_rate: 0.0,
        stamp_used: false,
        winner_id: "none".into(),
        loop_iterations: 0,
        improvement_delta: 0.0,
        refinement_feedback: None,
    }
}

impl Report {
    pub fn metrics_line(&self) -> String {
        format!(
            "cache_hit_ratio={} tokens_uncached={} tokens_cached={} model_input_tokens={} model_output_tokens={} output_bytes={} output_token_estimate={} critic_calls={} gate_pass_rate={:.2} stamp_used={} winner_id={} loop_iterations={} improvement_delta={:.3} model_usage_source={}",
            optional_ratio(self.cache_hit_ratio),
            optional_u64(self.tokens_uncached),
            optional_u64(self.tokens_cached),
            optional_u64(self.model_input_tokens),
            optional_u64(self.model_output_tokens),
            self.output_bytes,
            optional_u64(self.output_token_estimate),
            self.critic_calls,
            self.gate_pass_rate,
            self.stamp_used,
            self.winner_id,
            self.loop_iterations,
            self.improvement_delta,
            self.model_usage_source.as_deref().unwrap_or("unavailable")
        )
    }
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "cache_hit_ratio": self.cache_hit_ratio,
            "tokens_uncached": self.tokens_uncached,
            "tokens_cached": self.tokens_cached,
            "model_input_tokens": self.model_input_tokens,
            "model_output_tokens": self.model_output_tokens,
            "model_usage_source": self.model_usage_source,
            "output_bytes": self.output_bytes,
            "output_token_estimate": self.output_token_estimate,
            "critic_calls": self.critic_calls,
            "gate_pass_rate": self.gate_pass_rate,
            "stamp_used": self.stamp_used,
            "winner_id": self.winner_id,
            "loop_iterations": self.loop_iterations,
            "improvement_delta": self.improvement_delta,
            "refinement_feedback": self.refinement_feedback,
        })
    }

    pub fn apply_cast_metrics(&mut self, metrics: &CastMetrics) {
        self.output_bytes = metrics.output_bytes;
        self.output_token_estimate =
            (metrics.output_bytes > 0).then_some(metrics.output_token_estimate);
        if !metrics.measured_usage {
            return;
        }
        self.model_input_tokens = metrics.usage.input_tokens;
        self.model_output_tokens = metrics.usage.output_tokens;
        self.tokens_cached = metrics.usage.cache_read_input_tokens;
        self.tokens_uncached = metrics
            .usage
            .input_tokens
            .zip(metrics.usage.cache_read_input_tokens)
            .map(|(input, cached)| input.saturating_sub(cached));
        self.cache_hit_ratio = metrics.usage.cache_hit_ratio();
        self.model_usage_source = metrics
            .usage_source
            .clone()
            .or_else(|| Some("host-cli-json".into()));
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CastMetrics {
    pub usage: cache::ProviderUsage,
    pub measured_usage: bool,
    pub usage_source: Option<String>,
    pub output_bytes: u64,
    pub output_token_estimate: u64,
}

pub fn read_cast_metrics(paths: &job::JobPaths) -> Result<CastMetrics, String> {
    let mut metrics = CastMetrics::default();
    if !paths.dir.is_dir() {
        return Ok(metrics);
    }
    for entry in std::fs::read_dir(&paths.dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("cast_") {
            continue;
        }
        let result_path = entry.path().join("result.json");
        if !result_path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&result_path)
            .map_err(|error| format!("anvil report: read {}: {error}", result_path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("anvil report: parse {}: {error}", result_path.display()))?;
        metrics.output_bytes = metrics.output_bytes.saturating_add(
            value
                .get("output_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        metrics.output_token_estimate = metrics.output_token_estimate.saturating_add(
            value
                .get("output_token_estimate")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if let Some(usage) = value
            .get("model_usage")
            .and_then(cache::ProviderUsage::from_json)
        {
            metrics.usage.merge(&usage);
            metrics.measured_usage = true;
            if let Some(source) = value.get("model_usage_source").and_then(Value::as_str) {
                merge_usage_source(&mut metrics.usage_source, source);
            }
        }
    }
    Ok(metrics)
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub(crate) fn merge_usage_source(target: &mut Option<String>, source: &str) {
    match target.as_deref() {
        None => *target = Some(source.to_string()),
        Some(existing) if existing != source => *target = Some("mixed".into()),
        Some(_) => {}
    }
}

fn optional_ratio(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "unknown".into())
}

pub fn write_report(paths: &job::JobPaths, report: &Report) -> Result<(), String> {
    paths.ensure_dir()?;
    write_text(&paths.report_path(), &report.to_json().to_string())
        .map_err(|error| format!("anvil.report.json: {error}"))
}

pub fn read_report(paths: &job::JobPaths) -> Result<Report, String> {
    let text = std::fs::read_to_string(paths.report_path())
        .map_err(|error| format!("anvil.report.json: {error}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("anvil.report.json: {error}"))?;
    Ok(Report {
        cache_hit_ratio: value.get("cache_hit_ratio").and_then(Value::as_f64),
        tokens_uncached: value.get("tokens_uncached").and_then(Value::as_u64),
        tokens_cached: value.get("tokens_cached").and_then(Value::as_u64),
        model_input_tokens: value.get("model_input_tokens").and_then(Value::as_u64),
        model_output_tokens: value.get("model_output_tokens").and_then(Value::as_u64),
        model_usage_source: value
            .get("model_usage_source")
            .and_then(Value::as_str)
            .map(str::to_string),
        output_bytes: value
            .get("output_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_token_estimate: value.get("output_token_estimate").and_then(Value::as_u64),
        critic_calls: value
            .get("critic_calls")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        gate_pass_rate: value
            .get("gate_pass_rate")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        stamp_used: value
            .get("stamp_used")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        winner_id: value
            .get("winner_id")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
        loop_iterations: value
            .get("loop_iterations")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        improvement_delta: value
            .get("improvement_delta")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        refinement_feedback: value
            .get("refinement_feedback")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempReportDir(std::path::PathBuf);

    impl TempReportDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "anvil-report-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempReportDir {
        fn drop(&mut self) {
            for _ in 0..5 {
                match std::fs::remove_dir_all(&self.0) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        }
    }

    #[test]
    fn read_report_round_trips_loop_metrics() {
        let dir = TempReportDir::new();
        let paths = job::JobPaths::from_resolved(dir.0.clone(), dir.0.clone());
        let mut built = empty_report();
        built.loop_iterations = 4;
        built.improvement_delta = 0.2;
        built.gate_pass_rate = 0.75;
        built.winner_id = "cast_2".into();
        write_report(&paths, &built).expect("write");
        let loaded = read_report(&paths).expect("read");
        assert_eq!(loaded.loop_iterations, 4);
        assert!((loaded.improvement_delta - 0.2).abs() < 1e-9);
        assert!((loaded.gate_pass_rate - 0.75).abs() < 1e-9);
        assert_eq!(loaded.winner_id, "cast_2");
        assert_eq!(loaded.cache_hit_ratio, None);
        assert_eq!(loaded.model_usage_source, None);
    }

    #[test]
    fn unknown_provider_metrics_are_serialized_as_null_not_zero() {
        let json = empty_report().to_json();
        assert!(json.get("cache_hit_ratio").is_some_and(Value::is_null));
        assert!(json.get("tokens_uncached").is_some_and(Value::is_null));
        assert!(json.get("tokens_cached").is_some_and(Value::is_null));
    }

    #[test]
    fn cast_metrics_read_provider_usage_and_output_separately() {
        let dir = TempReportDir::new();
        let paths = job::JobPaths::from_resolved(dir.0.clone(), dir.0.clone());
        let cast_dir = paths.dir.join("cast_0");
        std::fs::create_dir_all(&cast_dir).unwrap();
        std::fs::write(
            cast_dir.join("result.json"),
            serde_json::json!({
                "output_bytes": 80,
                "output_token_estimate": 20,
                "model_usage": {
                    "input_tokens": 100,
                    "output_tokens": 10,
                    "cache_read_input_tokens": 60,
                    "cache_creation_input_tokens": 40
                }
            })
            .to_string(),
        )
        .unwrap();
        let metrics = read_cast_metrics(&paths).expect("metrics");
        assert!(metrics.measured_usage);
        assert_eq!(metrics.output_bytes, 80);
        assert_eq!(metrics.usage.input_tokens, Some(100));
        assert_eq!(metrics.usage.output_tokens, Some(10));
    }
}
