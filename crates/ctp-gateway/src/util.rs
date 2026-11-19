use regex::Regex;
use std::sync::OnceLock;

/// Лимит вывода из текста ошибки провайдера. Нужен, чтобы один раз повторить
/// запрос с меньшим `max_tokens`, как это делает Python-шлюз через LiteLLM.
pub fn parse_output_limit(text: &str) -> Option<u64> {
    static FILTER: OnceLock<Regex> = OnceLock::new();
    static NUMBERS: OnceLock<Regex> = OnceLock::new();
    let filter = FILTER.get_or_init(|| {
        Regex::new(r"(?i)max_(completion_)?tokens|max.{0,20}output|context").expect("limit regex")
    });
    if !filter.is_match(text) {
        return None;
    }
    let numbers = NUMBERS.get_or_init(|| Regex::new(r"\b(\d{3,7})\b").expect("numbers regex"));
    numbers
        .captures_iter(text)
        .filter_map(|caps| caps[1].parse::<u64>().ok())
        .min()
}
