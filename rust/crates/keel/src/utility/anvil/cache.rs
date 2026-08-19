pub fn cache_headers_for(provider: &str) -> Vec<(String, String)> {
    match provider {
        "anthropic" => vec![("cache_control".into(), "ephemeral".into())],
        _ => vec![],
    }
}
