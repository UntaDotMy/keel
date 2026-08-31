use serde_json::Value;

use crate::runtime::write_text;
use crate::utility::anvil::job;

pub struct Report {
    pub cache_hit_ratio: f64,
    pub tokens_uncached: u64,
    pub tokens_cached: u64,
    pub critic_calls: u64,
    pub gate_pass_rate: f64,
    pub stamp_used: bool,
    pub winner_id: String,
    pub loop_iterations: u64,
    pub improvement_delta: f64,
}

pub fn empty_report() -> Report {
    Report {
        cache_hit_ratio: 0.0,
        tokens_uncached: 0,
        tokens_cached: 0,
        critic_calls: 0,
        gate_pass_rate: 0.0,
        stamp_used: false,
        winner_id: "none".into(),
        loop_iterations: 0,
        improvement_delta: 0.0,
    }
}

impl Report {
    pub fn metrics_line(&self) -> String {
        format!("cache_hit_ratio={:.2} tokens_uncached={} tokens_cached={} critic_calls={} gate_pass_rate={:.2} stamp_used={} winner_id={} loop_iterations={} improvement_delta={:.3}", self.cache_hit_ratio, self.tokens_uncached, self.tokens_cached, self.critic_calls, self.gate_pass_rate, self.stamp_used, self.winner_id, self.loop_iterations, self.improvement_delta)
    }
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "cache_hit_ratio": self.cache_hit_ratio,
            "tokens_uncached": self.tokens_uncached,
            "tokens_cached": self.tokens_cached,
            "critic_calls": self.critic_calls,
            "gate_pass_rate": self.gate_pass_rate,
            "stamp_used": self.stamp_used,
            "winner_id": self.winner_id,
            "loop_iterations": self.loop_iterations,
            "improvement_delta": self.improvement_delta
        })
    }
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
        cache_hit_ratio: value
            .get("cache_hit_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        tokens_uncached: value
            .get("tokens_uncached")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        tokens_cached: value
            .get("tokens_cached")
            .and_then(Value::as_u64)
            .unwrap_or(0),
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
    }
}
