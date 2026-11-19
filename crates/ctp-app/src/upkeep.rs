//! Подстройка под установленный Claude Desktop, чтобы обновление Claude не ломало
//! приложение молча: правила, по которым Claude прячет модели шлюза, берутся из его
//! `app.asar`; перевод обновляется при смене версии; русская копия собирается заранее;
//! итог — список проверок для окна настроек.
//!
//! Проверки статические (файлы Claude читаются, не запускаются): «пройдено» значит
//! «нашлось то, на что опирается приложение», а не «Claude точно примет конфиг».

use crate::model::Config;
use crate::{ruui, tr};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const RULES_CACHE: &str = "claude-rules.json";
const TRANSLATION_META: &str = "claude-ru.meta.json";
const TRANSLATION_URL: &str = "https://claude.ai/i18n/ru.json";
const MAX_TRANSLATION: u64 = 64 * 1024 * 1024;
/// Поля 3p-конфига, которые пишет приложение (`claude.rs`), и модель-образец уровней.
const CONFIG_FIELDS: [&str; 6] = [
    "inferenceModels",
    "labelOverride",
    "supports1m",
    "behavesAs",
    "maxEffort",
    crate::claude::EFFORT_PROFILE,
];

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Папка версии Claude (`app-6.6395.0`); пусто — Claude не найден.
    pub version: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub ok: bool,
    pub text: String,
}

impl Check {
    fn push(&mut self, ok: bool, text: impl Into<String>) {
        self.items.push(Item {
            ok,
            text: text.into(),
        });
    }
}

/// `…\app-6.6395.0\claude.exe` → папка версии; заглушка или свой путь — `None`.
pub fn app_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let name = dir.file_name()?.to_str()?;
    name.starts_with("app-").then(|| dir.to_path_buf())
}

/// Правило из `app.asar`: литерал вида `X=/…deepseek…/;function f(e){return X.test(e.toLowerCase())}`.
/// Ищем по строению, не по имени (имена минифицированы и меняются от сборки к сборке).
/// Разные копии объединяются: модель отбрасывается, если её отбросит любая.
pub fn extract_rules(app_asar: &[u8]) -> Result<String> {
    let shape = regex::bytes::Regex::new(
        r"=/((?:[^/\\\n]|\\.)+)/;function [\w$]+\([\w$]+\)\{return [\w$]+\.test\([\w$]+\.toLowerCase\(\)\)\}",
    )
    .expect("rules shape");
    let mut found: Vec<String> = Vec::new();
    for caps in shape.captures_iter(app_asar) {
        let literal = String::from_utf8_lossy(&caps[1]).into_owned();
        if literal.contains("deepseek") && !found.contains(&literal) {
            found.push(literal);
        }
    }
    ensure!(
        !found.is_empty(),
        "{}",
        tr!(
            "в app.asar не найдено правило отбора моделей",
            "no model filter rule found in app.asar"
        )
    );
    Ok(if found.len() == 1 {
        found.remove(0)
    } else {
        found
            .iter()
            .map(|r| format!("(?:{r})"))
            .collect::<Vec<_>>()
            .join("|")
    })
}

#[derive(Serialize, Deserialize, PartialEq)]
struct RulesCache {
    asar: String,
    len: u64,
    modified: u64,
    pattern: String,
}

fn stamp(path: &Path) -> Result<(u64, u64)> {
    let meta = fs::metadata(path).with_context(|| path.display().to_string())?;
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), modified))
}

/// Правило установленного Claude (из кеша, если `app.asar` тот же).
fn rules_of(config_dir: &Path, app: &Path) -> Result<String> {
    let asar = app.join("resources").join("app.asar");
    let (len, modified) = stamp(&asar)?;
    let cache_path = config_dir.join(RULES_CACHE);
    let key = asar.display().to_string();
    if let Some(cache) = fs::read(&cache_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<RulesCache>(&raw).ok())
        .filter(|c| c.asar == key && c.len == len && c.modified == modified)
    {
        return Ok(cache.pattern);
    }
    let pattern = extract_rules(&fs::read(&asar).with_context(|| key.clone())?)?;
    let cache = RulesCache {
        asar: key,
        len,
        modified,
        pattern: pattern.clone(),
    };
    let _ = fs::write(&cache_path, serde_json::to_vec(&cache)?);
    Ok(pattern)
}

/// Подставляет правило установленного Claude. `Ok(true)` — правило поменялось
/// (псевдонимы моделей могли измениться: шлюз надо перезапустить до записи конфига).
/// Ошибка — действует вшитое правило, текст ошибки — для самопроверки.
pub fn apply_rules(config_dir: &Path, exe: &Path) -> Result<bool> {
    let applied = app_dir(exe)
        .with_context(|| {
            tr!(
                "Claude найден не в папке версии app-…",
                "Claude is not in an app-… version folder"
            )
        })
        .and_then(|app| rules_of(config_dir, &app))
        .and_then(|pattern| {
            ctp_core::alias::set_desktop_rules(Some(&pattern)).map_err(|exc| {
                anyhow::anyhow!(
                    "{}",
                    tr!(
                        "правило Claude не разобрано ({exc}), действует вшитое",
                        "Claude rule not parsed ({exc}); the built-in one applies"
                    )
                )
            })
        });
    match applied {
        Ok(changed) => Ok(changed),
        Err(exc) => {
            let _ = ctp_core::alias::set_desktop_rules(None);
            Err(exc)
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslationMeta {
    claude_version: String,
}

/// Доля английских строк интерфейса, у которых есть русский перевод.
fn coverage(en_keys: &HashSet<String>, ru: &serde_json::Map<String, Value>) -> f64 {
    if en_keys.is_empty() {
        return 0.0;
    }
    en_keys.iter().filter(|k| ru.contains_key(*k)).count() as f64 / en_keys.len() as f64
}

fn parse_translation(raw: &[u8]) -> Result<serde_json::Map<String, Value>> {
    let map: serde_json::Map<String, Value> = serde_json::from_slice(raw).with_context(|| {
        tr!(
            "перевод — не JSON-объект",
            "translation is not a JSON object"
        )
    })?;
    let count = map.len();
    ensure!(
        count > 1000,
        "{}",
        tr!(
            "в переводе слишком мало строк ({count})",
            "translation has too few strings ({count})"
        )
    );
    Ok(map)
}

/// Перевод с claude.ai через системный `curl.exe`: Cloudflare пускает не всех по
/// отпечатку TLS — reqwest и .NET получают 403, curl (Schannel) — 200 (19.11.2026).
/// shortcut: защита Cloudflare может закрыть и curl — тогда остаётся прежний файл и
/// ручное скачивание из браузера; переделать, если 403 станет постоянным.
fn download_translation() -> Result<Vec<u8>> {
    let curl = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("curl.exe"))
        .filter(|path| path.is_file())
        .with_context(|| tr!("нет системного curl.exe", "system curl.exe not found"))?;
    let mut command = std::process::Command::new(curl);
    command.args([
        "-sS",
        "-f",
        "--max-time",
        "30",
        "--max-filesize",
        "67108864",
        "-A",
        ctp_gateway::state::USER_AGENT,
        TRANSLATION_URL,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: без мелькающей консоли
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    ensure!(
        output.stdout.len() as u64 <= MAX_TRANSLATION,
        "{}",
        tr!("ответ больше 64 МБ", "response is larger than 64 MB")
    );
    Ok(output.stdout)
}

/// Одно скачивание за раз (старт и «Открыть Claude» после обновления Claude видят смену
/// версии одновременно) и пауза после неудачи: без сети кнопка не ждёт таймаута на
/// каждом нажатии.
static DOWNLOAD: std::sync::Mutex<Option<(String, std::time::Instant)>> =
    std::sync::Mutex::new(None);
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// Свежий перевод при смене версии Claude (или если файла нет). Худший по покрытию
/// строк этой версии не заменяет лучший. Возвращает строку для самопроверки.
pub fn refresh_translation(config_dir: &Path, app: &Path) -> Result<String> {
    let version = app
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let file = config_dir.join(ruui::TRANSLATION_FILE);
    let meta_path = config_dir.join(TRANSLATION_META);
    let mut last_failure = DOWNLOAD.lock().unwrap_or_else(|p| p.into_inner());
    let meta: TranslationMeta = fs::read(&meta_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    let en: HashSet<String> = ruui::read_asar_file(
        &app.join("resources").join("ion-dist.asar"),
        "i18n/en-US.json",
    )
    .ok()
    .and_then(|raw| serde_json::from_slice::<serde_json::Map<String, Value>>(&raw).ok())
    .map(|m| m.into_iter().map(|(k, _)| k).collect())
    .unwrap_or_default();
    let current = fs::read(&file)
        .ok()
        .and_then(|raw| parse_translation(&raw).ok());

    let mut note = String::new();
    let failed_recently = last_failure
        .as_ref()
        .is_some_and(|(v, at)| *v == version && at.elapsed() < RETRY_AFTER);
    if (current.is_none() || meta.claude_version != version) && failed_recently {
        note = tr!(
            " (скачать не вышло, повтор позже)",
            " (download failed, will retry later)"
        );
        if current.is_none() {
            anyhow::bail!(
                "{}",
                tr!(
                    "перевод не скачан, повтор через полчаса",
                    "translation not downloaded, retrying in half an hour"
                )
            );
        }
    } else if current.is_none() || meta.claude_version != version {
        let downloaded =
            download_translation().and_then(|raw| parse_translation(&raw).map(|map| (raw, map)));
        *last_failure = downloaded
            .is_err()
            .then(|| (version.clone(), std::time::Instant::now()));
        match downloaded {
            Ok((raw, fresh)) => {
                let worse = current
                    .as_ref()
                    .is_some_and(|old| coverage(&en, &fresh) + 1e-9 < coverage(&en, old));
                if worse {
                    note = tr!(
                        " (скачанный перевод хуже — оставлен прежний)",
                        " (downloaded translation is worse, kept the previous one)"
                    );
                } else if current.as_ref() != Some(&fresh) {
                    let tmp = file.with_extension("json.download");
                    fs::write(&tmp, &raw)?;
                    fs::rename(&tmp, &file)?;
                    note = tr!(" (обновлён)", " (updated)");
                }
                let meta = TranslationMeta {
                    claude_version: version,
                };
                fs::write(&meta_path, serde_json::to_vec(&meta)?)?;
            }
            Err(exc) if current.is_some() => {
                note = tr!(" (обновить не вышло: {exc:#})", " (update failed: {exc:#})");
            }
            Err(exc) => {
                return Err(exc.context(tr!("перевод не скачан", "translation not downloaded")));
            }
        }
    }
    let map = parse_translation(&fs::read(&file)?)?;
    let share = coverage(&en, &map);
    Ok(if en.is_empty() {
        let count = map.len();
        tr!(
            "Перевод: {count} строк{note}",
            "Translation: {count} strings{note}"
        )
    } else {
        let percent = share * 100.0;
        tr!(
            "Перевод: {percent:.0}% строк этой версии Claude{note}",
            "Translation: {percent:.0}% of this Claude version's strings{note}"
        )
    })
}

/// Перед «Открыть Claude»: правила моделей и перевод для текущей версии. `true` —
/// псевдонимы могли измениться (перезапустить шлюз до записи конфига).
pub fn prepare(config: &Config, config_dir: &Path) -> bool {
    let Some(exe) = crate::claude::detect(config) else {
        return false;
    };
    let changed = apply_rules(config_dir, &exe).unwrap_or_else(|exc| {
        tracing::warn!(error = %format!("{exc:#}"), "Claude model rule");
        false
    });
    if config.russian_ui
        && let Some(app) = app_dir(&exe)
        && let Err(exc) = refresh_translation(config_dir, &app)
    {
        tracing::warn!(error = %format!("{exc:#}"), "Claude translation");
    }
    changed
}

/// Полная самопроверка; с `build_copy` заодно собирает русскую копию заранее.
pub fn check(config: &Config, config_dir: &Path, build_copy: bool) -> Check {
    let mut check = Check::default();
    let Some(exe) = crate::claude::detect(config) else {
        check.push(
            false,
            tr!("Claude Desktop не найден", "Claude Desktop not found"),
        );
        return check;
    };
    let Some(app) = app_dir(&exe) else {
        check.version = exe.display().to_string();
        check.push(
            false,
            tr!(
                "Claude найден не в папке версии app-… — проверки файлов пропущены",
                "Claude is not in an app-… version folder; file checks skipped"
            ),
        );
        return check;
    };
    check.version = app
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .trim_start_matches("app-")
        .to_string();

    match apply_rules(config_dir, &exe) {
        Ok(_) => {
            let same = rules_of(config_dir, &app)
                .is_ok_and(|p| p == ctp_core::alias::builtin_desktop_rules());
            check.push(
                true,
                if same {
                    tr!(
                        "Правило отбора моделей прочитано из Claude (как в прошлой версии)",
                        "Model filter rule read from Claude (same as the previous version)"
                    )
                } else {
                    tr!(
                        "Правило отбора моделей прочитано из Claude (новое — применено)",
                        "Model filter rule read from Claude (new, applied)"
                    )
                },
            );
        }
        Err(exc) => check.push(false, format!("{exc:#}")),
    }
    let rejected: Vec<String> = config
        .providers
        .iter()
        .flat_map(crate::store::alias_rows)
        .filter(|row| ctp_core::alias::desktop_rejects(&row.alias))
        .map(|row| row.alias)
        .collect();
    if rejected.is_empty() {
        // Только запретный список: вторую половину условия Claude (в имени «claude» или
        // семейство) псевдоним `claude-…` выполняет по построению, её не проверяем.
        check.push(
            true,
            tr!(
                "Имена моделей шлюза не попадают под запрет Claude",
                "Gateway model names pass Claude's block list"
            ),
        );
    } else {
        let rejected = rejected.join(", ");
        check.push(
            false,
            tr!(
                "Claude скроет модели: {rejected}",
                "Claude will hide models: {rejected}"
            ),
        );
    }

    match fs::read(app.join("resources").join("app.asar")) {
        Ok(asar) => {
            let missing: Vec<&str> = CONFIG_FIELDS
                .iter()
                .copied()
                .filter(|field| !contains(&asar, field.as_bytes()))
                .collect();
            if missing.is_empty() {
                check.push(
                    true,
                    tr!(
                        "Поля конфига моделей на месте",
                        "Model config fields are in place"
                    ),
                );
            } else {
                let missing = missing.join(", ");
                check.push(
                    false,
                    tr!(
                        "В Claude нет полей конфига: {missing}",
                        "Claude lacks config fields: {missing}"
                    ),
                );
            }
        }
        Err(exc) => check.push(
            false,
            tr!("app.asar не прочитан: {exc}", "app.asar not read: {exc}"),
        ),
    }

    if config.russian_ui {
        match refresh_translation(config_dir, &app) {
            Ok(line) => check.push(true, line),
            Err(exc) => check.push(false, format!("{exc:#}")),
        }
        match ruui::locale_changes(&app.join("resources").join("ion-dist.asar")) {
            Ok(_) => check.push(
                true,
                tr!(
                    "Русский язык встраивается в эту версию",
                    "Russian can be added to this version"
                ),
            ),
            Err(exc) => check.push(
                false,
                tr!("Русский интерфейс: {exc:#}", "Russian interface: {exc:#}"),
            ),
        }
        if build_copy {
            match fs::read(config_dir.join(ruui::TRANSLATION_FILE))
                .map_err(anyhow::Error::from)
                .and_then(|ru| ruui::ensure_copy(&app, &crate::claude::copies_root()?, &ru))
            {
                Ok(_) => check.push(true, tr!("Русская копия готова", "Russian copy is ready")),
                Err(exc) => check.push(
                    false,
                    tr!("Русская копия: {exc:#}", "Russian copy: {exc:#}"),
                ),
            }
        }
    }
    check
}

/// Самопроверка с записью итога в состояние окна. Одна за раз: повторные клики
/// «Открыть Claude» не читают 50 МБ параллельно и старый итог не затирает новый.
pub fn check_into(app: &tauri::AppHandle, config: &Config, config_dir: &Path, build_copy: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Manager;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    // Сброс и при панике внутри check: иначе самопроверки молча прекратились бы.
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _release = Release;
    let result = check(config, config_dir, build_copy);
    *app.state::<crate::state::AppShared>()
        .claude
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(result);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_found_by_shape_and_merged() {
        let one =
            br#"x;eke=/ark|deepseek|\bling\b/;function tke(e){return eke.test(e.toLowerCase())}y"#;
        assert_eq!(extract_rules(one).unwrap(), r"ark|deepseek|\bling\b");
        let twice = [
            one.as_slice(),
            br#"UM=/ark|deepseek|\bling\b/;function Ese(e){return UM.test(e.toLowerCase())}"#,
        ]
        .concat();
        assert_eq!(extract_rules(&twice).unwrap(), r"ark|deepseek|\bling\b");
        let differ = [
            one.as_slice(),
            br#"Q=/deepseek|newvendor/;function $q(e){return Q.test(e.toLowerCase())}"#,
        ]
        .concat();
        let merged = extract_rules(&differ).unwrap();
        assert_eq!(merged, r"(?:ark|deepseek|\bling\b)|(?:deepseek|newvendor)");
        assert!(
            regex::Regex::new(&merged)
                .unwrap()
                .is_match("claude-newvendor-1")
        );
        // Чужие регулярки без deepseek и другой формы — не правило.
        assert!(
            extract_rules(br#"a=/foo|bar/;function f(e){return a.test(e.toLowerCase())}"#).is_err()
        );
        assert!(extract_rules(b"nothing here").is_err());
    }

    #[test]
    fn coverage_counts_english_keys_with_russian() {
        let en: HashSet<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let ru: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"a":"1","b":"2","zz":"3"}"#).unwrap();
        assert!((coverage(&en, &ru) - 0.5).abs() < 1e-9);
        assert_eq!(coverage(&HashSet::new(), &ru), 0.0);
    }

    #[test]
    fn app_dir_only_for_version_folders() {
        let exe = Path::new(r"C:\x\AnthropicClaude\app-6.1.0\claude.exe");
        assert_eq!(
            app_dir(exe).unwrap(),
            Path::new(r"C:\x\AnthropicClaude\app-6.1.0")
        );
        assert!(app_dir(Path::new(r"C:\x\AnthropicClaude\claude.exe")).is_none());
    }

    /// Вся самопроверка на установленном Claude, папка настроек временная, без сборки
    /// копии (скачивает перевод): `cargo test -p ctp-app check_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn check_real_install() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-upkeep-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let config = Config {
            russian_ui: true,
            providers: vec![crate::model::ProviderRow {
                id: "opencode".into(),
                kind: "openai".into(),
                base_url: "https://opencode.ai/inference/go/v1".into(),
                models: vec!["deepseek-v4.1-flash[1m]".into()],
                ..Default::default()
            }],
            ..Config::default()
        };
        let started = std::time::Instant::now();
        let first = check(&config, &dir, false);
        println!("{} за {:?}", first.version, started.elapsed());
        for item in &first.items {
            println!("  {} {}", if item.ok { "ok " } else { "ERR" }, item.text);
        }
        // Второй раз — без скачивания (версия та же) и из кеша правила.
        let started = std::time::Instant::now();
        let second = check(&config, &dir, false);
        println!("повторно за {:?}", started.elapsed());
        // Папку убираем до проверок: упавший assert не должен её оставлять.
        fs::remove_dir_all(&dir).unwrap();
        assert!(first.items.iter().all(|i| i.ok), "{:?}", first.items);
        assert!(second.items.iter().all(|i| i.ok));
    }

    /// На установленном Claude правило совпадает с вшитым:
    /// `CTP_RU_APP=<…\app-x> cargo test -p ctp-app rules_of_real -- --ignored`
    #[test]
    #[ignore]
    fn rules_of_real_install() {
        let app = PathBuf::from(std::env::var_os("CTP_RU_APP").expect("CTP_RU_APP"));
        let asar = fs::read(app.join("resources").join("app.asar")).unwrap();
        assert_eq!(
            extract_rules(&asar).unwrap(),
            ctp_core::alias::builtin_desktop_rules()
        );
    }
}
