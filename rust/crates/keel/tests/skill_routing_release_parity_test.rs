//! Release-rail parity: mirrors `.github/release-smoke.mjs` skill-activation-route.
//! The release smoke routes one prompt against the REAL installed skill bundle
//! and requires a match. PR validate only exercises tiny fixture corpora, so
//! a routing regression that manifests solely on the full corpus (like the
//! J03 confidence gate silencing a 0.569-margin winner) passes PR CI and kills
//! every release Smoke job. This test closes that hole.
use std::path::{Path, PathBuf};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

#[test]
fn smoke_prompt_routes_against_real_skill_bundle() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let tmp = std::env::temp_dir().join(format!("keel-release-parity-{}", std::process::id()));
    let skills = tmp.join("skills");
    let _ = std::fs::remove_dir_all(&tmp);
    for entry in std::fs::read_dir(&repo).unwrap().flatten() {
        let skill_md = entry.path().join("SKILL.md");
        if entry.path().is_dir() && skill_md.is_file() {
            copy_dir(&entry.path(), &skills.join(entry.file_name()));
        }
    }
    let prompt = "preserve existing flow before editing brownfield source";
    let found = keel::utility::skill_match::match_skill_for_prompt_with_details(&tmp, prompt);
    let found = found.expect("release smoke prompt must match on the real bundle");
    assert_eq!(found.name, "preserve-existing-flow");
    assert!(
        skills.join(&found.name).join("SKILL.md").is_file(),
        "matched skill must be present on disk"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
