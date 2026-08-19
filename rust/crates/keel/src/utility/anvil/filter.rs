pub fn compress_output(text: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut first_error: Option<String> = None;
    for line in text.lines() {
        if (line.contains("ERROR") || line.contains("error:") || line.contains("FAILED"))
            && first_error.is_none()
        {
            first_error = Some(line.to_string());
        }
        if seen.insert(line.to_string()) {
            out.push(line);
        }
    }
    let mut result = String::new();
    if let Some(e) = first_error {
        result.push_str(&e);
        result.push('\n');
    }
    if out.len() > 100 {
        result.push_str(&format!(
            "... {} passing lines clipped to count ...\n",
            out.len()
        ));
    } else {
        result.push_str(&out.join("\n"));
    }
    if text.len() > result.len() {
        format!("ANVIL_CLIPPED {}->{}\n{}", text.len(), result.len(), result)
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_first_error() {
        let t = "ok\nerror: boom\nok\n";
        let c = compress_output(t);
        assert!(c.contains("error: boom"));
    }
    #[test]
    fn compresses_repeated_lines() {
        let t = "ok\n".repeat(10000);
        let c = compress_output(&t);
        assert!(c.len() < t.len());
    }
}
