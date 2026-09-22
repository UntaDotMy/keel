//! Purpose: a local sentence encoder, so a prompt the head has never seen still
//!   lands near the rows that mean the same thing.
//! Caller: `lexical_experts` (the head) at train and at score time.
//! Dependencies: `ort` (ONNX Runtime, CPU only) plus a WordPiece vocabulary.
//! Main Functions: `encoder_for`, `Encoder::encode`.
//! Side Effects: reads the model directory; writes none.
//!
//! Why an encoder on top of the word-vector table: the table gives every single
//! word a meaning, but it cannot tell that "the deploy rolls every pod at once"
//! and "a rollout replaced all replicas" are the same complaint, because the
//! average of bagged vectors loses order and phrasing. Measured: with the table
//! the host corpus moved from five to six correct of twenty-one. A sentence
//! encoder models the phrase, which is the kase that bag-of-words cannot reach.
//!
//! Model and licence: all-MiniLM-L6-v2 (sentence-transformers), Apache-2.0, the
//! int8 AVX2 ONNX build, with its WordPiece `vocab.txt`. The files are a local
//! cache under `state/models/minilm`; nothing is downloaded at decision time and
//! no graph runs a network call. CPU only: no CUDA feature is enabled anywhere.
//!
//! Determinism: greedy WordPiece, fixed sequence length, no sampling. The same
//! prompt encodes to the same vector on every run and every machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// MiniLM's encoder output width, checked against the model at load.
pub const EMBEDDING_DIM: usize = 384;
/// Feature prefix for the encoder block, matching the word-vector block's rule:
/// uppercase cannot collide with a token, because the tokenizer lowercases.
pub const EMBEDDING_FEATURE_PREFIX: &str = "EMB";
/// Encodings kept per encoder instance before the memo is dropped wholesale.
const ENCODING_CACHE_LIMIT: usize = 40_000;
/// Tokens per prompt, MiniLM's own limit. The corpus is short text by design.
const MAX_TOKENS: usize = 128;
/// A word longer than this is not in any vocabulary and becomes unknown.
const MAX_WORD_CHARS: usize = 100;
const CLS_TOKEN: &str = "[CLS]";
const SEP_TOKEN: &str = "[SEP]";
const UNK_TOKEN: &str = "[UNK]";
const PAD_TOKEN: &str = "[PAD]";

pub const MODEL_FILE_NAME: &str = "model.onnx";
pub const VOCAB_FILE_NAME: &str = "vocab.txt";
/// Directory under the keel state tree that holds the encoder.
pub const MODEL_DIR_NAME: &str = "minilm";

#[derive(Clone)]
pub struct Encoder {
    vocab: HashMap<String, i64>,
    cls_id: i64,
    sep_id: i64,
    unk_id: i64,
    session: Arc<Mutex<ort::session::Session>>,
    /// Encodings already computed. Training vectorizes the same rows several
    /// times (once per pruning fold, once for the held-out score), and a forward
    /// pass costs milliseconds, so the memo is what keeps a run bounded.
    cache: Arc<Mutex<HashMap<String, Vec<f32>>>>,
}

impl Encoder {
    /// Load the encoder from a directory holding the model and the vocabulary.
    pub fn load(dir: &Path) -> Option<Self> {
        let vocab = load_vocab(&dir.join(VOCAB_FILE_NAME))?;
        // why: a runtime or graph that will not load leaves the head without a
        // prior, which is the same state as having no model at all.
        let mut session = ort::session::Session::builder()
            .ok()?
            .commit_from_file(dir.join(MODEL_FILE_NAME))
            .ok()?;
        // why: the head is trained against this width, so a different model
        // would silently score a different feature space.
        let dim = output_width(&mut session)?;
        if dim != EMBEDDING_DIM {
            return None;
        }
        // why: the four special tokens must exist, or this is not the vocabulary
        // the model was exported against.
        vocab.get(PAD_TOKEN)?;
        Some(Self {
            cls_id: *vocab.get(CLS_TOKEN)?,
            sep_id: *vocab.get(SEP_TOKEN)?,
            unk_id: *vocab.get(UNK_TOKEN)?,
            vocab,
            session: Arc::new(Mutex::new(session)),
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Token ids for one prompt, `[CLS]` and `[SEP]` included.
    pub fn token_ids(&self, text: &str) -> Vec<i64> {
        let mut ids = Vec::with_capacity(MAX_TOKENS);
        ids.push(self.cls_id);
        for piece in greedy_pieces(text, &self.vocab) {
            if ids.len() + 1 >= MAX_TOKENS {
                break;
            }
            ids.push(
                self.vocab
                    .get(piece.as_str())
                    .copied()
                    .unwrap_or(self.unk_id),
            );
        }
        ids.push(self.sep_id);
        ids
    }

    /// The prompt's sentence vector, mean-pooled over real tokens and
    /// unit-normalized. `None` when the model cannot answer, never a zero
    /// vector: a zero vector would read as "close to everything".
    pub fn encode(&self, text: &str) -> Option<Vec<f32>> {
        if let Ok(cache) = self.cache.lock() {
            if let Some(embedding) = cache.get(text) {
                return Some(embedding.clone());
            }
        }
        let embedding = self.encode_uncached(text)?;
        if let Ok(mut cache) = self.cache.lock() {
            // why: a bounded memo; a long-running session must not grow without
            // limit, and the oldest encodings are the least likely to repeat.
            if cache.len() >= ENCODING_CACHE_LIMIT {
                cache.clear();
            }
            cache.insert(text.to_string(), embedding.clone());
        }
        Some(embedding)
    }

    fn encode_uncached(&self, text: &str) -> Option<Vec<f32>> {
        let ids = self.token_ids(text);
        let length = ids.len();
        let mask: Vec<i64> = vec![1; length];
        let types: Vec<i64> = vec![0; length];
        let shape = vec![1i64, length as i64];
        let inputs = ort::inputs![
            "input_ids" => ort::value::Tensor::from_array((shape.clone(), ids)).ok()?,
            "attention_mask" => ort::value::Tensor::from_array((shape.clone(), mask)).ok()?,
            "token_type_ids" => ort::value::Tensor::from_array((shape, types)).ok()?,
        ];
        let mut session = self.session.lock().ok()?;
        let outputs = session.run(inputs).ok()?;
        let (shape, data) = outputs
            .get("last_hidden_state")
            .or_else(|| outputs.get("token_embeddings"))?
            .try_extract_tensor::<f32>()
            .ok()?;
        let dims: Vec<usize> = shape.iter().map(|value| *value as usize).collect();
        let values: Vec<f32> = data.to_vec();
        drop(outputs);
        drop(session);
        if dims.len() != 3 || dims[2] != EMBEDDING_DIM {
            return None;
        }
        let (rows, width) = (dims[1], dims[2]);
        let mut pooled = vec![0.0f64; width];
        for row in 0..rows {
            for column in 0..width {
                pooled[column] += f64::from(values[row * width + column]);
            }
        }
        let divisor = rows.max(1) as f64;
        for value in &mut pooled {
            *value /= divisor;
        }
        let norm = pooled.iter().map(|value| value * value).sum::<f64>().sqrt();
        if norm <= 0.0 {
            return None;
        }
        Some(
            pooled
                .into_iter()
                .map(|value| (value / norm) as f32)
                .collect(),
        )
    }
}

fn output_width(session: &mut ort::session::Session) -> Option<usize> {
    for output in session.outputs() {
        if let ort::value::ValueType::Tensor { shape, .. } = output.dtype() {
            let dims: Vec<i64> = shape.iter().copied().collect();
            if let Some(last) = dims.last() {
                if *last > 0 {
                    return Some(*last as usize);
                }
            }
        }
        // why: some exports name the width only in the type, which ort reports
        // as dynamic; fall back to the compiled-in constant in that case.
    }
    Some(EMBEDDING_DIM)
}

fn load_vocab(path: &Path) -> Option<HashMap<String, i64>> {
    let text = std::fs::read_to_string(path).ok()?; // why: absent vocab means no encoder
    let mut vocab = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let token = line.trim_end_matches(['\r', '\n']);
        if !token.is_empty() {
            vocab.insert(token.to_string(), index as i64);
        }
    }
    (!vocab.is_empty()).then_some(vocab)
}

/// BERT basic tokenization followed by greedy WordPiece: lowercase, split on
/// punctuation and spaces, then the longest vocabulary match, with `##` marking
/// a continuation. A word with no match at all becomes the unknown token.
fn greedy_pieces(text: &str, vocab: &HashMap<String, i64>) -> Vec<String> {
    let mut pieces = Vec::new();
    for token in basic_tokens(text) {
        let characters: Vec<char> = token.chars().collect();
        if characters.len() > MAX_WORD_CHARS {
            pieces.push(UNK_TOKEN.to_string());
            continue;
        }
        let mut start = 0usize;
        let mut matched = true;
        while start < characters.len() {
            let mut end = characters.len();
            let mut found: Option<String> = None;
            while start < end {
                let slice: String = characters[start..end].iter().collect();
                let candidate = if start > 0 {
                    format!("##{slice}")
                } else {
                    slice.clone()
                };
                if vocab.contains_key(&candidate) {
                    found = Some(candidate);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(piece) => {
                    pieces.push(piece);
                    start = end;
                }
                None => {
                    matched = false;
                    break;
                }
            }
        }
        if !matched {
            pieces.push(UNK_TOKEN.to_string());
        }
    }
    pieces
}

/// Split on whitespace and punctuation the way BERT's basic tokenizer does.
fn basic_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.to_lowercase().chars() {
        if character.is_alphanumeric() {
            current.push(character);
            continue;
        }
        // Any other character ends the word; punctuation also becomes a token.
        if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        if !character.is_whitespace() {
            tokens.push(character.to_string());
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Encoder for a keel home, cached in process under the model directory's
/// fingerprint: loading ONNX Runtime and the graph costs more than encoding.
pub fn encoder_at(dir: &Path) -> Option<Arc<Encoder>> {
    let fingerprint = model_fingerprint(dir)?;
    if let Ok(cache) = ENCODER_CACHE.lock() {
        if let Some((cached, encoder)) = cache.as_ref() {
            if cached == &fingerprint {
                return Some(Arc::clone(encoder));
            }
        }
    }
    let encoder = Arc::new(Encoder::load(dir)?);
    if let Ok(mut cache) = ENCODER_CACHE.lock() {
        *cache = Some((fingerprint, Arc::clone(&encoder)));
    }
    Some(encoder)
}

/// The encoder a keel home carries, if one was prepared.
pub fn encoder_for(claude_home: &Path) -> Option<Arc<Encoder>> {
    encoder_at(&model_dir(claude_home))
}

/// The encoder an artifact expects, found by its own path: the artifact lives in
/// `state/benchmarks`, the model in `state/models/<name>`.
pub fn encoder_beside(artifact_path: &Path) -> Option<Arc<Encoder>> {
    let state = artifact_path.parent()?.parent()?;
    encoder_at(&state.join("models").join(MODEL_DIR_NAME))
}

pub fn model_dir(claude_home: &Path) -> PathBuf {
    crate::runtime::state_directory(claude_home)
        .join("models")
        .join(MODEL_DIR_NAME)
}

type EncoderCache = Option<(String, Arc<Encoder>)>;
static ENCODER_CACHE: LazyLock<Mutex<EncoderCache>> = LazyLock::new(|| Mutex::new(None));

fn model_fingerprint(dir: &Path) -> Option<String> {
    // fallback: a missing model is the absence of a prior, not an error.
    let model = std::fs::metadata(dir.join(MODEL_FILE_NAME)).ok()?; // fallback: no model
    let vocab = std::fs::metadata(dir.join(VOCAB_FILE_NAME)).ok()?; // fallback: no vocab
    let modified = |metadata: &std::fs::Metadata| {
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    };
    Some(format!(
        "{}:{}:{}:{}:{}",
        dir.display(),
        model.len(),
        modified(&model),
        vocab.len(),
        modified(&vocab)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPLOY: &str = "deploy";
    const PODS: &str = "pods";

    #[test]
    fn basic_tokens_split_on_punctuation_and_case() {
        assert_eq!(
            basic_tokens("Deploy rolls PODS, then health-check!"),
            vec!["deploy", "rolls", "pods", ",", "then", "health", "-", "check", "!"]
        );
    }

    #[test]
    fn word_pieces_are_greedy_and_prefix_continuations() {
        // A tiny in-memory vocabulary stands in for the model file.
        let vocab: HashMap<String, i64> = [
            (CLS_TOKEN, 101i64),
            (SEP_TOKEN, 102),
            (UNK_TOKEN, 100),
            (PAD_TOKEN, 0),
            (DEPLOY, 1),
            ("roll", 2),
            ("##ing", 3),
            (PODS, 4),
        ]
        .into_iter()
        .map(|(token, id)| (token.to_string(), id))
        .collect();
        let pieces = greedy_pieces(&format!("{DEPLOY} rolling {PODS}"), &vocab);
        assert_eq!(pieces, vec![DEPLOY, "roll", "##ing", PODS]);
        let unknown = greedy_pieces("zzz", &vocab);
        assert_eq!(unknown, vec![UNK_TOKEN]);
    }
}
