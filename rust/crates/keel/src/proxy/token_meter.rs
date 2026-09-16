//! Purpose: Count token savings for raw versus compact command output.
//! Caller: proxy::run, adapters, and gain analytics.
//! Dependencies: tiktoken-rs o200k_base tokenizer.
//! Main Functions: TokenMeter::count_text, TokenMeter::count_bytes, TokenMeter::measure.
//! Side Effects: None.

#[derive(Debug, Clone, Copy)]
pub struct TokenMeasurement {
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub tokens_saved: usize,
    pub savings_pct: f64,
}

pub struct TokenMeter;

impl TokenMeter {
    pub fn count_text(text: &str) -> usize {
        tiktoken_rs::o200k_base_singleton()
            .encode_with_special_tokens(text)
            .len()
    }

    /// Return the longest UTF-8-safe prefix that fits within `max_tokens`.
    ///
    /// Tokenizing once and decoding the accepted token prefix avoids the
    /// repeated full-input binary searches that made a single noisy line
    /// disproportionately expensive. The token sequence is still produced by
    /// the authoritative o200k tokenizer, so the boundary remains exact.
    pub fn prefix_to_token_budget(text: &str, max_tokens: usize) -> String {
        if text.is_empty() || max_tokens == 0 {
            return String::new();
        }
        let tokenizer = tiktoken_rs::o200k_base_singleton();
        let tokens = tokenizer.encode_with_special_tokens(text);
        if tokens.len() <= max_tokens {
            return text.to_string();
        }

        let mut end = max_tokens.min(tokens.len());
        while end > 0 {
            if let Ok(prefix) = tokenizer.decode(&tokens[..end]) {
                return prefix;
            }
            // A token can end in the middle of a UTF-8 sequence. Back off only
            // as far as needed to preserve a valid model-visible string.
            end -= 1;
        }
        String::new()
    }

    pub fn count_bytes(bytes: &[u8]) -> usize {
        let text = String::from_utf8_lossy(bytes);
        Self::count_text(&text)
    }

    pub fn estimate(text: &str) -> usize {
        Self::count_text(text)
    }

    pub fn estimate_bytes(bytes: &[u8]) -> usize {
        Self::count_bytes(bytes)
    }

    pub fn measure(raw_stdout: &[u8], raw_stderr: &[u8], compact: &[u8]) -> TokenMeasurement {
        let tokens_before = Self::estimate_bytes(raw_stdout) + Self::estimate_bytes(raw_stderr);
        let tokens_after = Self::estimate_bytes(compact);
        let tokens_saved = tokens_before.saturating_sub(tokens_after);
        let savings_pct = if tokens_before == 0 {
            0.0
        } else {
            (tokens_saved as f64 / tokens_before as f64) * 100.0
        };
        TokenMeasurement {
            tokens_before,
            tokens_after,
            tokens_saved,
            savings_pct,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TokenMeter;

    #[test]
    fn counts_with_real_o200k_tokenizer() {
        assert_eq!(TokenMeter::count_text("hello world"), 2);
        assert_eq!(TokenMeter::count_text(""), 0);
    }

    #[test]
    fn measures_saved_tokens_with_exact_counts() {
        let measurement = TokenMeter::measure(
            "hello world\nhello world".as_bytes(),
            b"",
            "hello world".as_bytes(),
        );
        assert_eq!(measurement.tokens_before, 5);
        assert_eq!(measurement.tokens_after, 2);
        assert_eq!(measurement.tokens_saved, 3);
    }

    #[test]
    fn prefix_to_token_budget_is_exact_and_utf8_safe() {
        let text = "こんにちは世界 — diagnostic output";
        let prefix = TokenMeter::prefix_to_token_budget(text, 3);
        assert!(text.starts_with(&prefix));
        assert!(TokenMeter::count_text(&prefix) <= 3);
        assert!(prefix.is_char_boundary(prefix.len()));
    }

    #[test]
    fn prefix_to_token_budget_handles_empty_and_unbounded_inputs() {
        assert_eq!(TokenMeter::prefix_to_token_budget("text", 0), "");
        assert_eq!(TokenMeter::prefix_to_token_budget("text", 10), "text");
    }
}
