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
/// Перевод подписей каталога моделей (меню усилий) с claude.ai, ADR-009.
const CATALOG_URL: &str = "https://claude.ai/i18n/dynamic/ru.json";
/// Скачанный `dynamic/ru.json`; нет файла — сборка обходится вшитым словарём.
const CATALOG_FILE: &str = "claude-ru-dynamic.json";
const MAX_TRANSLATION: u64 = 64 * 1024 * 1024;
/// Поля 3p-конфига, которые пишет приложение (`claude.rs`), и модель-образец уровней.
const CONFIG_FIELDS: [&str; 9] = [
    "inferenceModels",
    "labelOverride",
    "supports1m",
    "prefer1m",
    "behavesAs",
    "maxEffort",
    "anthropicFamilyTier",
    "isFamilyDefault",
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
    /// Необязательная возможность: её отсутствие показывается, но не зажигает
    /// предупреждение в заголовке окна.
    pub optional: bool,
}

impl Check {
    fn push(&mut self, ok: bool, text: impl Into<String>) {
        self.items.push(Item {
            ok,
            text: text.into(),
            optional: false,
        });
    }

    fn push_optional(&mut self, ok: bool, text: impl Into<String>) {
        self.items.push(Item {
            ok,
            text: text.into(),
            optional: true,
        });
    }
}

/// Имя папки копии этой версии (`app-X`): у Squirrel это имя папки, у MSIX — из имени
/// пакета. Единственное место, где считается версия для копий, меты перевода и проверок.
fn copy_version(app: &Path) -> String {
    crate::ruui::version_folder(app).unwrap_or_else(|_| {
        app.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    })
}

/// Папка версии: `…\app-6.6395.0\claude.exe` или `<package>\app\claude.exe` (MSIX);
/// заглушка или свой путь — `None`.
pub fn app_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    crate::ruui::version_folder(dir)
        .is_ok()
        .then(|| dir.to_path_buf())
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

/// Правило установленного Claude (из кеша, если `app.asar` тот же).
fn rules_of(config_dir: &Path, app: &Path) -> Result<String> {
    let asar = app.join("resources").join("app.asar");
    let (len, modified) = crate::ruui::stamp(&asar)?;
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
    /// Версия Claude, под которую скачан `CATALOG_FILE`.
    #[serde(default)]
    catalog_version: String,
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

/// Перевод с claude.ai: сначала системный `curl.exe` (быстро, без окна), а если
/// Cloudflare ответил проверкой на бота (403) — встроенный браузер (`webfetch`),
/// который её проходит сам. Без сети, при 404/5xx браузер не открывается: окно с
/// ошибкой страницы висело бы зря. 19.11.2026 утром curl пропускали, к обеду нет.
fn download_translation(url: &str, show_window: bool) -> Result<Vec<u8>> {
    download_with_curl(url).or_else(|curl| {
        if !is_bot_check(&format!("{curl:#}")) {
            return Err(curl);
        }
        tracing::info!(error = %format!("{curl:#}"), "curl refused, trying the built-in browser");
        crate::webfetch::fetch_json(url, show_window)
            .map(String::into_bytes)
            .map_err(|browser| {
                // stderr curl уже начинается с «curl:».
                anyhow::anyhow!(
                    "{}",
                    tr!(
                        "{curl:#}; браузер: {browser:#}",
                        "{curl:#}; browser: {browser:#}"
                    )
                )
            })
    })
}

/// Ошибка curl — ответ 403 (так Cloudflare отвечает программам проверкой на бота).
fn is_bot_check(curl_error: &str) -> bool {
    curl_error.contains("returned error: 403")
}

fn download_with_curl(url: &str) -> Result<Vec<u8>> {
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
        url,
    ]);
    // claude.ai из части стран закрыт: через прокси приложения, если он включён.
    if let Some(proxy) = ctp_gateway::proxy::current() {
        command.args(["-x", &proxy, "--noproxy", ctp_gateway::proxy::NO_PROXY]);
    }
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

/// Неудачная попытка скачать перевод: для какой версии Claude, когда и была ли она
/// фоновой (без окна).
struct Failure {
    version: String,
    at: std::time::Instant,
    hidden: bool,
}

/// Одно скачивание за раз (старт и «Открыть Claude» после обновления Claude видят смену
/// версии одновременно) и пауза после неудачи: без сети кнопка не ждёт таймаута на
/// каждом нажатии.
static DOWNLOAD: std::sync::Mutex<Option<Failure>> = std::sync::Mutex::new(None);
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
/// Фон после своей же неудачи (claude.ai просит галочку, нет сети) пробует реже: скрытое
/// окно браузера раз в полчаса сутками в трее ни к чему.
const WATCH_RETRY_AFTER: Duration = Duration::from_secs(3 * 60 * 60);

/// Пропустить ли попытку из-за недавней неудачи. Фоновая неудача не мешает попытке при
/// человеке: та может показать окно проверки, которое фону недоступно.
fn skip_after_failure(failure: Option<&Failure>, version: &str, mode: Fetch) -> bool {
    failure.is_some_and(|f| {
        let pause = if mode == Fetch::Watch && f.hidden {
            WATCH_RETRY_AFTER
        } else {
            RETRY_AFTER
        };
        f.version == version && (mode == Fetch::Watch || !f.hidden) && f.at.elapsed() < pause
    })
}

/// Кто просит перевод — от этого зависит, качать ли и можно ли показать окно.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fetch {
    /// Перед «Открыть Claude»: качаем, только если перевода нет вовсе — обновление под
    /// новую версию (до двух минут с проверкой claude.ai) кнопку не задерживает.
    Launch,
    /// Самопроверка при старте и после запуска: человек рядом, окно проверки можно показать.
    Check,
    /// Фоновая подготовка после обновления Claude: окно не показываем никогда.
    Watch,
}

/// Свежий перевод при смене версии Claude (или если файла нет). Худший по покрытию
/// строк этой версии не заменяет лучший. Возвращает строку для самопроверки.
pub fn refresh_translation(config_dir: &Path, app: &Path, mode: Fetch) -> Result<String> {
    let version = copy_version(app);
    let file = config_dir.join(ruui::TRANSLATION_FILE);
    let meta_path = config_dir.join(TRANSLATION_META);
    // Перед «Открыть Claude» при наличии перевода качать нечего: не ждём фоновую
    // загрузку (она держит DOWNLOAD до двух минут) и не читаем 50 МБ ради покрытия.
    // Строку prepare не показывает, только пишет ошибки в журнал.
    if mode == Fetch::Launch && fs::read(&file).is_ok_and(|raw| parse_translation(&raw).is_ok()) {
        return Ok(String::new());
    }
    let mut last_failure = DOWNLOAD.lock().unwrap_or_else(|p| p.into_inner());
    let meta: TranslationMeta = fs::read(&meta_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    // Английские строки версии (50 МБ архива) — только когда нужны: сравнить покрытие
    // или показать его в самопроверке.
    let english = || -> HashSet<String> {
        ruui::read_asar_file(
            &app.join("resources").join("ion-dist.asar"),
            "i18n/en-US.json",
        )
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Map<String, Value>>(&raw).ok())
        .map(|m| m.into_iter().map(|(k, _)| k).collect())
        .unwrap_or_default()
    };
    let current = fs::read(&file)
        .ok()
        .and_then(|raw| parse_translation(&raw).ok());

    let mut note = String::new();
    let mut en: Option<HashSet<String>> = None;
    let due = current.is_none() || (mode != Fetch::Launch && meta.claude_version != version);
    if due && skip_after_failure(last_failure.as_ref(), &version, mode) {
        note = tr!(
            " (скачать не вышло, повтор позже)",
            " (download failed, will retry later)"
        );
        if current.is_none() {
            anyhow::bail!("{}", not_downloaded(&file));
        }
    } else if due {
        let downloaded = download_translation(TRANSLATION_URL, mode != Fetch::Watch)
            .and_then(|raw| parse_translation(&raw).map(|map| (raw, map)))
            .and_then(|(raw, fresh)| {
                // Сохранение — часть попытки: не записалось (файл занят, диск полон) —
                // тоже неудача с паузой, а не новое скачивание на каждом круге.
                let en = en.insert(english());
                let worse = current
                    .as_ref()
                    .is_some_and(|old| coverage(en, &fresh) + 1e-9 < coverage(en, old));
                let saved = if worse {
                    tr!(
                        " (скачанный перевод хуже — оставлен прежний)",
                        " (downloaded translation is worse, kept the previous one)"
                    )
                } else if current.as_ref() != Some(&fresh) {
                    crate::store::write_bytes(&file, &raw)
                        .map_err(anyhow::Error::msg)
                        .with_context(|| {
                            let target = file.display();
                            tr!(
                                "скачанный перевод не записан в {target}",
                                "downloaded translation not saved to {target}"
                            )
                        })?;
                    tr!(" (обновлён)", " (updated)")
                } else {
                    String::new()
                };
                let meta = TranslationMeta {
                    claude_version: version.clone(),
                    catalog_version: meta.catalog_version.clone(),
                };
                crate::store::write_bytes(&meta_path, &serde_json::to_vec(&meta)?)
                    .map_err(anyhow::Error::msg)?;
                Ok(saved)
            });
        *last_failure = downloaded.is_err().then(|| Failure {
            version: version.clone(),
            at: std::time::Instant::now(),
            hidden: mode == Fetch::Watch,
        });
        match downloaded {
            Ok(saved) => note = saved,
            Err(exc) if current.is_some() => {
                note = tr!(" (обновить не вышло: {exc:#})", " (update failed: {exc:#})");
            }
            Err(exc) => return Err(exc.context(not_downloaded(&file))),
        }
    }
    // Перевод меню усилий — попытка без гарантий: его неудача основному не мешает.
    if mode != Fetch::Launch {
        refresh_catalog(config_dir, &version, mode);
    }
    // Фону строка самопроверки не нужна — не читаем ради неё 50 МБ.
    if mode == Fetch::Watch {
        return Ok(String::new());
    }
    let en = en.unwrap_or_else(english);
    let map = parse_translation(&fs::read(&file)?)?;
    let share = coverage(&en, &map);
    let catalog = fs::read(config_dir.join(CATALOG_FILE)).unwrap_or_default();
    let menu = ruui::catalog_ru(&app.join("resources").join("ion-dist.asar"), &map, &catalog);
    let (done, total) = (menu.translated, menu.total);
    Ok(if en.is_empty() {
        let count = map.len();
        tr!(
            "Перевод: {count} строк, меню усилий {done} из {total}{note}",
            "Translation: {count} strings, effort menu {done} of {total}{note}"
        )
    } else {
        let percent = share * 100.0;
        tr!(
            "Перевод: {percent:.0}% строк этой версии Claude, меню усилий {done} из {total}{note}",
            "Translation: {percent:.0}% of this Claude version's strings, effort menu {done} of {total}{note}"
        )
    })
}

/// Неудачи скачивания перевода меню — отдельно от основного: его пауза основной не задерживает.
static CATALOG_DOWNLOAD: std::sync::Mutex<Option<Failure>> = std::sync::Mutex::new(None);

/// Перевод меню усилий под версию Claude (ADR-009): раз на версию, после неудачи — с той
/// же паузой, что у основного. Прежний файл остаётся, пока не скачан новый.
fn refresh_catalog(config_dir: &Path, version: &str, mode: Fetch) {
    let meta_path = config_dir.join(TRANSLATION_META);
    let read_meta = || -> TranslationMeta {
        fs::read(&meta_path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    };
    if read_meta().catalog_version == version {
        return;
    }
    let mut last_failure = CATALOG_DOWNLOAD.lock().unwrap_or_else(|p| p.into_inner());
    if skip_after_failure(last_failure.as_ref(), version, mode) {
        return;
    }
    let file = config_dir.join(CATALOG_FILE);
    let result = download_translation(CATALOG_URL, mode != Fetch::Watch).and_then(|raw| {
        parse_catalog(&raw)?;
        crate::store::write_bytes(&file, &raw).map_err(anyhow::Error::msg)?;
        let meta = TranslationMeta {
            catalog_version: version.to_string(),
            ..read_meta()
        };
        crate::store::write_bytes(&meta_path, &serde_json::to_vec(&meta)?)
            .map_err(anyhow::Error::msg)?;
        Ok(())
    });
    if let Err(exc) = &result {
        tracing::info!(error = %format!("{exc:#}"), "effort menu translation not downloaded");
    }
    *last_failure = result.is_err().then(|| Failure {
        version: version.to_string(),
        at: std::time::Instant::now(),
        hidden: mode == Fetch::Watch,
    });
}

/// Перевод меню с claude.ai: JSON-объект из непустых строк, не больше 1 МБ и 10 000 ключей.
fn parse_catalog(raw: &[u8]) -> Result<()> {
    ensure!(
        raw.len() <= 1024 * 1024,
        "{}",
        tr!(
            "перевод меню больше 1 МБ",
            "menu translation is larger than 1 MB"
        )
    );
    let map: serde_json::Map<String, Value> = serde_json::from_slice(raw).with_context(|| {
        tr!(
            "перевод меню — не JSON-объект",
            "menu translation is not a JSON object"
        )
    })?;
    ensure!(
        !map.is_empty()
            && map.len() <= 10_000
            && map
                .values()
                .all(|v| v.as_str().is_some_and(|s| !s.is_empty())),
        "{}",
        tr!(
            "перевод меню не похож на файл языка",
            "menu translation does not look like a language file"
        )
    );
    Ok(())
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
        && let Err(exc) = refresh_translation(config_dir, &app, Fetch::Launch)
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
    check.version = copy_version(&app).trim_start_matches("app-").to_string();

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

    match ruui::host_cowork_available(&app) {
        Ok(()) => check.push(
            true,
            tr!(
                "Патч Cowork (host-loop) ложится на эту версию",
                "Host-loop Cowork patch applies to this version"
            ),
        ),
        Err(exc) => check.push(
            false,
            tr!(
                "Патч Cowork (host-loop) не ложится: {exc:#}",
                "Host-loop Cowork patch does not apply: {exc:#}"
            ),
        ),
    }

    if config.russian_ui {
        match refresh_translation(config_dir, &app, Fetch::Check) {
            Ok(line) => check.push(true, line),
            Err(exc) => check.push(false, format!("{exc:#}")),
        }
    }
    // Copy first: the Code tab line below reads the copy's app.asar, which a stale copy lacks.
    // Только из успокоившейся папки версии: установщик ещё может её распаковывать. Без
    // русского — копия без перевода: ради настоящей AppData\Local у процессов Claude.
    let copy = (build_copy && settled(&app, SETTLE)).then(|| {
        copy_translation(config, config_dir).and_then(|(ru, catalog)| {
            ruui::ensure_copy(&app, &crate::claude::copies_root()?, &ru, &catalog)
        })
    });
    if config.russian_ui {
        match ruui::locale_changes(&app.join("resources").join("ion-dist.asar")) {
            Ok(patches) => {
                check.push(
                    true,
                    tr!(
                        "Русский язык встраивается в эту версию",
                        "Russian can be added to this version"
                    ),
                );
                let env = crate::claude::copies_root()
                    .is_ok_and(|root| ruui::env_prelude_ok(&app, &root));
                check.push_optional(
                    patches.folderless && env,
                    code_tab_line(patches.folderless, env),
                );
            }
            Err(exc) => check.push(
                false,
                tr!("Русский интерфейс: {exc:#}", "Russian interface: {exc:#}"),
            ),
        }
    }
    if let Some(copy) = copy {
        match copy {
            Ok(_) => {
                check.push(
                    true,
                    if config.russian_ui {
                        tr!("Русская копия готова", "Russian copy is ready")
                    } else {
                        tr!(
                            "Копия Claude готова: его процессы видят настоящую AppData\\Local",
                            "Claude copy is ready: its processes see the real AppData\\Local"
                        )
                    },
                );
                // Копия лежит вне папки установщика, и её Claude пишет «App is not
                // installed, not enabling auto-updates»: новые версии ставит только обычный.
                check.push_optional(
                    true,
                    tr!(
                        "Копия Claude сама не обновляется: новые версии ставит обычный Claude — открывайте его иногда",
                        "The copy does not update Claude: new versions come from the regular Claude, open it now and then"
                    ),
                );
            }
            Err(exc) => check.push(false, tr!("Копия Claude: {exc:#}", "Claude copy: {exc:#}")),
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

/// Как часто фон смотрит, не обновился ли Claude.
const WATCH_EVERY: Duration = Duration::from_secs(10 * 60);
/// Установщик Claude раскладывает новую версию не мгновенно: берём её папку, только если
/// ни она, ни ключевые файлы не менялись столько времени (не собирать из недописанного).
pub(crate) const SETTLE: Duration = Duration::from_secs(2 * 60);

/// Claude обновился, пока программа в трее, — русская копия готовится заранее: сначала
/// свежий перевод, потом одна сборка. К «Открыть Claude» всё готово, и копия не
/// собирается дважды (со старым переводом при запуске и с новым — следующим запуском).
/// Правила моделей фон не трогает: их применяет «Открыть Claude» вместе с перезапуском
/// шлюза, иначе шлюз остался бы со старыми именами моделей, а Claude получил бы новые.
pub fn spawn_copy_watch(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || {
        // Первый круг — через WATCH_EVERY: при старте копию уже готовит check_into.
        let mut ready: Option<String> = None;
        let mut build_failed: Option<(String, std::time::Instant)> = None;
        loop {
            std::thread::sleep(WATCH_EVERY);
            let (config, dir) = {
                let shared = app.state::<crate::state::AppShared>();
                let config = shared
                    .config
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                (config, shared.config_dir.clone())
            };
            let copies = match crate::claude::copies_root() {
                Ok(copies) => copies,
                Err(exc) => {
                    tracing::info!(error = %format!("{exc:#}"), "no folder for the Russian copy");
                    continue;
                }
            };
            match prepare_copy(&config, &dir, &copies, &mut build_failed) {
                Ok(Some(version)) => {
                    if ready.as_deref() != Some(version.as_str()) {
                        tracing::info!(%version, "Russian copy is ready");
                    }
                    ready = Some(version);
                }
                Ok(None) => {}
                Err(exc) => {
                    tracing::info!(error = %format!("{exc:#}"), "Russian copy not prepared yet")
                }
            }
        }
    });
}

/// После неудачной сборки (новый Claude устроен иначе) фон не пробует её каждые 10 минут:
/// одна попытка — до ~700 МБ записи на диск.
const BUILD_RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// Один шаг фоновой подготовки. `Ok(Some(версия))` — копия этой версии собрана ровно с
/// нынешним переводом (без русского — без перевода); `Ok(None)` — делать нечего или рано:
/// Claude ещё ставится, перевода под версию нет, копия открыта или сборка недавно не удалась.
/// Проверка дешёвая (метка и хеш файла перевода), поэтому идёт на каждом круге: если
/// перевод потом обновится, копия пересоберётся тоже в фоне, а не на клике.
fn prepare_copy(
    config: &Config,
    config_dir: &Path,
    copies: &Path,
    build_failed: &mut Option<(String, std::time::Instant)>,
) -> Result<Option<String>> {
    let Some(app) = crate::claude::detect(config).as_deref().and_then(app_dir) else {
        return Ok(None);
    };
    let version = copy_version(&app);
    if !settled(&app, SETTLE) {
        return Ok(None);
    }
    if config.russian_ui {
        if let Err(exc) = refresh_translation(config_dir, &app, Fetch::Watch) {
            tracing::info!(error = %format!("{exc:#}"), "background translation download");
        }
        // Только со свежим под эту версию переводом: со старым копия пересобралась бы ещё раз.
        if !translation_is_for(config_dir, &version) {
            return Ok(None);
        }
    }
    // Пауза после неудачной сборки — раньше пробы «открыта ли копия» (переименование
    // папки): незачем дёргать её каждые 10 минут, если собирать всё равно не будем.
    if build_failed
        .as_ref()
        .is_some_and(|(v, at)| *v == version && at.elapsed() < BUILD_RETRY_AFTER)
    {
        return Ok(None);
    }
    let (ru, catalog) = copy_translation(config, config_dir)?;
    match ruui::copy_state(&app, copies, &ru, &catalog)? {
        ruui::CopyState::Current => {
            // Копия, открытая во время прошлой сборки, осталась рядом (до 700 МБ): убираем,
            // а не ждём следующей версии Claude. Занятую Windows не отдаст — попробуем позже.
            ruui::remove_leftovers(copies, &version);
            return Ok(Some(version));
        }
        // Открытую копию Windows не даст заменить: соберём после закрытия.
        ruui::CopyState::Open => return Ok(None),
        ruui::CopyState::Stale => {}
    }
    if let Err(exc) = ruui::ensure_copy(&app, copies, &ru, &catalog) {
        *build_failed = Some((version, std::time::Instant::now()));
        return Err(exc);
    }
    *build_failed = None;
    // ensure_copy возвращает Ok и для открытой копии со старым переводом — сверяемся.
    Ok(
        (ruui::copy_state(&app, copies, &ru, &catalog)? == ruui::CopyState::Current)
            .then_some(version),
    )
}

/// Перевод скачан (или сверен) под эту версию Claude.
/// Перевод для копии: с русским — файл перевода и скачанный перевод меню (нет — пусто),
/// без него — пусто (копия без перевода, ради настоящей AppData\Local у процессов Claude).
pub(crate) fn copy_translation(config: &Config, config_dir: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    if !config.russian_ui {
        return Ok(Default::default());
    }
    let file = config_dir.join(ruui::TRANSLATION_FILE);
    let main = fs::read(&file).with_context(|| {
        let file = file.display();
        tr!("нет файла перевода {file}", "no translation file {file}")
    })?;
    let catalog = fs::read(config_dir.join(CATALOG_FILE)).unwrap_or_default();
    Ok((main, catalog))
}

fn translation_is_for(config_dir: &Path, version: &str) -> bool {
    fs::read(config_dir.join(TRANSLATION_META))
        .ok()
        .and_then(|raw| serde_json::from_slice::<TranslationMeta>(&raw).ok())
        .is_some_and(|meta| meta.claude_version == version)
}

/// Папка версии и её главные файлы не менялись `quiet`: установщик Claude закончил.
pub(crate) fn settled(app: &Path, quiet: Duration) -> bool {
    let resources = app.join("resources");
    [
        app.to_path_buf(),
        app.join("claude.exe"),
        resources.join("app.asar"),
        resources.join("ion-dist.asar"),
    ]
    .iter()
    .all(|path| {
        fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age >= quiet)
    })
}

/// Перевода нет и скачать не вышло: как положить файл вручную (claude.ai бывает закрыт
/// проверкой на бота, которую не прошёл даже встроенный браузер, или нет сети).
fn not_downloaded(file: &Path) -> String {
    let target = file.display();
    tr!(
        "перевод не скачан — откройте {TRANSLATION_URL} в браузере и сохраните как {target}",
        "translation not downloaded; open {TRANSLATION_URL} in a browser and save it as {target}"
    )
}

/// Строка самопроверки про «Без папки» во вкладке Code русской копии.
/// Одна строка самопроверки про вкладку Code русской копии: «Без папки» и обычная
/// AppData\Local у процессов сессий (`ruui::ENV_PRELUDE`). Одна, а не две: правая колонка
/// настроек должна помещаться без прокрутки.
fn code_tab_line(folderless: bool, env: bool) -> String {
    match (folderless, env) {
        (true, true) => tr!(
            "Вкладка Code: «Без папки», обычная AppData\\Local",
            "Code tab: “No folder”, usual AppData\\Local"
        ),
        (true, false) => tr!(
            "Вкладка Code: «Без папки»; AppData\\Local сессий — от копии",
            "Code tab: “No folder”; sessions see the copy's AppData\\Local"
        ),
        (false, true) => tr!(
            "Вкладка Code: «Без папки» в этой версии не найден",
            "Code tab: “No folder” not found in this version"
        ),
        (false, false) => tr!(
            "Вкладка Code: нет «Без папки», AppData\\Local сессий — от копии",
            "Code tab: no “No folder”; sessions see the copy's AppData\\Local"
        ),
    }
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("agents").join(format!(
            "ctp-upkeep-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_claude_folder_is_not_settled() {
        let app = temp_dir("settle").join("app-9.9.9");
        fs::create_dir_all(app.join("resources")).unwrap();
        for file in [
            "claude.exe",
            "resources/app.asar",
            "resources/ion-dist.asar",
        ] {
            fs::write(app.join(file), b"x").unwrap();
        }
        assert!(!settled(&app, Duration::from_secs(60)));
        assert!(settled(&app, Duration::ZERO));
        fs::remove_file(app.join("resources/app.asar")).unwrap();
        assert!(
            !settled(&app, Duration::ZERO),
            "a missing file is not settled"
        );
        fs::remove_dir_all(app.parent().unwrap()).unwrap();
    }

    #[test]
    fn copy_waits_for_a_translation_of_this_version() {
        let dir = temp_dir("meta");
        assert!(!translation_is_for(&dir, "app-6.1.0"));
        fs::write(
            dir.join(TRANSLATION_META),
            br#"{"claudeVersion":"app-6.0.0"}"#,
        )
        .unwrap();
        assert!(!translation_is_for(&dir, "app-6.1.0"));
        fs::write(
            dir.join(TRANSLATION_META),
            br#"{"claudeVersion":"app-6.1.0"}"#,
        )
        .unwrap();
        assert!(translation_is_for(&dir, "app-6.1.0"));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Фоновая подготовка на настоящем Claude и папке настроек (перевод под версию уже
    /// есть — копия не пересобирается, если готова):
    /// `CTP_CONFIG_DIR=%APPDATA%\app.provider-gateway-for-claude cargo test -p ctp-app prepare_copy_real -- --ignored`
    #[test]
    #[ignore]
    fn prepare_copy_real_install() {
        let dir = PathBuf::from(std::env::var_os("CTP_CONFIG_DIR").expect("CTP_CONFIG_DIR"));
        let config = Config {
            russian_ui: true,
            ..Default::default()
        };
        let copies = crate::claude::copies_root().unwrap();
        let mut failed = None;
        let started = std::time::Instant::now();
        let version = prepare_copy(&config, &dir, &copies, &mut failed).unwrap();
        println!("{version:?} за {:?}", started.elapsed());
        assert!(version.is_some());
        // Следующий круг сверяет метку и снова «готово», ничего не собирая.
        let started = std::time::Instant::now();
        assert_eq!(
            prepare_copy(&config, &dir, &copies, &mut failed).unwrap(),
            version
        );
        println!("повторный круг за {:?}", started.elapsed());
        assert!(failed.is_none());
    }

    #[test]
    fn hidden_failure_does_not_block_a_visible_attempt() {
        let now = std::time::Instant::now();
        let hidden = Failure {
            version: "app-1".into(),
            at: now,
            hidden: true,
        };
        assert!(skip_after_failure(Some(&hidden), "app-1", Fetch::Watch));
        assert!(!skip_after_failure(Some(&hidden), "app-1", Fetch::Check));
        assert!(!skip_after_failure(Some(&hidden), "app-1", Fetch::Launch));
        let visible = Failure {
            hidden: false,
            ..hidden
        };
        assert!(skip_after_failure(Some(&visible), "app-1", Fetch::Check));
        assert!(skip_after_failure(Some(&visible), "app-1", Fetch::Watch));
        // Другая версия Claude — пауза не действует.
        assert!(!skip_after_failure(Some(&visible), "app-2", Fetch::Check));
        assert!(!skip_after_failure(None, "app-1", Fetch::Watch));
        // Паузы кончаются: видимая через 30 минут, фоновая для фона — через 3 часа.
        let ago = |d: Duration| now.checked_sub(d).unwrap();
        let old_visible = Failure {
            at: ago(RETRY_AFTER + Duration::from_secs(1)),
            ..visible
        };
        assert!(!skip_after_failure(
            Some(&old_visible),
            "app-1",
            Fetch::Check
        ));
        let hour_old_hidden = Failure {
            version: "app-1".into(),
            at: ago(Duration::from_secs(60 * 60)),
            hidden: true,
        };
        assert!(skip_after_failure(
            Some(&hour_old_hidden),
            "app-1",
            Fetch::Watch
        ));
        let old_hidden = Failure {
            at: ago(WATCH_RETRY_AFTER + Duration::from_secs(1)),
            ..hour_old_hidden
        };
        assert!(!skip_after_failure(
            Some(&old_hidden),
            "app-1",
            Fetch::Watch
        ));
    }

    #[test]
    fn menu_translation_must_look_like_a_language_file() {
        assert!(parse_catalog(r#"{"HoZQmASaw3":"Очень высокий"}"#.as_bytes()).is_ok());
        assert!(parse_catalog(b"<html>Just a moment...</html>").is_err());
        assert!(parse_catalog(b"{}").is_err());
        assert!(parse_catalog(br#"{"a":1}"#).is_err());
        assert!(parse_catalog(br#"{"a":""}"#).is_err());
        assert!(parse_catalog(&vec![b' '; 1024 * 1024 + 1]).is_err());
    }

    #[test]
    fn the_copy_has_a_translation_only_with_russian_on() {
        // Without Russian the copy is built with no translation (only the real AppData\Local
        // for Claude's processes); with it the translation file is required.
        let dir = temp_dir("copy-translation");
        let mut config = Config {
            russian_ui: false,
            ..Default::default()
        };
        assert_eq!(copy_translation(&config, &dir).unwrap(), Default::default());
        config.russian_ui = true;
        assert!(copy_translation(&config, &dir).is_err());
        fs::write(dir.join(ruui::TRANSLATION_FILE), b"{}").unwrap();
        assert_eq!(
            copy_translation(&config, &dir).unwrap(),
            (b"{}".to_vec(), Vec::new())
        );
        // The downloaded menu translation goes along, so a new one rebuilds the copy.
        fs::write(dir.join(CATALOG_FILE), br#"{"a":"b"}"#).unwrap();
        assert_eq!(copy_translation(&config, &dir).unwrap().1, br#"{"a":"b"}"#);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_403_goes_to_the_browser() {
        assert!(is_bot_check(
            "curl: (22) The requested URL returned error: 403"
        ));
        assert!(!is_bot_check(
            "curl: (22) The requested URL returned error: 404"
        ));
        assert!(!is_bot_check("curl: (6) Could not resolve host: claude.ai"));
        assert!(!is_bot_check(
            "curl: (28) Operation timed out after 30000 milliseconds"
        ));
    }

    #[test]
    fn code_tab_line_names_what_is_missing() {
        assert!(code_tab_line(true, true).contains("обычная"));
        assert!(code_tab_line(true, false).contains("от копии"));
        assert!(code_tab_line(false, true).contains("не найден"));
        assert!(code_tab_line(false, false).contains("нет «Без папки»"));
    }

    #[test]
    fn copy_version_is_the_copy_folder_for_both_layouts() {
        // Иначе фоновая подготовка искала бы копию пакета в папке "app" и удаляла готовую.
        let package =
            Path::new(r"C:\Program Files\WindowsApps\Claude_6.28899.6.0_x64__pzs8sxrjxfjjc\app");
        assert_eq!(copy_version(package), "app-6.28899.6.0");
        assert_eq!(
            copy_version(Path::new(r"C:\x\AnthropicClaude\app-7.37095.0")),
            "app-7.37095.0"
        );
    }

    #[test]
    fn app_dir_only_for_version_folders() {
        let exe = Path::new(r"C:\x\AnthropicClaude\app-6.1.0\claude.exe");
        assert_eq!(
            app_dir(exe).unwrap(),
            Path::new(r"C:\x\AnthropicClaude\app-6.1.0")
        );
        assert!(app_dir(Path::new(r"C:\x\AnthropicClaude\claude.exe")).is_none());
        // MSIX: <package>\app\claude.exe (ADR-021).
        let package = Path::new(
            r"C:\Program Files\WindowsApps\Claude_6.28899.6.0_x64__pzs8sxrjxfjjc\app\claude.exe",
        );
        assert_eq!(
            app_dir(package).unwrap(),
            Path::new(r"C:\Program Files\WindowsApps\Claude_6.28899.6.0_x64__pzs8sxrjxfjjc\app")
        );
        assert!(app_dir(Path::new(r"C:\x\SomeOther\app\claude.exe")).is_none());
        // Та же папка, что версия копии, иначе «app» вместо «app-<версия>» (дефект 9d77556).
        assert!(app_dir(Path::new(r"C:\x\Claude_old\app\claude.exe")).is_none());
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
