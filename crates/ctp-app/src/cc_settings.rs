//! Советник Claude Code (`advisorModel` в его пользовательских настройках). Claude Code
//! объявляет советника, только если эта настройка задана; `/advisor` пишет её туда же.
//! Файл общий для всех запусков Claude Code этого профиля: консоль, вкладка Code у
//! Claude Desktop, обычный Claude Code с аккаунтом Anthropic. Поэтому правка точечная —
//! одна строка верхнего уровня, остальное байт в байт, с проверкой перед записью.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const KEY: &str = "advisorModel";
/// Что пишется при включении: «opus» Claude Code в режиме шлюза ведёт к сильной модели.
pub const ADVISOR_VALUE: &str = "opus";
const BOM: char = '\u{feff}';

/// `settings.json` того профиля Claude Code, который увидят запускаемые сессии: они
/// наследуют `CLAUDE_CONFIG_DIR` программы (его нет в `claude::HOST_ENV_PREFIXES`).
pub fn settings_path() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("settings.json")),
        None => dirs::home_dir().map(|home| home.join(".claude").join("settings.json")),
    }
}

/// Модель советника из настроек: `Ok(Some)` — непустая строка, `Ok(None)` — нет ключа,
/// пусто или файла нет, `Err` — файл есть, но не читается как объект JSON (тогда
/// программа его не трогает и галочку не предлагает).
pub fn read_advisor(path: &Path) -> Result<Option<String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(exc) => return Err(exc.to_string()),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches(BOM))
        .map_err(|exc| format!("not valid JSON ({exc})"))?;
    let object = value.as_object().ok_or("not a JSON object")?;
    Ok(object
        .get(KEY)
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .map(str::to_string))
}

/// Включить (ключ `opus`, если своей модели нет) или выключить (ключа нет) советника.
/// Свой выбор человека при включении не трогается. `true` — файл изменён.
pub fn set_advisor(path: &Path, on: bool) -> Result<bool, String> {
    // Claude Code пишет этот файл сам: если он успел изменить его, пока мы правили,
    // правка делается заново по свежему тексту, а не затирает его изменение.
    for _ in 0..3 {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => {
                if !on {
                    return Ok(false);
                }
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|exc| format!("{}: {exc}", dir.display()))?;
                }
                let text = format!("{{\n  \"{KEY}\": \"{ADVISOR_VALUE}\"\n}}\n");
                write_file(path, &text)?;
                return Ok(true);
            }
            Err(exc) => return Err(format!("{}: {exc}", path.display())),
        };
        let (bom, body) = match text.strip_prefix(BOM) {
            Some(body) => (true, body),
            None => (false, text.as_str()),
        };
        let Some(edited) = edit(body, on).map_err(|why| format!("{}: {why}", path.display()))?
        else {
            return Ok(false);
        };
        let edited = if bom {
            format!("{BOM}{edited}")
        } else {
            edited
        };
        if fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
            continue;
        }
        // Нетронутый оригинал — один раз: следующая запись не затрёт его уже изменённым.
        let backup = path.with_extension("json.bak");
        if !backup.exists() {
            fs::write(&backup, &text).map_err(|exc| format!("{}: {exc}", backup.display()))?;
        }
        write_file(path, &edited)?;
        return Ok(true);
    }
    Err(format!(
        "{}: the file keeps changing, try again",
        path.display()
    ))
}

/// Через временный файл и замену; символическая ссылка остаётся ссылкой (пишется цель).
/// Так же пишутся настройки программы и профиль Claude: обрыв записи (полный диск, питание,
/// антивирус) оставляет прежний файл целым, а не пустым.
pub(crate) fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return fs::write(path, text).map_err(|exc| format!("{}: {exc}", path.display()));
    }
    let temp = path.with_extension("json.ctp-tmp");
    fs::write(&temp, text).map_err(|exc| format!("{}: {exc}", temp.display()))?;
    fs::rename(&temp, path).map_err(|exc| {
        let _ = fs::remove_file(&temp);
        format!("{}: {exc}", path.display())
    })
}

/// Новый текст или `None`, если менять нечего. Ошибка — файл не объект JSON или правка
/// не сошлась (тогда ничего не пишется).
fn edit(body: &str, on: bool) -> Result<Option<String>, String> {
    let original: Value =
        serde_json::from_str(body).map_err(|exc| format!("not valid JSON ({exc})"))?;
    let Some(object) = original.as_object() else {
        return Err("not a JSON object".to_string());
    };
    let current = object.get(KEY);
    let quoted = serde_json::to_string(ADVISOR_VALUE).expect("string");
    let span = top_level_key(body, KEY);
    let edited = match (on, current) {
        // Своя модель уже выбрана — включено.
        (true, Some(Value::String(model))) if !model.is_empty() => return Ok(None),
        (false, None) => return Ok(None),
        (true, Some(_)) => {
            let span = span.ok_or("advisorModel not found at the top level")?;
            format!(
                "{}{quoted}{}",
                &body[..span.value_start],
                &body[span.value_end..]
            )
        }
        (true, None) => insert(body, &quoted)?,
        (false, Some(_)) => remove(
            body,
            &span.ok_or("advisorModel not found at the top level")?,
        )?,
    };
    // Проверка перед записью: JSON цел, ключ какой задуман, всё остальное как было.
    let result: Value =
        serde_json::from_str(&edited).map_err(|_| "the edit broke the JSON".to_string())?;
    let mut expected = object.clone();
    if on {
        expected.insert(KEY.to_string(), Value::String(ADVISOR_VALUE.to_string()));
    } else {
        expected.remove(KEY);
    }
    if result.as_object() != Some(&expected) {
        return Err("the edit changed more than advisorModel".to_string());
    }
    Ok(Some(edited))
}

struct Span {
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

/// Ключ верхнего уровня (не вложенный, не внутри строки) и границы его значения.
fn top_level_key(body: &str, name: &str) -> Option<Span> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let end = string_end(bytes, i)?;
                if depth == 1 && &body[i + 1..end] == name {
                    let colon = skip_ws(bytes, end + 1);
                    if bytes.get(colon) == Some(&b':') {
                        let value_start = skip_ws(bytes, colon + 1);
                        let value_end = value_end(bytes, value_start)?;
                        return Some(Span {
                            key_start: i,
                            value_start,
                            value_end,
                        });
                    }
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Индекс закрывающей кавычки строки, что начинается в `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Конец значения JSON (исключительно), начиная с `start`.
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string_end(bytes, start).map(|end| end + 1),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut i = start;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => i = string_end(bytes, i)?,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = start;
            while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b']') {
                i += 1;
            }
            // Без хвостовых пробелов: `true  ,` → конец после `true`.
            while i > start && bytes[i - 1].is_ascii_whitespace() {
                i -= 1;
            }
            Some(i)
        }
    }
}

/// Ключ первым в объекте, в стиле файла: с отступом на новой строке или компактно.
fn insert(body: &str, quoted: &str) -> Result<String, String> {
    let bytes = body.as_bytes();
    let open = skip_ws(bytes, 0);
    if bytes.get(open) != Some(&b'{') {
        return Err("not a JSON object".to_string());
    }
    let first = skip_ws(bytes, open + 1);
    let gap = &body[open + 1..first];
    let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
    if bytes.get(first) == Some(&b'}') {
        // Пустой объект: `{}` → ключ на своей строке.
        return Ok(format!(
            "{}{{{newline}  \"{KEY}\": {quoted}{newline}}}{}",
            &body[..open],
            &body[first + 1..]
        ));
    }
    let entry = match gap.rfind('\n') {
        Some(at) => format!("{newline}{}\"{KEY}\": {quoted},", &gap[at + 1..]),
        None => format!("\"{KEY}\":{quoted},"),
    };
    Ok(format!("{}{entry}{}", &body[..open + 1], &body[open + 1..]))
}

/// Ключ убирается вместе со своей строкой и одной запятой, остальной текст не трогается.
fn remove(body: &str, span: &Span) -> Result<String, String> {
    let bytes = body.as_bytes();
    let after = skip_ws(bytes, span.value_end);
    if bytes.get(after) == Some(&b',') {
        // Не последний: от начала его строки (с переводом строки перед ней) до запятой.
        let mut start = span.key_start;
        while start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
            start -= 1;
        }
        if start > 0 && bytes[start - 1] == b'\n' {
            start -= 1;
            if start > 0 && bytes[start - 1] == b'\r' {
                start -= 1;
            }
        }
        return Ok(format!("{}{}", &body[..start], &body[after + 1..]));
    }
    // Последний: от запятой перед ним; единственный — объект становится пустым.
    let mut before = span.key_start;
    while before > 0 && bytes[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    match bytes.get(before.wrapping_sub(1)) {
        Some(b',') => Ok(format!(
            "{}{}",
            &body[..before - 1],
            &body[span.value_end..]
        )),
        Some(b'{') => Ok(format!("{}{}", &body[..before], &body[after..])),
        _ => Err("unexpected JSON layout around advisorModel".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(body: &str) -> String {
        edit(body, true).unwrap().unwrap()
    }

    fn off(body: &str) -> String {
        edit(body, false).unwrap().unwrap()
    }

    #[test]
    fn on_then_off_gives_the_file_back_byte_for_byte() {
        for original in [
            "{\n  \"model\": \"opus\",\n  \"env\": {\"advisorModel\": \"x\"}\n}\n",
            "{\r\n  \"a\": 1,\r\n  \"b\": [1, 2]\r\n}\r\n",
            "{\"a\":1,\"b\":\"advisorModel\"}",
            "{}",
            "{\n}\n",
            "  {\n\t\"a\": true\n}",
        ] {
            let enabled = on(original);
            assert_eq!(
                serde_json::from_str::<Value>(&enabled).unwrap()[KEY],
                ADVISOR_VALUE,
                "{enabled}"
            );
            // Вложенный и строковый `advisorModel` не тронуты.
            if original.contains("\"env\"") {
                assert!(enabled.contains("{\"advisorModel\": \"x\"}"));
            }
            let disabled = off(&enabled);
            if original == "{\n}\n" {
                // Пустой объект с переводом строки становится `{}` — допустимо.
                assert_eq!(disabled, "{}\n");
            } else {
                assert_eq!(disabled, original, "{enabled:?}");
            }
        }
    }

    #[test]
    fn style_of_the_file_is_kept() {
        assert_eq!(
            on("{\n    \"a\": 1\n}\n"),
            "{\n    \"advisorModel\": \"opus\",\n    \"a\": 1\n}\n"
        );
        assert_eq!(
            on("{\r\n  \"a\": 1\r\n}"),
            "{\r\n  \"advisorModel\": \"opus\",\r\n  \"a\": 1\r\n}"
        );
        assert_eq!(on("{\"a\":1}"), "{\"advisorModel\":\"opus\",\"a\":1}");
    }

    #[test]
    fn existing_choice_is_kept_and_empty_is_replaced() {
        // Своя модель — включено, файл не меняется.
        assert!(
            edit("{\"advisorModel\": \"fable\"}", true)
                .unwrap()
                .is_none()
        );
        // Пустая строка (так тоже бывает выключено) — включается в том же месте.
        assert_eq!(
            on("{\n  \"a\": 1,\n  \"advisorModel\": \"\",\n  \"b\": 2\n}"),
            "{\n  \"a\": 1,\n  \"advisorModel\": \"opus\",\n  \"b\": 2\n}"
        );
        // Выключить: ключ последним, средним, единственным.
        assert_eq!(
            off("{\n  \"a\": 1,\n  \"advisorModel\": \"fable\"\n}"),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            off("{\n  \"a\": 1,\n  \"advisorModel\": \"fable\",\n  \"b\": 2\n}"),
            "{\n  \"a\": 1,\n  \"b\": 2\n}"
        );
        assert_eq!(off("{ \"advisorModel\": null }"), "{}");
        assert!(edit("{\"a\": 1}", false).unwrap().is_none());
    }

    #[test]
    fn broken_or_odd_files_are_refused() {
        assert!(edit("{\"a\": 1,}", true).is_err());
        assert!(edit("[1, 2]", true).is_err());
        assert!(edit("", true).is_err());
    }

    #[test]
    fn file_is_written_with_bom_kept_and_backup_once() {
        let dir = std::env::temp_dir().join(format!("ctp-cc-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        // Нет файла: выключить — ничего, включить — создаётся.
        assert!(!set_advisor(&path, false).unwrap());
        assert_eq!(read_advisor(&path), Ok(None));
        assert!(set_advisor(&path, true).unwrap());
        assert_eq!(read_advisor(&path).unwrap().as_deref(), Some(ADVISOR_VALUE));
        assert!(!dir.join("settings.json.bak").exists());

        let original = format!("{BOM}{{\n  \"a\": 1\n}}\n");
        fs::write(&path, &original).unwrap();
        assert!(set_advisor(&path, true).unwrap());
        assert!(!set_advisor(&path, true).unwrap());
        assert!(fs::read_to_string(&path).unwrap().starts_with(BOM));
        assert!(set_advisor(&path, false).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        // Копия — тот самый первый оригинал, не промежуточная версия.
        assert_eq!(
            fs::read_to_string(dir.join("settings.json.bak")).unwrap(),
            original
        );
        // Битый, пустой или не объект — не трогается ни включением, ни выключением, и
        // чтение говорит, что файл не годится (галочку тогда не предлагают).
        for broken in ["{oops", "", "[1]", "{\"a\": 1,}"] {
            fs::write(&path, broken).unwrap();
            assert!(read_advisor(&path).is_err(), "{broken:?}");
            assert!(set_advisor(&path, true).is_err());
            assert!(set_advisor(&path, false).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
