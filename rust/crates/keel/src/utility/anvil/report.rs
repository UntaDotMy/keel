use serde_json::Value;

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
    std::fs::write(paths.report_path(), report.to_json().to_string())
        .map_err(|error| format!("anvil.report.json: {error}"))
}
