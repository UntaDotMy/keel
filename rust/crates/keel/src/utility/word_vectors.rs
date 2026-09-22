//! Purpose: a compact static word-vector table, so a prompt the head has never
//!   seen still carries meaning instead of going silent on unknown vocabulary.
//! Caller: `lexical_experts` (the head) at train and at score time.
//! Dependencies: std only. The table is int8-quantized and read as bytes.
//! Main Functions: `load`, `average`, `words_in`.
//! Side Effects: reads one binary file; writes none.
//!
//! Why a table and not a trained model: keel's head is a linear model over
//! surface strings, so `deadlock` and `lock contention` share nothing. Measured
//! cost of that gap: twelve of twenty-one host prompts came back silent. The
//! prior is what a bag of words cannot learn from eleven thousand short rows,
//! and a fifty-dimension int8 table is 2.9 MB and costs one hash lookup a word.
//!
//! This is not `semantic_fast`: that engine is character n-grams, which measure
//! how two strings look alike, not what two different words mean. Both are
//! deterministic and offline, and they answer different questions.
//!
//! Source and attribution: GloVe 6B 50d vectors (Pennington, Socher, Manning),
//! kept only for the fifty thousand most frequent words plus the head's own
//! vocabulary. GloVe is released under the Public Domain Dedication and License
//! v1.0. The table is a local cache under `state/benchmarks`, not shipped here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const TABLE_MAGIC: &[u8; 8] = b"KEELWV01";
/// Feature prefix for the embedding block. Uppercase cannot collide with a
/// token: the tokenizer lowercases and only emits alphanumerics and underscores.
pub const VECTOR_FEATURE_PREFIX: &str = "VEC";

#[derive(Clone)]
pub struct WordVectors {
    dim: usize,
    scale: f64,
    rows: HashMap<String, Box<[i8]>>,
}

impl WordVectors {
    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn contains(&self, word: &str) -> bool {
        self.rows.contains_key(word)
    }

    /// The mean of the words' vectors, dequantized and unit-normalized so the
    /// embedding block weighs the same as the term-frequency block it joins.
    /// `None` when no word in the prompt is in the table.
    pub fn average<'a>(&self, words: impl Iterator<Item = &'a str>) -> Option<Vec<f64>> {
        let mut sum = vec![0.0f64; self.dim];
        let mut counted = 0usize;
        for word in words {
            let Some(row) = self.rows.get(word) else {
                continue;
            };
            counted += 1;
            for (index, value) in row.iter().enumerate() {
                sum[index] += f64::from(*value) / 127.0 * self.scale;
            }
        }
        if counted == 0 {
            return None;
        }
        for value in &mut sum {
            *value /= counted as f64;
        }
        let norm = sum.iter().map(|value| value * value).sum::<f64>().sqrt();
        if norm > 0.0 {
            for value in &mut sum {
                *value /= norm;
            }
        }
        Some(sum)
    }

    /// Read a table written by the preparation step. A malformed table is no
    /// table: the caller then scores without the embedding block.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?; // why: an absent table is no prior
        if bytes.len() < 20 || &bytes[0..8] != TABLE_MAGIC {
            return None;
        }
        let dim = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
        let scale = f64::from(f32::from_le_bytes(bytes[12..16].try_into().ok()?));
        let count = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
        if dim == 0 || dim > 4096 || count == 0 {
            return None;
        }
        let mut rows: HashMap<String, Box<[i8]>> = HashMap::with_capacity(count);
        let mut cursor = 20usize;
        for _ in 0..count {
            let length =
                u16::from_le_bytes(bytes.get(cursor..cursor + 2)?.try_into().ok()?) as usize;
            cursor += 2;
            let word = std::str::from_utf8(bytes.get(cursor..cursor + length)?)
                .ok()?
                .to_string();
            cursor += length;
            let slice = bytes.get(cursor..cursor + dim)?;
            let row: Vec<i8> = slice.iter().map(|value| *value as i8).collect();
            cursor += dim;
            rows.insert(word, row.into_boxed_slice());
        }
        Some(Self { dim, scale, rows })
    }
}

/// Where a prepared table lives for a keel home: beside the artifact it scores.
pub fn table_path(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("benchmarks")
        .join(TABLE_FILE_NAME)
}

/// The table an artifact expects, found by its own path: the two files are
/// written together by a training run, so the model never needs a home.
pub fn table_beside(artifact_path: &Path) -> Option<std::sync::Arc<WordVectors>> {
    let path = artifact_path.with_file_name(TABLE_FILE_NAME);
    table_at(&path)
}

/// The table for this home, or `None` when none was prepared. Cached in process
/// under the file's fingerprint: the head reads it on every cold start.
pub fn table_for(claude_home: &Path) -> Option<std::sync::Arc<WordVectors>> {
    table_at(&table_path(claude_home))
}

/// Load one table file, cached by its fingerprint.
pub fn table_at(path: &Path) -> Option<std::sync::Arc<WordVectors>> {
    let fingerprint = table_fingerprint(path)?;
    if let Ok(cache) = TABLE_CACHE.lock() {
        if let Some((cached, table)) = cache.as_ref() {
            if cached == &fingerprint {
                return Some(std::sync::Arc::clone(table));
            }
        }
    }
    let table = std::sync::Arc::new(WordVectors::load(path)?);
    if let Ok(mut cache) = TABLE_CACHE.lock() {
        *cache = Some((fingerprint, std::sync::Arc::clone(&table)));
    }
    Some(table)
}

pub const TABLE_FILE_NAME: &str = "word-vectors.bin";

type TableCache = Option<(String, std::sync::Arc<WordVectors>)>;
static TABLE_CACHE: std::sync::LazyLock<std::sync::Mutex<TableCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn table_fingerprint(path: &Path) -> Option<String> {
    // why: an unreadable path is the absence of a prior, not an error to report.
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    Some(format!(
        "{}:{}:{}",
        path.display(),
        metadata.len(),
        modified
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD_A: &str = "alpha";
    const WORD_B: &str = "beta";
    const UNKNOWN: &str = "gamma";

    fn write_table(path: &Path, dim: usize, scale: f32, rows: &[(&str, Vec<i8>)]) {
        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(TABLE_MAGIC);
        payload.extend_from_slice(&(dim as u32).to_le_bytes());
        payload.extend_from_slice(&scale.to_le_bytes());
        payload.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        for (word, values) in rows {
            payload.extend_from_slice(&(word.len() as u16).to_le_bytes());
            payload.extend_from_slice(word.as_bytes());
            for value in values {
                payload.push(*value as u8);
            }
        }
        std::fs::write(path, payload).unwrap();
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_table(label: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("keel-vectors-{label}-{}", std::process::id()));
        cleanup(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(TABLE_FILE_NAME);
        (dir, path)
    }

    #[test]
    fn a_table_round_trips_and_averages_known_words() {
        let (dir, path) = temp_table("round");
        write_table(
            &path,
            3,
            2.0,
            &[(WORD_A, vec![127, 0, -127]), (WORD_B, vec![127, 0, -127])],
        );
        let table = WordVectors::load(&path).expect("table loads");
        assert_eq!(table.dim(), 3);
        assert_eq!(table.rows(), 2);
        assert!(table.contains(WORD_A));
        assert!(!table.contains(UNKNOWN));
        let averaged = table
            .average([WORD_A, WORD_B].into_iter())
            .expect("average");
        let expected = std::f64::consts::FRAC_1_SQRT_2;
        assert!((averaged[0] - expected).abs() < 1e-3, "{averaged:?}");
        assert!((averaged[2] + expected).abs() < 1e-3, "{averaged:?}");
        assert!(
            table.average([UNKNOWN].into_iter()).is_none(),
            "no known word means no embedding, never a zero vector"
        );
        cleanup(&dir);
    }

    #[test]
    fn a_truncated_or_foreign_table_reads_as_no_prior() {
        let (dir, path) = temp_table("broken");
        std::fs::write(&path, b"not a table").unwrap();
        assert!(WordVectors::load(&path).is_none());
        std::fs::write(&path, b"KEELWV01\x03\x00\x00\x00").unwrap();
        assert!(
            WordVectors::load(&path).is_none(),
            "a header without rows is not a table"
        );
        write_table(&path, 3, 1.0, &[(WORD_A, vec![1, 2, 3])]);
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 1]).unwrap();
        assert!(
            WordVectors::load(&path).is_none(),
            "a row cut short is not a table"
        );
        assert!(WordVectors::load(&dir.join("absent.bin")).is_none());
        cleanup(&dir);
    }
}
