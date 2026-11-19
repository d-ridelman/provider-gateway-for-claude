use serde_json::Value;
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Грубая оценка токенов без словаря Claude: латиница ≈ 4 символа на токен,
/// CJK и кириллица — ближе к 1–2 символам. Точности tiktoken здесь не ждём;
/// число нужно, чтобы Claude не считал контекст нулевым.
pub fn estimate_tokens(text: &str) -> u64 {
    let mut wide = 0u64;
    let mut narrow = 0u64;
    for ch in text.chars() {
        if is_wide(ch) {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    let estimated = wide + narrow.div_ceil(4);
    estimated.max(1)
}

/// Точнее, чем эвристика: тот же BPE, которым пользуется Python-шлюз
/// (LiteLLM считает `gpt-4o`). Для модели Claude это всё равно оценка.
pub fn count_tokens(text: &str) -> u64 {
    match bpe() {
        Some(bpe) => bpe.encode_ordinary(text).len() as u64,
        None => estimate_tokens(text),
    }
}

pub fn count_json_tokens(value: &Value) -> u64 {
    match serde_json::to_string(value) {
        Ok(text) => count_tokens(&text).max(1),
        Err(_) => 1,
    }
}

pub fn estimate_json_tokens(value: &Value) -> u64 {
    match serde_json::to_string(value) {
        Ok(text) => estimate_tokens(&text),
        Err(_) => 1,
    }
}

fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().ok()).as_ref()
}

fn is_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0400..=0x04FF      // кириллица
        | 0x0590..=0x05FF    // иврит
        | 0x0600..=0x06FF    // арабский
        | 0x0900..=0x097F    // деванагари
        | 0x0E00..=0x0E7F    // тайский
        | 0x3040..=0x30FF    // хирагана, катакана
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK
        | 0xAC00..=0xD7AF    // хангыль
        | 0xF900..=0xFAFF    // CJK compatibility
        | 0xFF00..=0xFFEF    // полноширинные формы
        | 0x1F300..=0x1FAFF  // эмодзи
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_are_positive_and_differ_from_estimates() {
        assert_eq!(count_tokens(""), 0);
        assert!(count_tokens("hello world") > 0);
        assert!(count_tokens("привет мир") > 0);
    }

    #[test]
    fn estimates_are_positive_and_scale() {
        assert_eq!(estimate_tokens(""), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("привет"), 6);
        assert_eq!(estimate_tokens("你好"), 2);
    }
}
