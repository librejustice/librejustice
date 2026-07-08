//! Télécharge le tokenizer Qwen3-Embedding-0.6B au build dans `OUT_DIR` (~11 Mo,
//! jamais committé) pour `include_bytes!` dans `lj-ingest` — seul binaire qui
//! re-chunke en BPE exact. Aucune I/O réseau au runtime, aucune lib partagée
//! polluée.

use std::path::PathBuf;
use std::process::Command;

const HF_URL: &str = "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B/resolve/main/tokenizer.json";
/// Taille exacte attendue (Qwen/Qwen3-Embedding-0.6B, main) — garde-fou intégrité.
const EXPECTED_BYTES: u64 = 11_423_705;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR absent"))
        .join("qwen3_embedding_tokenizer.json");

    if dest
        .metadata()
        .map(|m| m.len() == EXPECTED_BYTES)
        .unwrap_or(false)
    {
        return; // déjà téléchargé (cache cargo)
    }

    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&dest)
        .arg(HF_URL)
        .status()
        .expect("curl introuvable : impossible de télécharger le tokenizer Qwen");
    assert!(
        status.success(),
        "téléchargement tokenizer échoué ({HF_URL})"
    );
    assert!(
        dest.metadata()
            .map(|m| m.len() == EXPECTED_BYTES)
            .unwrap_or(false),
        "tokenizer téléchargé : taille inattendue (attendu {EXPECTED_BYTES} octets)"
    );
}
