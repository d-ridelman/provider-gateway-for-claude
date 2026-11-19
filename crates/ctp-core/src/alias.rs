use sha2::{Digest, Sha256};

/// Псевдоним, который понимает Claude Desktop: `claude-<провайдер>-<8 цифр>`, цифры —
/// из хеша `provider/model`. Формулу не менять: по псевдониму Claude помнит выбор модели.
pub fn model_alias(provider_id: &str, model_id: &str) -> String {
    let seed = format!("{provider_id}/{model_id}");
    let digest = Sha256::digest(seed.as_bytes());
    let n = u32::from_le_bytes(digest[..4].try_into().expect("sha256 is 32 bytes")) % 100_000_000;
    format!("claude-{provider_id}-{n:08}")
}

/// Имена чужих моделей и вендоров, которые Claude Desktop 6.x не пускает в список
/// моделей шлюза (`eke` в `app.asar` 6.6395.0, функция `Fo`): имя, где встречается
/// любое из них, отбрасывается, даже если начинается с `claude-`.
const DESKTOP_BLOCKED: &str = r"ark-code|astron|command-r|deepseek|doubao|gemini|gemma|glm|gpt|grok|hermes|hy3|kimi|lfm|\bling\b|llama|longcat|mimo|minimax|mistral|mixtral|moonshot|nemotron|openai|phi-|qianfan|qwen|tc-code|\bunic\b|yi-|stepfun|step-3|seed-|bytedance|hunyuan|granite|amazon\.nova|nova-|devstral|ministral|ernie|codex|arcee|trinity|abab|phi\d|\bk2\.|\bm2\.|jamba|arctic|solar|mercury|zamba|kat-coder|\bds-|dpsk";

/// Правило из установленного Claude (`app.asar`), если его удалось прочитать; иначе
/// действует вшитое `DESKTOP_BLOCKED`.
static INSTALLED_RULES: std::sync::RwLock<Option<regex::Regex>> = std::sync::RwLock::new(None);

/// Подставляет правило установленного Claude (`None` — вернуть вшитое). `Ok(true)`, если
/// правило поменялось (псевдонимы могли измениться). Не компилируется — `Err`, правило
/// остаётся прежним.
pub fn set_desktop_rules(pattern: Option<&str>) -> Result<bool, regex::Error> {
    let next = pattern.map(regex::Regex::new).transpose()?;
    let mut rules = INSTALLED_RULES.write().unwrap_or_else(|p| p.into_inner());
    // Сравниваем действующее правило: «нет своего» и «своё, равное вшитому» — одно и то
    // же, иначе лишний перезапуск шлюза оборвал бы запросы открытого Claude.
    let same = rules.as_ref().map_or(DESKTOP_BLOCKED, regex::Regex::as_str)
        == next.as_ref().map_or(DESKTOP_BLOCKED, regex::Regex::as_str);
    *rules = next;
    Ok(!same)
}

/// Отбросит ли Claude Desktop такое имя модели шлюза.
pub fn desktop_rejects(name: &str) -> bool {
    let name = name.to_lowercase();
    if let Some(rules) = INSTALLED_RULES
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        return rules.is_match(&name);
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(DESKTOP_BLOCKED).expect("desktop blocklist"))
        .is_match(&name)
}

/// Вшитое правило — для сравнения с правилом установленного Claude.
pub fn builtin_desktop_rules() -> &'static str {
    DESKTOP_BLOCKED
}

/// Псевдоним для 3p-конфига Claude Desktop: `model_alias`, а если Claude его
/// отбросил бы (ID провайдера вроде `openai`, `deepseek`, `gemini`), — с нейтральной
/// меткой `p` + 6 цифр из хеша ID вместо ID: `claude-p123456-<8 цифр>`. Только
/// цифры — ни одно слово из списка в метке не сложится.
pub fn desktop_alias(provider_id: &str, model_id: &str) -> String {
    let alias = model_alias(provider_id, model_id);
    if !desktop_rejects(&alias) {
        return alias;
    }
    let digest = Sha256::digest(provider_id.as_bytes());
    let tag = u32::from_le_bytes(digest[..4].try_into().expect("sha256 is 32 bytes")) % 1_000_000;
    let number = &alias[alias.len() - 8..];
    format!("claude-p{tag:06}-{number}")
}

/// Отбрасывает суффикс `[1m]`, который Claude иногда добавляет к имени модели.
pub fn strip_1m(name: &str) -> &str {
    let name = name.trim();
    match name.len().checked_sub(4).and_then(|i| name.get(i..)) {
        Some(suffix) if suffix.eq_ignore_ascii_case("[1m]") => &name[..name.len() - 4],
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_stable() {
        // Значения закреплены: смена формулы потеряла бы выбор модели в профиле Claude.
        assert_eq!(model_alias("openai", "gpt-5.2"), "claude-openai-80673633");
        assert_eq!(
            model_alias("anthropic", "claude-sonnet-4-5"),
            "claude-anthropic-18183586"
        );
    }

    #[test]
    fn desktop_alias_avoids_names_claude_desktop_drops() {
        // Безопасные ID не меняются — сохранённые в Claude выборы моделей на месте.
        assert_eq!(
            desktop_alias("acme", "deepseek-v4.1-flash"),
            model_alias("acme", "deepseek-v4.1-flash")
        );
        assert_eq!(
            desktop_alias("anthropic", "claude-sonnet-4-5"),
            "claude-anthropic-18183586"
        );
        // Проблемные ID шаблонов: openai, deepseek, gemini, llamacpp.
        for id in ["openai", "deepseek", "gemini", "llamacpp", "my-qwen"] {
            assert!(desktop_rejects(&model_alias(id, "m")), "{id}");
            let alias = desktop_alias(id, "m");
            assert!(!desktop_rejects(&alias), "{alias}");
            assert!(alias.starts_with("claude-p"), "{alias}");
            assert!(alias.ends_with(&model_alias(id, "m")[model_alias(id, "m").len() - 8..]));
        }
        // Разные провайдеры с одной моделью — разные псевдонимы.
        assert_ne!(desktop_alias("openai", "m"), desktop_alias("deepseek", "m"));
        // Границы слов как у Claude: `ling` только отдельным словом.
        assert!(!desktop_rejects("claude-sibling-1"));
        assert!(desktop_rejects("claude-ling-1"));
    }

    #[test]
    fn strips_one_million_suffix() {
        assert_eq!(
            strip_1m("claude-openai-12345678[1m]"),
            "claude-openai-12345678"
        );
        assert_eq!(
            strip_1m("claude-openai-12345678[1M]"),
            "claude-openai-12345678"
        );
        assert_eq!(strip_1m("claude-openai-12345678"), "claude-openai-12345678");
        assert_eq!(strip_1m(""), "");
    }

    #[test]
    fn installed_rules_replace_builtin_and_report_change() {
        // Правило глобальное и тесты идут параллельно: подставляем то же вшитое,
        // чтобы соседние тесты видели прежнее поведение.
        let builtin = builtin_desktop_rules();
        assert!(
            !set_desktop_rules(Some(builtin)).unwrap(),
            "то же, что вшитое, — не изменение (лишний перезапуск шлюза)"
        );
        assert!(!set_desktop_rules(Some(builtin)).unwrap());
        assert!(desktop_rejects("claude-deepseek-1"));
        assert!(
            set_desktop_rules(Some("(?<=x)y")).is_err(),
            "не компилируется — Err"
        );
        assert!(
            desktop_rejects("claude-deepseek-1"),
            "прежнее правило осталось"
        );
        assert!(!set_desktop_rules(None).unwrap());
        assert!(desktop_rejects("claude-deepseek-1"));
        // Настоящая смена — Ok(true). Добавка не совпадёт ни с одним именем соседних тестов.
        let wider = format!("{builtin}|zzq-never-a-model");
        assert!(set_desktop_rules(Some(&wider)).unwrap());
        assert!(desktop_rejects("claude-zzq-never-a-model-1"));
        assert!(
            set_desktop_rules(None).unwrap(),
            "возврат к вшитому — тоже смена"
        );
    }
}
