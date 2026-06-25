use std::io::Read;
use std::path::PathBuf;

const MODEL_ID: &str = "TaylorAI/bge-micro-v2";
const FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_FEATURE_SEMANTIC").is_err() {
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    for file in FILES {
        let dst = out_dir.join(file);
        if dst.exists() {
            continue;
        }
        let url = format!("https://huggingface.co/{MODEL_ID}/resolve/main/{file}");
        eprintln!("keel: downloading {url}");
        let response = ureq::get(&url)
            .call()
            .unwrap_or_else(|error| panic!("download {url}: {error}"));
        let mut bytes = Vec::with_capacity(34_000_000);
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .unwrap_or_else(|error| panic!("read {url}: {error}"));
        std::fs::write(&dst, &bytes)
            .unwrap_or_else(|error| panic!("write {}: {error}", dst.display()));
    }
}
