//! Purpose: Latent Semantic Analysis (LSA) over the recall corpus — genuine
//!   cross-vocabulary semantic recall with NO pretrained model, NO network, and
//!   NO new crates. Builds a TF-IDF term×document matrix, takes a truncated SVD
//!   (via the eigendecomposition of the document Gram matrix), and represents
//!   each document as a low-rank latent vector so documents that co-occur with
//!   similar neighbors land near each other even when they share no words.
//! Caller: `recall::cascade_recall_query` augments thin lexical results with the
//!   latent neighbors of the documents it already found.
//! Dependencies: std only. The eigensolver is a hand-rolled cyclic Jacobi
//!   routine for symmetric matrices (deterministic, always converges).
//! Side Effects: none — pure in-memory computation over documents passed in.
//!
//! Why this stays in the moat: vector search (e.g. HNSW over neural embeddings)
//! buys cross-vocabulary matching by learning meaning from a huge external
//! corpus and bundling a model. LSA buys the same cross-vocabulary effect from
//! YOUR corpus's own co-occurrence structure, computed deterministically at
//! query time. It is weaker than a neural model on rare/never-co-occurring
//! vocabulary (it can only learn synonymy your documents actually exhibit), but
//! it is reproducible, dependency-free, and runs entirely in-process — ahead of
//! lexical recall on meaning, and ahead of neural recall on the moat dimensions.

/// Minimum documents for LSA to be meaningful. Below this the co-occurrence
/// structure is too sparse for a truncated SVD to reveal latent synonymy, so the
/// caller keeps the lexical results unaugmented.
pub const LSA_MIN_DOCS: usize = 3;

/// Above this many documents the on-demand Jacobi decomposition (O(D^3) per
/// sweep) gets too slow to run inside a query, and a corpus that large usually
/// has enough lexical coverage that augmentation matters less. The caller skips
/// LSA above the cap rather than blocking the query.
pub const LSA_MAX_DOCS: usize = 400;

/// Upper bound on retained latent dimensions (the SVD truncation rank `k`). LSA
/// quality comes from keeping the top handful of dimensions and discarding the
/// long tail of noise; 64 is ample for a single-user memory corpus and is
/// further clamped to `docs - 1` so it is always a real truncation.
const LSA_MAX_DIMENSIONS: usize = 64;

/// Latent cosine below which a neighbor is too weak to surface. The augmentation
/// only runs when lexical recall was thin, so this is tuned to admit genuine
/// topical neighbors while keeping unrelated documents out of an otherwise
/// precise result set.
pub const LSA_MIN_SIMILARITY: f64 = 0.30;

/// Fraction of total spectral energy the retained latent dimensions must cover.
/// LSA's cross-vocabulary bridging only emerges under aggressive dimensionality
/// reduction, so this is deliberately well below 1.0: keeping the dominant topic
/// dimensions (and discarding the long tail) is what folds a transitive neighbor
/// onto its seed. 0.5 concentrates on the principal topics while staying robust
/// to corpus size via the variance-fraction rule in [`latent_coordinates`].
const LSA_VARIANCE_FRACTION: f64 = 0.5;

/// A document's low-rank latent representation plus the metadata needed to turn
/// a neighbor into a `RecallHit`. `coords` are the truncated LSA coordinates
/// (`sqrt(eigenvalue_r) * eigenvector[doc][r]`); cosine between two `coords`
/// vectors is the latent semantic similarity.
pub struct SemanticIndex {
    paths: Vec<String>,
    contents: Vec<String>,
    coords: Vec<Vec<f64>>,
}

impl SemanticIndex {
    pub fn path(&self, index: usize) -> &str {
        &self.paths[index]
    }

    pub fn content(&self, index: usize) -> &str {
        &self.contents[index]
    }
}

/// Build an LSA index over `(path, content)` documents, or `None` when the
/// corpus is too small/large or degenerate (empty vocabulary, no positive
/// latent dimensions). Returning `None` is the "skip augmentation" signal — the
/// caller keeps its lexical results untouched.
pub fn build_semantic_index(documents: &[(String, String)]) -> Option<SemanticIndex> {
    let document_count = documents.len();
    if !(LSA_MIN_DOCS..=LSA_MAX_DOCS).contains(&document_count) {
        return None;
    }

    // 1. Vocabulary: every term of length >= 2, lowercased. TF-IDF downweights
    //    ubiquitous stopwords via idf, so no separate stopword list is needed.
    let tokenized: Vec<Vec<String>> = documents
        .iter()
        .map(|(_, content)| tokenize(content))
        .collect();
    let vocabulary = build_vocabulary(&tokenized);
    if vocabulary.is_empty() {
        return None;
    }

    // 2. TF-IDF document columns, L2-normalized so the Gram matrix is the
    //    document-document cosine matrix (entries in [0, 1]).
    let term_count = vocabulary.len();
    let document_frequency = document_frequencies(&tokenized, &vocabulary, term_count);
    let mut columns: Vec<Vec<f64>> = Vec::with_capacity(document_count);
    for tokens in &tokenized {
        columns.push(tfidf_column(
            tokens,
            &vocabulary,
            &document_frequency,
            document_count,
            term_count,
        ));
    }

    // 3. Document Gram matrix G = A^T A (docs x docs), symmetric PSD. With
    //    L2-normalized columns, G[i][j] is the cosine of documents i and j.
    let gram = gram_matrix(&columns);

    // 4. Eigendecompose G. Its eigenvectors are the right singular vectors of A
    //    and its eigenvalues are the squared singular values.
    let (eigenvalues, eigenvectors) = jacobi_eigendecomposition(&gram);

    // 5. Truncate to the top-k positive dimensions and form latent coordinates
    //    c_d[r] = sqrt(lambda_r) * V[d][r].
    let coords = latent_coordinates(&eigenvalues, &eigenvectors, document_count);
    if coords.iter().all(|row| row.is_empty()) {
        return None;
    }

    Some(SemanticIndex {
        paths: documents.iter().map(|(path, _)| path.clone()).collect(),
        contents: documents
            .iter()
            .map(|(_, content)| content.clone())
            .collect(),
        coords,
    })
}

impl SemanticIndex {
    /// Rank documents by their best latent cosine to any of the `seed` documents,
    /// excluding the seeds themselves and anything in `exclude`. Returns up to
    /// `limit` `(document_index, similarity)` pairs above [`LSA_MIN_SIMILARITY`],
    /// best first. This is the augmentation primitive: `seed` are the documents
    /// lexical recall already found, and the result is their semantic neighbors.
    pub fn neighbors_of(
        &self,
        seed: &[usize],
        exclude: &[usize],
        limit: usize,
    ) -> Vec<(usize, f64)> {
        if seed.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(usize, f64)> = Vec::new();
        for candidate in 0..self.coords.len() {
            if seed.contains(&candidate) || exclude.contains(&candidate) {
                continue;
            }
            // Best similarity to ANY seed: a candidate near even one of the
            // lexical hits is a worthwhile expansion.
            let mut best = 0.0f64;
            for &s in seed {
                let similarity = cosine(&self.coords[candidate], &self.coords[s]);
                if similarity > best {
                    best = similarity;
                }
            }
            if best >= LSA_MIN_SIMILARITY {
                scored.push((candidate, best));
            }
        }
        // Descending similarity; index breaks ties so the ordering is stable.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        scored
    }

    /// Index of the document at `path`, if present. Used to map lexical hits
    /// (which carry paths) onto the seed indices `neighbors_of` expects.
    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.paths.iter().position(|p| p == path)
    }
}

/// Lowercase alphanumeric tokens of length >= 2. Mirrors the recall tokenizer's
/// spirit (alphanumerics plus intra-word `-`/`_`) so LSA and the lexical stages
/// agree on what a "word" is.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_'))
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Sorted, de-duplicated vocabulary mapped to dense indices. Sorting keeps the
/// term→index assignment deterministic so two runs over the same corpus produce
/// identical matrices (and therefore identical eigenvectors up to sign).
fn build_vocabulary(tokenized: &[Vec<String>]) -> std::collections::HashMap<String, usize> {
    let mut terms: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for tokens in tokenized {
        for token in tokens {
            terms.insert(token.clone());
        }
    }
    terms
        .into_iter()
        .enumerate()
        .map(|(index, term)| (term, index))
        .collect()
}

/// Document frequency per term index: how many documents contain the term.
fn document_frequencies(
    tokenized: &[Vec<String>],
    vocabulary: &std::collections::HashMap<String, usize>,
    term_count: usize,
) -> Vec<usize> {
    let mut document_frequency = vec![0usize; term_count];
    for tokens in tokenized {
        let mut seen = vec![false; term_count];
        for token in tokens {
            if let Some(&term_index) = vocabulary.get(token) {
                if !seen[term_index] {
                    seen[term_index] = true;
                    document_frequency[term_index] += 1;
                }
            }
        }
    }
    document_frequency
}

/// One L2-normalized TF-IDF column for a document. `tf` is the raw term count;
/// `idf` is the smoothed `ln((D+1)/(df+1)) + 1`, which keeps every term's weight
/// positive while still downweighting ubiquitous terms. L2 normalization makes
/// the later Gram entries bounded cosines.
fn tfidf_column(
    tokens: &[String],
    vocabulary: &std::collections::HashMap<String, usize>,
    document_frequency: &[usize],
    document_count: usize,
    term_count: usize,
) -> Vec<f64> {
    let mut counts = vec![0.0f64; term_count];
    for token in tokens {
        if let Some(&term_index) = vocabulary.get(token) {
            counts[term_index] += 1.0;
        }
    }
    let mut column = vec![0.0f64; term_count];
    for (term_index, &count) in counts.iter().enumerate() {
        if count == 0.0 {
            continue;
        }
        let idf = ((document_count as f64 + 1.0) / (document_frequency[term_index] as f64 + 1.0))
            .ln()
            + 1.0;
        column[term_index] = count * idf;
    }
    let norm = column.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut column {
            *value /= norm;
        }
    }
    column
}

/// Document Gram matrix G[i][j] = column_i . column_j. Symmetric, so only the
/// upper triangle is computed and mirrored.
fn gram_matrix(columns: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let document_count = columns.len();
    let mut gram = vec![vec![0.0f64; document_count]; document_count];
    for i in 0..document_count {
        for j in i..document_count {
            let dot: f64 = columns[i].iter().zip(&columns[j]).map(|(a, b)| a * b).sum();
            gram[i][j] = dot;
            gram[j][i] = dot;
        }
    }
    gram
}

/// Cyclic Jacobi eigendecomposition of a symmetric matrix. Returns
/// `(eigenvalues, eigenvectors)` where `eigenvectors[i][r]` is the i-th
/// component of the r-th eigenvector (columns are eigenvectors). The algorithm
/// repeatedly applies Givens rotations that zero the largest off-diagonal
/// element; for symmetric input it always converges, and it is deterministic, so
/// the recall results are reproducible.
// `needless_range_loop` is allowed here on purpose: a Jacobi sweep reads and
// writes several cells of the SAME matrix by index symmetrically (a[i][p],
// a[p][i], a[i][q], a[q][i]), so the loop index IS the algorithm. Rewriting it as
// an iterator would fight the borrow checker (aliasing distinct rows) and bury
// the numerical intent — the index form mirrors the standard reference algorithm.
#[allow(clippy::needless_range_loop)]
fn jacobi_eigendecomposition(matrix: &[Vec<f64>]) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = matrix.len();
    let mut a = matrix.to_vec();
    // Eigenvector accumulator starts as the identity.
    let mut v = vec![vec![0.0f64; n]; n];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    if n <= 1 {
        let eigenvalues = (0..n).map(|i| a[i][i]).collect();
        return (eigenvalues, v);
    }

    const MAX_SWEEPS: usize = 100;
    const CONVERGENCE_EPSILON: f64 = 1e-12;

    for _sweep in 0..MAX_SWEEPS {
        // Sum of squared off-diagonal magnitude; converged when negligible.
        let mut off_diagonal = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                off_diagonal += a[p][q] * a[p][q];
            }
        }
        if off_diagonal < CONVERGENCE_EPSILON {
            break;
        }

        for p in 0..n {
            for q in (p + 1)..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                // Rotation angle that zeros a[p][q]: pick the smaller-magnitude
                // tangent root for numerical stability (Golub & Van Loan).
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = if theta == 0.0 {
                    1.0
                } else {
                    theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt())
                };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;

                let app = a[p][p];
                let aqq = a[q][q];
                let apq = a[p][q];
                // New diagonal entries; the off-diagonal a[p][q]/a[q][p] becomes 0.
                a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
                a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
                a[p][q] = 0.0;
                a[q][p] = 0.0;
                // Mix the other entries of rows/columns p and q (symmetry kept).
                for i in 0..n {
                    if i != p && i != q {
                        let aip = a[i][p];
                        let aiq = a[i][q];
                        a[i][p] = c * aip - s * aiq;
                        a[p][i] = a[i][p];
                        a[i][q] = s * aip + c * aiq;
                        a[q][i] = a[i][q];
                    }
                }
                // Accumulate the rotation into the eigenvector matrix.
                for row in v.iter_mut() {
                    let vip = row[p];
                    let viq = row[q];
                    row[p] = c * vip - s * viq;
                    row[q] = s * vip + c * viq;
                }
            }
        }
    }

    let eigenvalues = (0..n).map(|i| a[i][i]).collect();
    (eigenvalues, v)
}

/// Truncated latent coordinates: keep the leading eigenpairs (largest
/// eigenvalue first) and set `coords[d][r] = sqrt(lambda_r) * eigenvector[d][r]`.
///
/// Truncation rank `k` is chosen by a VARIANCE-FRACTION rule: keep the fewest
/// leading dimensions whose eigenvalues cover [`LSA_VARIANCE_FRACTION`] of the
/// total spectral energy (then clamp to `LSA_MAX_DIMENSIONS`). This is the
/// standard LSA practice and it matters for correctness, not just size: LSA's
/// cross-vocabulary bridging ("authentication" ↔ "login" via a co-occurring
/// document) only emerges under AGGRESSIVE reduction. Keeping `docs - 1`
/// dimensions is nearly lossless and leaves unrelated documents orthogonal;
/// collapsing to the few dominant topic dimensions is what folds a transitive
/// neighbor onto its seed. We always keep at least 1 and at most `docs - 1`
/// dimensions so the result is a real truncation rather than a lossless
/// rotation.
fn latent_coordinates(
    eigenvalues: &[f64],
    eigenvectors: &[Vec<f64>],
    document_count: usize,
) -> Vec<Vec<f64>> {
    // Rank eigenpairs by descending eigenvalue, dropping non-positive ones
    // (numerical noise / null space carry no semantic signal).
    let mut order: Vec<usize> = (0..eigenvalues.len())
        .filter(|&r| eigenvalues[r] > 1e-9)
        .collect();
    order.sort_by(|&a, &b| {
        eigenvalues[b]
            .partial_cmp(&eigenvalues[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if order.is_empty() {
        return vec![Vec::new(); document_count];
    }

    // Variance-fraction rank: smallest prefix whose eigenvalues cover the target
    // share of total energy. This adapts to corpus size (a tight, on-topic
    // corpus concentrates energy in fewer dimensions than a diverse one).
    let total_energy: f64 = order.iter().map(|&r| eigenvalues[r]).sum();
    let target = total_energy * LSA_VARIANCE_FRACTION;
    let mut cumulative = 0.0;
    let mut dimensions = 0usize;
    for &r in &order {
        cumulative += eigenvalues[r];
        dimensions += 1;
        if cumulative >= target {
            break;
        }
    }
    // Always a real truncation: at least 1 dimension, at most docs - 1, and no
    // more than the absolute cap.
    let max_dimensions = LSA_MAX_DIMENSIONS
        .min(document_count.saturating_sub(1))
        .max(1);
    dimensions = dimensions.clamp(1, max_dimensions).min(order.len());
    let kept = &order[..dimensions];

    (0..document_count)
        .map(|d| {
            kept.iter()
                .map(|&r| eigenvalues[r].sqrt() * eigenvectors[d][r])
                .collect()
        })
        .collect()
}

/// Cosine similarity of two latent coordinate vectors. Zero when either is the
/// zero vector (a document with no retained latent energy).
fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eigenvalues are sorted so the assertion is order-independent (Jacobi does
    /// not guarantee output order).
    fn sorted(mut values: Vec<f64>) -> Vec<f64> {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values
    }

    #[test]
    fn jacobi_diagonal_matrix_returns_its_diagonal() {
        let matrix = vec![vec![3.0, 0.0], vec![0.0, 5.0]];
        let (eigenvalues, _) = jacobi_eigendecomposition(&matrix);
        let s = sorted(eigenvalues);
        assert!((s[0] - 3.0).abs() < 1e-9, "got {s:?}");
        assert!((s[1] - 5.0).abs() < 1e-9, "got {s:?}");
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn jacobi_known_symmetric_2x2() {
        // [[2,1],[1,2]] has eigenvalues 1 and 3.
        let matrix = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        let (eigenvalues, vectors) = jacobi_eigendecomposition(&matrix);
        let s = sorted(eigenvalues.clone());
        assert!((s[0] - 1.0).abs() < 1e-9, "eigenvalues {s:?}");
        assert!((s[1] - 3.0).abs() < 1e-9, "eigenvalues {s:?}");
        // Eigenvectors must be unit length.
        for r in 0..2 {
            let norm = (0..2).map(|i| vectors[i][r] * vectors[i][r]).sum::<f64>();
            assert!(
                (norm - 1.0).abs() < 1e-9,
                "eigenvector {r} not unit: {norm}"
            );
        }
    }

    #[test]
    fn jacobi_reconstructs_the_matrix() {
        // V diag(lambda) V^T must reconstruct the original symmetric matrix.
        let matrix = vec![
            vec![4.0, 1.0, 2.0],
            vec![1.0, 5.0, 3.0],
            vec![2.0, 3.0, 6.0],
        ];
        let (eigenvalues, v) = jacobi_eigendecomposition(&matrix);
        let n = 3;
        for i in 0..n {
            for j in 0..n {
                let reconstructed: f64 = (0..n).map(|r| v[i][r] * eigenvalues[r] * v[j][r]).sum();
                assert!(
                    (reconstructed - matrix[i][j]).abs() < 1e-7,
                    "reconstruction[{i}][{j}] = {reconstructed}, want {}",
                    matrix[i][j]
                );
            }
        }
    }

    #[test]
    fn too_few_documents_returns_none() {
        let docs = vec![
            ("a.md".to_string(), "alpha beta".to_string()),
            ("b.md".to_string(), "gamma delta".to_string()),
        ];
        assert!(build_semantic_index(&docs).is_none());
    }

    #[test]
    fn lsa_surfaces_a_cross_vocabulary_neighbor() {
        // The crux of the "ahead" claim: a document that shares NO word with the
        // seed is still surfaced because bridge documents tie their vocabularies
        // together. The corpus is two cohesive clusters (not one orthogonal
        // outlier, which would create a degenerate eigenvalue collision):
        //   - an AUTH cluster where "authentication" vocab and "login/session/
        //     token" vocab are linked through bridge documents that use both, and
        //   - an unrelated INFRA cluster with its own internal cohesion.
        // seed.md uses only the authentication vocabulary; target.md uses only the
        // login/session/token vocabulary and shares NO term with seed.md, yet LSA
        // must surface it via the bridges and rank it above the infra docs.
        let docs = vec![
            (
                "seed.md".to_string(),
                "authentication authentication policy credential authentication review credential"
                    .to_string(),
            ),
            (
                "bridge-a.md".to_string(),
                "authentication credential begins the login session flow login session".to_string(),
            ),
            (
                "bridge-b.md".to_string(),
                "login session token refresh authentication credential oauth token".to_string(),
            ),
            (
                "target.md".to_string(),
                "login session token oauth refresh login session token rotation".to_string(),
            ),
            (
                "infra-a.md".to_string(),
                "kubernetes pod node scheduling autoscaling cluster kubernetes node".to_string(),
            ),
            (
                "infra-b.md".to_string(),
                "kubernetes cluster node metrics scheduling autoscaling pod metrics".to_string(),
            ),
        ];
        let index = build_semantic_index(&docs).expect("index builds");
        let seed = index.index_of("seed.md").expect("seed present");
        let neighbors = index.neighbors_of(&[seed], &[], 10);
        let neighbor_paths: Vec<&str> = neighbors.iter().map(|&(i, _)| index.path(i)).collect();
        // target.md shares no word with seed.md but must surface via the bridges.
        assert!(
            neighbor_paths.contains(&"target.md"),
            "cross-vocabulary neighbor not surfaced; neighbors: {neighbor_paths:?}"
        );
        // The on-topic target must outrank any infra-cluster document that leaks in.
        let target_score = neighbors
            .iter()
            .find(|&&(i, _)| index.path(i) == "target.md")
            .map(|&(_, s)| s)
            .unwrap();
        let best_infra_score = neighbors
            .iter()
            .filter(|&&(i, _)| index.path(i).starts_with("infra-"))
            .map(|&(_, s)| s)
            .fold(0.0f64, f64::max);
        assert!(
            target_score > best_infra_score,
            "topical neighbor ({target_score}) must outrank the best infra doc ({best_infra_score})"
        );
    }

    #[test]
    fn neighbors_exclude_seed_and_excluded_indices() {
        let docs = vec![
            ("a.md".to_string(), "alpha alpha beta gamma".to_string()),
            ("b.md".to_string(), "alpha beta beta gamma".to_string()),
            ("c.md".to_string(), "alpha beta gamma gamma".to_string()),
            ("d.md".to_string(), "delta epsilon zeta eta".to_string()),
        ];
        let index = build_semantic_index(&docs).expect("index builds");
        let seed = index.index_of("a.md").unwrap();
        let excluded = index.index_of("b.md").unwrap();
        let neighbors = index.neighbors_of(&[seed], &[excluded], 10);
        for &(i, _) in &neighbors {
            assert_ne!(index.path(i), "a.md", "seed must be excluded");
            assert_ne!(index.path(i), "b.md", "excluded index must be excluded");
        }
    }

    #[test]
    fn cosine_handles_zero_vectors() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-12);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-12);
    }
}
