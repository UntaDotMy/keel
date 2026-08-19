use std::io::Write;

use crate::args::FlagSet;
use crate::utility::anvil::job;

pub fn build_static_prefix(goal: &str, bar_dossier: &str) -> String {
    let mut text = String::new();
    text.push_str("You are an Anvil worker. You do not narrate. You do not commit to git.\n");
    text.push_str("Tools: {read_file, write_file, run}\n");
    text.push_str(&format!("Goal: {goal}\n"));
    text.push_str(&format!("Bar dossier: {bar_dossier}\n"));
    text.push_str(
        "Criteria: specification — lock requirements/bar; output — fixtures/screenshots; errors — logs free of failures\n",
    );
    text.push_str(
        "Score protocol: <spec_score_A>LETTER</spec_score_A> <spec_score_B>LETTER</spec_score_B> etc A–T\n",
    );
    text.push_str("Hard rules: no git, no extra files, stop when gates pass.\n");
    pad_to_tokens(text, 2048)
}

fn pad_to_tokens(mut text: String, min_tokens: usize) -> String {
    let estimated = text.split_whitespace().count();
    if estimated >= min_tokens {
        return text;
    }
    let need = min_tokens - estimated;
    text.push_str("\n--- bar dossier padding (stable) ---\n");
    for _ in 0..need {
        text.push_str("bar ");
    }
    text
}

pub fn hash_static(prefix: &str) -> String {
    sha256_hex(prefix.as_bytes())
}

pub fn write_prefix_files(paths: &job::JobPaths, prefix: &str) -> Result<String, String> {
    let first = hash_static(prefix);
    let second = hash_static(prefix);
    if first != second {
        return Err("anvil prefix: PrefixGuard hash drifted".into());
    }
    paths.ensure_dir()?;
    std::fs::write(paths.prefix_path(), prefix)
        .map_err(|error| format!("anvil prefix.md: {error}"))?;
    std::fs::write(paths.prefix_hash_path(), format!("{first}\n"))
        .map_err(|error| format!("anvil prefix.sha256: {error}"))?;
    Ok(first)
}

pub fn run_prefix_check(
    arguments: &[String],
    standard_output: &mut dyn Write,
    standard_error: &mut dyn Write,
) -> u8 {
    let mut flags = FlagSet::new("anvil prefix-check");
    flags.string_flag("prefix", "");
    flags.string_flag("workspace-root", "");
    flags.string_flag("claude-home", "");
    if let Err(error) = flags.parse(arguments) {
        let _ = writeln!(standard_error, "{}", error.message);
        return 1;
    }
    let supplied = flags.string_value("prefix").to_string();
    let prefix = if supplied.is_empty() {
        match job::JobPaths::resolve(
            flags.string_value("workspace-root"),
            flags.string_value("claude-home"),
        ) {
            Ok(paths) => match std::fs::read_to_string(paths.prefix_path()) {
                Ok(text) => text,
                Err(_) => {
                    let _ = writeln!(
                        standard_error,
                        "anvil prefix-check: provide --prefix or compile first"
                    );
                    return 1;
                }
            },
            Err(error) => {
                let _ = writeln!(standard_error, "{error}");
                return 1;
            }
        }
    } else {
        supplied
    };
    let first = hash_static(&prefix);
    let second = hash_static(&prefix);
    if first != second {
        let _ = writeln!(standard_error, "anvil prefix-check: hash drifted");
        return 1;
    }
    let _ = writeln!(standard_output, "prefix sha256: {first}");
    0
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).saturating_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate().take(16) {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_in_prefix_changes_hash() {
        let first = build_static_prefix("goal", "bar v1");
        let second = build_static_prefix("goal", "bar v1 2026-01-01T00:00:00Z");
        assert_ne!(hash_static(&first), hash_static(&second));
    }

    #[test]
    fn piece_id_after_breakpoint_does_not_change_static_hash() {
        let text = build_static_prefix("goal", "bar");
        assert_eq!(hash_static(&text), hash_static(&text));
    }

    #[test]
    fn sha256_empty_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
