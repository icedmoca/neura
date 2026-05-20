use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokenizers::Tokenizer;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TokenizationMethod {
    HuggingFaceTokenizer,
    DeterministicEstimate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenizationRecord {
    pub method: TokenizationMethod,
    pub token_count: u64,
    pub char_count: usize,
    pub tokenizer_path: Option<PathBuf>,
    pub token_ids_preview: Vec<u32>,
    pub token_text_preview: Vec<String>,
}

static TOKENIZER: OnceLock<Option<(PathBuf, Tokenizer)>> = OnceLock::new();

pub fn tokenize_text(text: &str) -> TokenizationRecord {
    if let Some((path, tokenizer)) = tokenizer() {
        if let Ok(encoding) = tokenizer.encode(text, false) {
            let ids = encoding
                .get_ids()
                .iter()
                .copied()
                .take(64)
                .collect::<Vec<_>>();
            let token_text_preview = ids
                .iter()
                .take(24)
                .filter_map(|id| tokenizer.decode(&[*id], false).ok())
                .collect::<Vec<_>>();
            return TokenizationRecord {
                method: TokenizationMethod::HuggingFaceTokenizer,
                token_count: encoding.len() as u64,
                char_count: text.len(),
                tokenizer_path: Some(path.clone()),
                token_ids_preview: ids,
                token_text_preview,
            };
        }
    }
    TokenizationRecord {
        method: TokenizationMethod::DeterministicEstimate,
        token_count: estimate_tokens(text.len()),
        char_count: text.len(),
        tokenizer_path: None,
        token_ids_preview: Vec::new(),
        token_text_preview: text
            .split_whitespace()
            .take(24)
            .map(|s| s.to_string())
            .collect(),
    }
}

pub fn tokenize_joined<'a>(parts: impl IntoIterator<Item = &'a str>) -> TokenizationRecord {
    let joined = parts.into_iter().collect::<Vec<_>>().join("\n");
    tokenize_text(&joined)
}

pub fn estimate_tokens(chars: usize) -> u64 {
    ((chars as u64) + 3) / 4
}

fn tokenizer() -> Option<&'static (PathBuf, Tokenizer)> {
    TOKENIZER
        .get_or_init(|| {
            let path = std::env::var_os("KCODE_TOKENIZER_JSON")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("KCODE_SIDECAR_TOKENIZER_JSON").map(PathBuf::from));
            let path = path?;
            Tokenizer::from_file(&path).ok().map(|tok| (path, tok))
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_tokenization_is_nonzero() {
        let rec = tokenize_text("hello world");
        assert!(rec.token_count > 0);
        assert_eq!(rec.char_count, 11);
    }
}
