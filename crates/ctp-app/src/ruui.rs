//! Русский интерфейс Claude Desktop.
//!
//! Интерфейс (`ion-dist.asar`) знает 11 языков, русского среди них нет, хотя у
//! claude.ai есть официальный перевод (`/i18n/ru.json`). Мы собираем копию
//! установленной версии, в которой:
//! - `i18n/ru.json` — этот перевод (файл кладёт пользователь, в репозитории его нет);
//! - в список языков бандлов добавлен `"ru"`: без этого русская Windows получает `en-US`;
//! - хеш заголовка asar в `claude.exe` обновлён, иначе Electron не откроет архив;
//! - в выборе папки Code включён пункт «Без папки» (см. `enable_folderless`).
//!
//! Подпись копии `claude.exe` после этого недействительна. Оригинал не меняется:
//! копия лежит отдельно, файлы читаются, но не пишутся.

use crate::tr;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Файл перевода в папке настроек приложения.
pub const TRANSLATION_FILE: &str = "claude-ru.json";
/// Меняется, когда меняется сам патч: старые копии пересобираются.
const PATCH_REV: u32 = 10;
const BLOCK: usize = 4 * 1024 * 1024;
const MARKER: &str = ".ctp-ru.json";

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn align4(n: u64) -> u64 {
    n.div_ceil(4) * 4
}

struct Header {
    json: Value,
    raw: Vec<u8>,
    /// Где начинаются данные файлов; `offset` в заголовке — от этой точки.
    base: u64,
}

/// Pickle-заголовок asar: `[4][header_size][payload_size][json_len][json][выравнивание]`.
fn read_header(file: &mut File) -> Result<Header> {
    let mut head = [0u8; 16];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let word = |i: usize| {
        u64::from(u32::from_le_bytes([
            head[i],
            head[i + 1],
            head[i + 2],
            head[i + 3],
        ]))
    };
    let (four, header_size, payload_size, json_len) = (word(0), word(4), word(8), word(12));
    ensure!(
        four == 4 && header_size == payload_size + 4 && payload_size == align4(4 + json_len),
        "{}",
        tr!("неизвестный формат asar", "unknown asar format")
    );
    let mut raw = vec![0u8; json_len as usize];
    file.read_exact(&mut raw)?;
    let json = serde_json::from_slice(&raw)
        .with_context(|| tr!("заголовок asar не читается", "asar header is unreadable"))?;
    Ok(Header {
        json,
        raw,
        base: 8 + header_size,
    })
}

/// Как у `@electron/asar`: SHA-256 файла и каждого блока по 4 МиБ.
fn integrity(data: &[u8]) -> Value {
    let blocks: Vec<String> = if data.is_empty() {
        vec![sha256_hex(data)]
    } else {
        data.chunks(BLOCK).map(sha256_hex).collect()
    };
    json!({"algorithm": "SHA256", "hash": sha256_hex(data), "blockSize": BLOCK, "blocks": blocks})
}

/// Файлы `.js` под `prefix`: путь, смещение, размер.
fn js_files(node: &Value, prefix: &str, out: &mut Vec<(String, u64, u64)>) {
    let Some(files) = node.get("files").and_then(Value::as_object) else {
        return;
    };
    for (name, child) in files {
        let path = format!("{prefix}{name}");
        if child.get("files").is_some() {
            js_files(child, &format!("{path}/"), out);
        } else if name.ends_with(".js") && child.get("unpacked").is_none() {
            let offset = child
                .get("offset")
                .and_then(Value::as_str)
                .and_then(|o| o.parse().ok());
            let size = child.get("size").and_then(Value::as_u64);
            if let (Some(offset), Some(size)) = (offset, size) {
                out.push((path, offset, size));
            }
        }
    }
}

/// Запись файла по пути `a/b/c`; недостающие папки создаются.
fn entry_mut<'a>(root: &'a mut Value, path: &str) -> Result<&'a mut Value> {
    let mut node = root;
    let parts: Vec<&str> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let files = node
            .as_object_mut()
            .with_context(|| tr!("заголовок asar: не объект", "asar header: not an object"))?
            .entry("files")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .with_context(|| {
                tr!(
                    "заголовок asar: files не объект",
                    "asar header: files is not an object"
                )
            })?;
        let last = i + 1 == parts.len();
        node = files.entry(part.to_string()).or_insert_with(|| {
            if last {
                Value::Null
            } else {
                json!({"files": {}})
            }
        });
    }
    ensure!(
        node.get("unpacked").is_none(),
        "{}",
        tr!(
            "{path}: файл вне архива",
            "{path}: file is outside the archive"
        )
    );
    Ok(node)
}

/// Новый архив: старые данные как есть, новые версии файлов дописаны в конец,
/// записи заголовка указывают на них. Возвращает хеши заголовка до и после.
fn rebuild(src: &Path, dst: &Path, changes: &[(String, Vec<u8>)]) -> Result<(String, String)> {
    let mut input = File::open(src).with_context(|| src.display().to_string())?;
    let header = read_header(&mut input)?;
    let payload_len = input.metadata()?.len() - header.base;

    let mut json = header.json.clone();
    let mut cursor = payload_len;
    for (path, data) in changes {
        *entry_mut(&mut json, path)? = json!({
            "size": data.len(),
            "offset": cursor.to_string(),
            "integrity": integrity(data),
        });
        cursor += data.len() as u64;
    }
    let raw = serde_json::to_vec(&json)?;
    let json_len = raw.len() as u64;
    let payload_size = align4(4 + json_len);
    let words = [4u64, payload_size + 4, payload_size, json_len];

    let mut out = BufWriter::new(File::create(dst).with_context(|| dst.display().to_string())?);
    for word in words {
        out.write_all(&(u32::try_from(word)?).to_le_bytes())?;
    }
    out.write_all(&raw)?;
    out.write_all(&vec![0u8; (payload_size - 4 - json_len) as usize])?;
    input.seek(SeekFrom::Start(header.base))?;
    let copied = std::io::copy(&mut input, &mut out)?;
    ensure!(
        copied == payload_len,
        "{}",
        tr!("asar прочитан не полностью", "asar was not read completely")
    );
    for (_, data) in changes {
        out.write_all(data)?;
    }
    out.flush()?;
    Ok((sha256_hex(&header.raw), sha256_hex(&raw)))
}

fn replace_all(data: &[u8], old: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() + new.len());
    let mut rest = data;
    let mut found = false;
    while let Some(at) = rest.windows(old.len()).position(|w| w == old) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(new);
        rest = &rest[at + old.len()..];
        found = true;
    }
    found.then(|| {
        out.extend_from_slice(rest);
        out
    })
}

/// `ion-dist.asar` с русским: `i18n/ru.json`, `i18n/dynamic/ru.json` (см. `catalog_ru`) и
/// `"ru"` в списке языков бандлов. `catalog` — скачанный с claude.ai `dynamic/ru.json`,
/// пусто — его нет.
pub fn patch_ion_dist(
    src: &Path,
    dst: &Path,
    ru_json: &[u8],
    catalog: &[u8],
) -> Result<(String, String)> {
    let map: serde_json::Map<String, Value> =
        serde_json::from_slice(ru_json).with_context(|| {
            tr!(
                "файл перевода — не JSON-объект",
                "translation file is not a JSON object"
            )
        })?;
    let count = map.len();
    ensure!(
        count > 1000,
        "{}",
        tr!(
            "в файле перевода слишком мало строк ({count})",
            "translation file has too few strings ({count})"
        )
    );

    let mut changes = locale_changes(src)?.files;
    let menu = catalog_ru(src, &map, catalog);
    tracing::info!(
        translated = menu.translated,
        total = menu.total,
        "effort menu strings"
    );
    changes.push(("i18n/ru.json".to_string(), ru_json.to_vec()));
    changes.push(("i18n/dynamic/ru.json".to_string(), menu.json));
    rebuild(src, dst, &changes)
}

/// Запасной перевод подписей каталога моделей (уровни усилий, их описание, режимы
/// размышлений): ключи — как в `dynamic/en-US.json` Claude 6.28899.
const CATALOG_RU: &str = include_str!("ru-catalog.json");

/// Русский `i18n/dynamic/ru.json` для версии Claude и сколько строк в нём из скольких.
pub struct Catalog {
    pub json: Vec<u8>,
    pub translated: usize,
    pub total: usize,
}

/// Подписи каталога моделей Claude переводит вторым файлом языка, `i18n/dynamic/<язык>.json`
/// (ADR-009). Он собирается под ключи `dynamic/en-US.json` этой версии: для каждого — первый
/// найденный из скачанного с claude.ai (`official`), вшитого `CATALOG_RU` и основного перевода
/// строки `en-US.json` с тем же английским текстом. Сначала источники по ключу: короткие
/// строки («High») по тексту могут совпасть с чужими. Не нашлось — ключа нет, Claude покажет
/// его по-английски. Нет `dynamic/en-US.json` (версия устроена иначе) — скачанный поверх вшитого.
pub fn catalog_ru(src: &Path, ru: &serde_json::Map<String, Value>, official: &[u8]) -> Catalog {
    type Map = serde_json::Map<String, Value>;
    let parse = |raw: &[u8]| serde_json::from_slice::<Map>(raw).unwrap_or_default();
    let read = |path: &str| read_asar_file(src, path).ok().map(|raw| parse(&raw));
    let official = parse(official);
    let bundled = parse(CATALOG_RU.as_bytes());
    let mut out = Map::new();
    let total = match read("i18n/dynamic/en-US.json") {
        Some(english) => {
            // Английский текст основного файла → его ключ (первый из одинаковых).
            let mut by_text: std::collections::HashMap<String, String> = Default::default();
            for (key, text) in read("i18n/en-US.json").unwrap_or_default() {
                if let Value::String(text) = text {
                    by_text.entry(text).or_insert(key);
                }
            }
            for (key, text) in &english {
                let by_english = text
                    .as_str()
                    .and_then(|text| by_text.get(text))
                    .and_then(|id| ru.get(id));
                let found = [official.get(key), bundled.get(key), by_english]
                    .into_iter()
                    .flatten()
                    .find(|value| value.as_str().is_some_and(|s| !s.is_empty()));
                if let Some(value) = found {
                    out.insert(key.clone(), value.clone());
                }
            }
            english.len()
        }
        None => {
            out = bundled;
            out.extend(official.into_iter().filter(|(_, v)| v.is_string()));
            out.len()
        }
    };
    Catalog {
        translated: out.len(),
        total,
        json: serde_json::to_vec(&out).unwrap_or_else(|_| b"{}".to_vec()),
    }
}

/// Изменённые бандлы интерфейса для копии.
pub struct BundlePatches {
    pub files: Vec<(String, Vec<u8>)>,
    /// Флаг «Без папки» найден и включён; `false` — в этой версии Claude его нет.
    pub folderless: bool,
}

/// Бандлы интерфейса с `"ru"` в списке языков (и включённым «Без папки») — без записи
/// на диск: годится и для самопроверки новой версии Claude. Нет списка языков — версия
/// устроена иначе: отказ. Нет флага «Без папки» — не отказ, копия собирается без него.
pub fn locale_changes(src: &Path) -> Result<BundlePatches> {
    let mut input = File::open(src).with_context(|| src.display().to_string())?;
    let header = read_header(&mut input)?;
    let locales = bundled_locales(&header.json);
    ensure!(
        locales.len() >= 3 && locales.contains("en-US"),
        "{}",
        tr!(
            "в интерфейсе этой версии Claude не найдены файлы языков (i18n/*.json)",
            "no language files (i18n/*.json) found in this Claude version's interface"
        )
    );
    let mut files = Vec::new();
    js_files(&header.json, "", &mut files);
    let mut changes = Vec::new();
    let mut ru_added = false;
    let mut folderless_found = false;
    for (path, offset, size) in files.into_iter().filter(|(p, ..)| p.starts_with("assets/")) {
        let mut data = vec![0u8; size as usize];
        input.seek(SeekFrom::Start(header.base + offset))?;
        input.read_exact(&mut data)?;
        let with_ru = add_ru(&data, &locales);
        ru_added |= with_ru.is_some();
        let folderless = enable_folderless(with_ru.as_deref().unwrap_or(&data));
        folderless_found |= folderless.is_some();
        if let Some(patched) = folderless.or(with_ru) {
            changes.push((path, patched));
        }
    }
    ensure!(
        ru_added,
        "{}",
        tr!(
            "в интерфейсе этой версии Claude не найден список языков",
            "no language list found in this Claude version's interface"
        )
    );
    Ok(BundlePatches {
        files: changes,
        folderless: folderless_found,
    })
}

/// Языки, переводы которых лежат в `i18n/` архива (`en-US`, `de-DE`, …), кроме `ru`.
fn bundled_locales(header: &Value) -> std::collections::HashSet<String> {
    header
        .pointer("/files/i18n/files")
        .and_then(Value::as_object)
        .map(|files| {
            files
                .keys()
                .filter_map(|name| name.strip_suffix(".json"))
                .filter(|locale| *locale != "ru")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Дописывает `"ru"` в каждый JSON-массив, составленный ровно из `locales` (порядок в
/// коде свой — ищем по составу, а не по строке: новая версия Claude может переставить
/// языки или добавить новый). Вывод для прежнего списка байт в байт как раньше.
fn add_ru(data: &[u8], locales: &std::collections::HashSet<String>) -> Option<Vec<u8>> {
    static ARRAY: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let array = ARRAY.get_or_init(|| {
        regex::bytes::Regex::new(r#"\["[A-Za-z0-9-]{2,12}"(?:,"[A-Za-z0-9-]{2,12}")+\]"#)
            .expect("locale array")
    });
    let mut out = Vec::new();
    let mut last = 0;
    for found in array.find_iter(data) {
        let Ok(items) = serde_json::from_slice::<Vec<String>>(found.as_bytes()) else {
            continue;
        };
        let set: std::collections::HashSet<String> = items.iter().cloned().collect();
        if set.len() != items.len() || set != *locales {
            continue;
        }
        out.extend_from_slice(&data[last..found.end() - 1]);
        out.extend_from_slice(br#","ru"]"#);
        last = found.end();
    }
    (last > 0).then(|| {
        out.extend_from_slice(&data[last..]);
        out
    })
}

/// «Без папки» в выборе папки Code: в режиме стороннего провайдера Claude берёт флаги из
/// вшитого набора, и `claudeai_cc_epitaxy_folderless_start_main` там нет, хотя основной
/// процесс временную папку создать умеет. Вызов проверки флага заменяется на `!0`.
/// Нет такой строки (новая версия Claude) — `None`, копия собирается без этого.
fn enable_folderless(data: &[u8]) -> Option<Vec<u8>> {
    static GATE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let gate = GATE.get_or_init(|| {
        // Без `.` перед именем: `obj.fn("…")` заменять нельзя, останется `obj.!0`.
        regex::bytes::Regex::new(
            r#"(^|[^.\w$])[A-Za-z_$][\w$]*\("claudeai_cc_epitaxy_folderless_start_main"\)"#,
        )
        .expect("folderless gate")
    });
    gate.is_match(data)
        .then(|| gate.replace_all(data, &b"${1}!0"[..]).into_owned())
}

/// Переменная, через которую запуск копии (`claude::launch`) сообщает настоящую папку
/// AppData\Local. Сам Claude работает с подменённой `LOCALAPPDATA`: на ней держится его
/// отдельный профиль 3p (корень `%LOCALAPPDATA%\Claude-3p`; `CLAUDE_USER_DATA_DIR`
/// подписанные сборки не слушают) и защита установленного Claude от действий копии.
pub const REAL_LOCALAPPDATA_VAR: &str = "CTP_REAL_LOCALAPPDATA";

/// Вставка в начало главного процесса копии: процессы, которые Claude запускает через
/// `child_process` (сессии Code с их Bash и PowerShell, их MCP-серверы, внешние
/// MCP-команды, git), получают настоящую `LOCALAPPDATA`, как у обычного Claude, а сам
/// Claude по-прежнему видит подменённую. Подменённое значение в окружении ребёнка
/// меняется на настоящее, заданное иначе — не трогается; `CTP_REAL_LOCALAPPDATA` детям
/// не передаётся, а у самого Claude остаётся (его `app.relaunch` сохранит правку).
/// `undefined`/`null` на месте опций заменяются, а не сдвигают колбэк. `util.promisify`
/// у `exec`/`execFile` ведёт себя как прежде ({stdout, stderr}).
/// shortcut: процессы `utilityProcess` (MCP на встроенном Node, терминал, хост MCP
/// режима 3p) не тронуты — некоторые из них сами ищут данные профиля через
/// `LOCALAPPDATA`; расширить, если это начнёт мешать на деле.
const ENV_PRELUDE: &str = r#";(()=>{try{const real=process.env.CTP_REAL_LOCALAPPDATA,over=process.env.LOCALAPPDATA;if(!real||!over||real.toLowerCase()===over.toLowerCase())return;const cp=require("child_process"),util=require("util");const fix=(env)=>{const e=Object.assign({},env||process.env);for(const k of Object.keys(e)){const u=k.toUpperCase();if(u==="CTP_REAL_LOCALAPPDATA")delete e[k];else if(u==="LOCALAPPDATA"&&typeof e[k]==="string"&&e[k].toLowerCase()===over.toLowerCase())e[k]=real}return e};const fixArgs=(name,a)=>{a=Array.prototype.slice.call(a);if(name!=="exec"&&name!=="execSync"&&a.length>1&&(a[1]===undefined||a[1]===null))a[1]=[];const i=a.findIndex((x,j)=>j>0&&x!==null&&typeof x==="object"&&!Array.isArray(x));if(i>0){a[i]=Object.assign({},a[i],{env:fix(a[i].env)})}else{const at=(name==="exec"||name==="execSync")?1:(Array.isArray(a[1])?2:1);if(at<a.length&&(a[at]===undefined||a[at]===null))a[at]={env:fix()};else a.splice(Math.min(at,a.length),0,{env:fix()})}return a};for(const name of ["spawn","spawnSync","execFile","execFileSync","exec","execSync","fork"]){const orig=cp[name];if(typeof orig!=="function")continue;const wrapped=function(){return orig.apply(this,fixArgs(name,arguments))};const custom=orig[util.promisify.custom];if(typeof custom==="function"){wrapped[util.promisify.custom]=function(){return custom.apply(this,fixArgs(name,arguments))}}Object.defineProperty(wrapped,"name",{value:name});cp[name]=wrapped}}catch(e){}})();"#;

/// Для самопроверки: есть ли вставка окружения в уже собранной копии этой версии; копии
/// ещё нет — можно ли её встроить (точка входа главного процесса читается).
pub fn env_prelude_ok(app_dir: &Path, cache_root: &Path) -> bool {
    let version = app_dir.file_name().unwrap_or_default();
    let copy_asar = cache_root.join(version).join("resources").join("app.asar");
    if copy_asar.is_file() {
        let entry = read_asar_file(&copy_asar, "package.json")
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|pkg| pkg.get("main").and_then(Value::as_str).map(str::to_string))
            .and_then(|main| read_asar_file(&copy_asar, main.trim_start_matches("./")).ok());
        return entry.is_some_and(|code| {
            code.windows(REAL_LOCALAPPDATA_VAR.len())
                .any(|w| w == REAL_LOCALAPPDATA_VAR.as_bytes())
        });
    }
    env_prelude_change(&app_dir.join("resources").join("app.asar")).is_ok()
}

/// Точка входа главного процесса из `package.json` архива `app.asar` и она же со
/// вставкой `ENV_PRELUDE` (после `"use strict";`, если файл с него начинается).
fn env_prelude_change(app_asar: &Path) -> Result<(String, Vec<u8>)> {
    let package: Value = serde_json::from_slice(&read_asar_file(app_asar, "package.json")?)?;
    let main = package
        .get("main")
        .and_then(Value::as_str)
        .map(|m| m.trim_start_matches("./").to_string())
        .with_context(|| tr!("в package.json нет main", "package.json has no main"))?;
    let entry = read_asar_file(app_asar, &main)?;
    let strict = br#""use strict";"#;
    let mut patched = Vec::with_capacity(entry.len() + ENV_PRELUDE.len());
    if entry.starts_with(strict) {
        patched.extend_from_slice(strict);
        patched.extend_from_slice(ENV_PRELUDE.as_bytes());
        patched.extend_from_slice(&entry[strict.len()..]);
    } else {
        patched.extend_from_slice(ENV_PRELUDE.as_bytes());
        patched.extend_from_slice(&entry);
    }
    Ok((main, patched))
}

/// Подменяет хеш заголовка `ion-dist.asar` в ресурсе целостности exe (той же длины).
pub fn patch_exe(exe: &[u8], old_hash: &str, new_hash: &str) -> Result<Vec<u8>> {
    let old = old_hash.as_bytes();
    let count = exe.windows(old.len()).filter(|w| *w == old).count();
    ensure!(
        count == 1,
        "{}",
        tr!(
            "хеш ion-dist.asar найден в claude.exe {count} раз(а), ожидался 1",
            "ion-dist.asar hash found in claude.exe {count} time(s), expected 1"
        )
    );
    replace_all(exe, old, new_hash.as_bytes())
        .with_context(|| tr!("хеш не заменён", "hash not replaced"))
}

fn stamp(path: &Path) -> Result<(u64, u64)> {
    let meta = fs::metadata(path).with_context(|| path.display().to_string())?;
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), modified))
}

fn copy_tree(src: &Path, dst: &Path, skip: &[PathBuf]) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        if skip.contains(&from) {
            continue;
        }
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to, skip)?;
        } else {
            // Обычное копирование: никаких жёстких ссылок на файлы оригинала.
            fs::copy(&from, &to).with_context(|| from.display().to_string())?;
        }
    }
    Ok(())
}

/// Готовая русская копия версии `app_dir` в `cache_root`; собирается, если её нет
/// или изменились исходники, перевод или патч. Возвращает путь к её `claude.exe`.
pub fn ensure_copy(
    app_dir: &Path,
    cache_root: &Path,
    ru_json: &[u8],
    catalog: &[u8],
) -> Result<PathBuf> {
    let (version, marker) = copy_marker(app_dir, ru_json, catalog)?;
    let target = cache_root.join(&version);
    let ready = target.join("claude.exe");
    // Две сборки разом удаляли бы файлы друг у друга. Вторая ждёт первую (фоновую
    // сборку после обновления Claude) и берёт её результат, а не открывает английский.
    // Паника прошлой сборки не должна навсегда запрещать новые.
    let _build = BUILD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if fs::read_to_string(target.join(MARKER)).is_ok_and(|m| m == marker) && ready.is_file() {
        return Ok(ready);
    }
    // Открытую копию Windows не даст заменить. Если сменился только перевод — она
    // годится как есть, и собирать 700 МБ впустую незачем (до сборки, а не после).
    if ready.is_file()
        && only_translation_differs(&target.join(MARKER), &marker)
        && is_open(&target, cache_root)
    {
        tracing::info!("translation updated but the Russian copy is open, launching it");
        return Ok(ready);
    }
    fs::create_dir_all(cache_root)?;
    remove_leftovers(cache_root, &version);

    let building = cache_root.join(format!("{version}.building"));
    if let Err(exc) = build(app_dir, &building, ru_json, catalog, &marker) {
        let _ = fs::remove_dir_all(&building);
        return Err(exc);
    }
    if target.exists() {
        // Запущенную копию Windows переименовать не даст: тогда копия остаётся целой.
        if let Err(exc) = fs::rename(&target, cache_root.join(trash_name(&version))) {
            let _ = fs::remove_dir_all(&building);
            // Та же версия Claude, сменился только перевод: открытая копия годится,
            // новый перевод она получит при следующей сборке.
            if only_translation_differs(&target.join(MARKER), &marker) && ready.is_file() {
                tracing::info!("translation updated but the Russian copy is open, launching it");
                return Ok(ready);
            }
            anyhow::bail!(
                "{}",
                tr!(
                    "русская копия Claude сейчас открыта — закройте её и запустите снова ({exc})",
                    "the Russian copy of Claude is open; close it and launch again ({exc})"
                )
            );
        }
    }
    // Свежие 700 МБ ещё проверяет антивирус, и переименование на миг отказывает (os error 5):
    // повторяем, иначе копии не будет до следующей сборки через часы.
    let mut attempt = 0;
    while let Err(exc) = fs::rename(&building, &target) {
        attempt += 1;
        if attempt >= 10 {
            return Err(anyhow::Error::new(exc).context(format!(
                "{} -> {}",
                building.display(),
                target.display()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    remove_leftovers(cache_root, &version);
    Ok(ready)
}

static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Версия (`app-…`) и метка копии: исходники, патч и перевод. Совпала метка — копия годна.
fn copy_marker(app_dir: &Path, ru_json: &[u8], catalog: &[u8]) -> Result<(String, String)> {
    let version = app_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.starts_with("app-"))
        .with_context(|| {
            tr!(
                "Claude найден не в папке версии app-…",
                "Claude is not in an app-… version folder"
            )
        })?
        .to_string();
    let marker = json!({
        "rev": PATCH_REV,
        "exe": stamp(&app_dir.join("claude.exe"))?,
        "asar": stamp(&app_dir.join("resources").join("ion-dist.asar"))?,
        // Скачанный перевод меню — в той же метке перевода: открытая копия при его смене
        // ждёт закрытия, как при смене основного (`only_translation_differs`).
        "ru": if catalog.is_empty() {
            sha256_hex(ru_json)
        } else {
            format!("{}+{}", sha256_hex(ru_json), sha256_hex(catalog))
        },
    })
    .to_string();
    Ok((version, marker))
}

/// Что с копией этой версии при этом переводе — без сборки.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CopyState {
    /// Собрана ровно из этих исходников с этим переводом.
    Current,
    /// Устарела, но открыта: Windows не даст её заменить, ждём закрытия.
    Open,
    /// Нет или устарела — можно собирать.
    Stale,
}

/// Состояние копии; под `BUILD`, чтобы не смотреть на копию посреди чужой сборки.
pub fn copy_state(
    app_dir: &Path,
    cache_root: &Path,
    ru_json: &[u8],
    catalog: &[u8],
) -> Result<CopyState> {
    let (version, marker) = copy_marker(app_dir, ru_json, catalog)?;
    let target = cache_root.join(&version);
    let ready = target.join("claude.exe");
    let _build = BUILD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(
        if fs::read_to_string(target.join(MARKER)).is_ok_and(|m| m == marker) && ready.is_file() {
            CopyState::Current
        } else if ready.is_file() && is_open(&target, cache_root) {
            CopyState::Open
        } else {
            CopyState::Stale
        },
    )
}

/// Запущена ли копия: Windows не даёт переименовать папку с открытым exe. Проба —
/// переименование туда и сразу обратно.
fn is_open(target: &Path, cache_root: &Path) -> bool {
    // Имя не на `app-`: уборка `remove_leftovers` такую папку не тронет.
    let probe = cache_root.join(format!(
        "probe-{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("copy")
    ));
    // Осиротевшая проба (сбой между двумя rename) иначе навсегда давала бы «открыта».
    if probe.exists() {
        let _ = fs::remove_dir_all(&probe);
    }
    match fs::rename(target, &probe) {
        Ok(()) => {
            if let Err(exc) = fs::rename(&probe, target) {
                tracing::error!(error = %exc, "Russian copy was not renamed back after the probe");
            }
            false
        }
        Err(_) => true,
    }
}

/// Метка копии совпадает с новой во всём, кроме хеша перевода.
fn only_translation_differs(old_marker: &Path, new_marker: &str) -> bool {
    let strip = |text: &str| {
        serde_json::from_str::<Value>(text).ok().map(|mut v| {
            if let Some(map) = v.as_object_mut() {
                map.remove("ru");
            }
            v
        })
    };
    let old = fs::read_to_string(old_marker).ok();
    matches!((old.as_deref().and_then(strip), strip(new_marker)), (Some(a), Some(b)) if a == b)
}

/// Файл `path` (`i18n/en-US.json`) из asar без распаковки остального.
pub fn read_asar_file(asar: &Path, path: &str) -> Result<Vec<u8>> {
    let mut file = File::open(asar).with_context(|| asar.display().to_string())?;
    let header = read_header(&mut file)?;
    let mut node = &header.json;
    for part in path.split('/') {
        node = node
            .get("files")
            .and_then(|files| files.get(part))
            .with_context(|| {
                let asar = asar.display();
                tr!("в {asar} нет {path}", "{asar} has no {path}")
            })?;
    }
    let offset: u64 = node
        .get("offset")
        .and_then(Value::as_str)
        .and_then(|o| o.parse().ok())
        .with_context(|| tr!("у файла нет смещения", "file has no offset"))?;
    let size = node
        .get("size")
        .and_then(Value::as_u64)
        .with_context(|| tr!("у файла нет размера", "file has no size"))?;
    ensure!(
        size <= 256 * 1024 * 1024,
        "{}",
        tr!("{path}: слишком большой", "{path}: too large")
    );
    let mut data = vec![0u8; size as usize];
    file.seek(SeekFrom::Start(header.base + offset))?;
    file.read_exact(&mut data)?;
    Ok(data)
}

fn trash_name(version: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{version}.trash-{stamp}")
}

/// Сначала патч (если версия не подходит — отказ до копирования 700 МБ),
/// потом копия остальных файлов, метка последней. Пустой `ru_json` — копия без перевода
/// (английский интерфейс): в ней только настоящая AppData\Local для процессов Claude.
fn build(
    app_dir: &Path,
    building: &Path,
    ru_json: &[u8],
    catalog: &[u8],
    marker: &str,
) -> Result<()> {
    if building.exists() {
        fs::remove_dir_all(building)?;
    }
    let src_exe = app_dir.join("claude.exe");
    let src_asar = app_dir.join("resources").join("ion-dist.asar");
    fs::create_dir_all(building.join("resources"))?;
    let mut exe = fs::read(&src_exe)?;
    let mut skip = vec![src_exe];
    if !ru_json.is_empty() {
        let (old_hash, new_hash) = patch_ion_dist(
            &src_asar,
            &building.join("resources").join("ion-dist.asar"),
            ru_json,
            catalog,
        )?;
        exe = patch_exe(&exe, &old_hash, &new_hash)?;
        skip.push(src_asar);
    }
    // Настоящая AppData\Local для процессов копии. Не вышло (Claude устроен иначе) —
    // русская копия всё равно собирается, без этой правки; самопроверка это покажет.
    // Копия без перевода без неё не нужна: тогда отказ, и запускается оригинал.
    let src_app = app_dir.join("resources").join("app.asar");
    let dst_app = building.join("resources").join("app.asar");
    let applied = env_prelude_change(&src_app)
        .and_then(|change| rebuild(&src_app, &dst_app, &[change]))
        .and_then(|(old_app, new_app)| patch_exe(&exe, &old_app, &new_app));
    match applied {
        Ok(patched) => {
            exe = patched;
            skip.push(src_app);
        }
        Err(exc) if ru_json.is_empty() => return Err(exc),
        Err(exc) => {
            // Исходный app.asar скопирует copy_tree ниже: копия без правки, но целая.
            let _ = fs::remove_file(&dst_app);
            tracing::warn!(error = %format!("{exc:#}"), "child environment fix not applied");
        }
    }
    fs::write(building.join("claude.exe"), exe)?;
    copy_tree(app_dir, building, &skip)?;
    fs::write(building.join(MARKER), marker)?;
    Ok(())
}

/// Недособранные папки, корзина и копии прошлых версий (~700 МБ каждая).
/// Открытую копию переименовать нельзя — её не трогаем.
pub(crate) fn remove_leftovers(cache_root: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(cache_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("app-") || name == keep || name == format!("{keep}.building") {
            continue;
        }
        let path = entry.path();
        let doomed = if name.contains(".trash-") {
            path
        } else {
            let trash = cache_root.join(trash_name(&name));
            if fs::rename(&path, &trash).is_err() {
                continue;
            }
            trash
        };
        let _ = fs::remove_dir_all(doomed);
    }
}

/// Значение реестра, указывающее в папку копий, — путь установленного Claude:
/// `<copies>\app-X\claude.exe` → `<fallback_app>\claude.exe` (Claude, которого нашёл
/// шлюз, обычно самая свежая версия — её же пишет и сам установленный Claude),
/// `<copies>\Update.exe` → `<installed>\Update.exe`. Запасной путь, когда снимка
/// «до запуска» нет. `None`, если копия не упоминается или Claude сам лежит в копиях.
pub fn remap(value: &str, copies: &Path, installed: &Path, fallback_app: &Path) -> Option<String> {
    // ASCII-нижний регистр не меняет длину в байтах — позиции совпадают с исходником.
    let lower = |p: &Path| p.display().to_string().to_ascii_lowercase();
    let prefix = format!("{}\\", lower(copies));
    if [installed, fallback_app]
        .iter()
        .any(|p| format!("{}\\", lower(p)).starts_with(&prefix))
    {
        return None; // путь к Claude в настройках указывает на саму копию — возвращать некуда
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    let mut changed = false;
    // Идём только вперёд: подставленный текст повторно не просматривается.
    while let Some(at) = rest.to_ascii_lowercase().find(&prefix) {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let end = after.find(['\\', '"', ' ']).unwrap_or(after.len());
        let part = &after[..end];
        let target = if part.to_ascii_lowercase().starts_with("app-") {
            fallback_app.to_path_buf()
        } else {
            // Несуществующий путь в реестре хуже пути копии: такое значение не трогаем.
            let file = installed.join(part);
            if !file.exists() {
                return None;
            }
            file
        };
        out.push_str(&target.display().to_string());
        rest = &after[end..];
        changed = true;
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Что сделать со значением реестра, которое сейчас указывает в папку копий.
#[derive(Debug, PartialEq, Eq)]
pub enum Fix {
    Set(String),
    Remove,
    Skip,
}

/// Значение из снимка — если его программа ещё на месте (Claude мог обновиться,
/// пока копия была открыта). Новое значение автозапуска, которого до копии не было, —
/// убрать. Иначе — `remap` на установленный Claude.
pub fn decide(
    now: &str,
    saved: Option<&str>,
    had_snapshot: bool,
    is_run: bool,
    copies: &Path,
    installed: &Path,
    fallback_app: &Path,
) -> Fix {
    if let Some(saved) = saved.filter(|s| program_exists(s)) {
        return Fix::Set(saved.to_string());
    }
    if is_run && had_snapshot && saved.is_none() {
        return Fix::Remove;
    }
    remap(now, copies, installed, fallback_app).map_or(Fix::Skip, Fix::Set)
}

/// Значение реестра портативного Claude при уборке: прежнее (если его программа на
/// месте) или удалить.
pub fn forget_fix(saved: Option<&str>) -> Fix {
    match saved.filter(|s| program_exists(s)) {
        Some(saved) => Fix::Set(saved.to_string()),
        None => Fix::Remove,
    }
}

/// Существует ли программа из командной строки реестра (`"C:\…\x.exe" -- "%1"`).
fn program_exists(command: &str) -> bool {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => command.split(' ').next().unwrap_or(""),
    };
    !program.is_empty() && Path::new(program).exists()
}

/// Копия Claude при старте пишет в HKCU свой путь: ссылки `claude://`, COM-классы,
/// пункт «Open in Claude Code», автозапуск, MuiCache (проверено вживую: `claude://`
/// и COM-класс уведомлений она переписывает). Эти ключи принадлежат установленному
/// Claude: возвращаем значение из снимка до запуска, а если снимка нет (ремонт при
/// старте приложения) — пересаживаем путь на установленный Claude (`remap`).
#[cfg(windows)]
pub mod registry {
    use std::path::Path;
    use windows_registry::CURRENT_USER;

    const DEFAULTS: [&str; 3] = [
        r"Software\Classes\claude\shell\open\command",
        r"Software\Classes\Directory\shell\ClaudeCode\command",
        r"Software\Classes\Directory\Background\shell\ClaudeCode\command",
    ];
    const CLSID: &str = r"Software\Classes\CLSID";
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const MUI: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache";

    /// Значения до запуска копии: (ключ, имя значения, данные), без путей копий.
    pub struct Snapshot(Vec<(String, String, String)>);

    /// Все значения, которые копия может переписать: (ключ, имя значения).
    fn watched() -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = DEFAULTS
            .iter()
            .map(|p| (p.to_string(), String::new()))
            .collect();
        if let Ok(keys) = CURRENT_USER
            .open(CLSID)
            .and_then(|k| k.keys().map(|k| k.collect::<Vec<_>>()))
        {
            out.extend(
                keys.into_iter()
                    .map(|n| (format!(r"{CLSID}\{n}\LocalServer32"), String::new())),
            );
        }
        if let Ok(names) = CURRENT_USER
            .open(RUN)
            .and_then(|k| k.values().map(|v| v.map(|(n, _)| n).collect::<Vec<_>>()))
        {
            out.extend(names.into_iter().map(|n| (RUN.to_string(), n)));
        }
        out
    }

    fn read(path: &str, name: &str) -> Option<String> {
        CURRENT_USER.open(path).ok()?.get_string(name).ok()
    }

    fn mentions(text: &str, copies: &Path) -> bool {
        let dir = format!("{}\\", copies.display()).to_ascii_lowercase();
        text.to_ascii_lowercase().contains(&dir)
    }

    pub fn snapshot(copies: &Path) -> Snapshot {
        Snapshot(
            watched()
                .into_iter()
                .filter_map(|(path, name)| {
                    let value = read(&path, &name)?;
                    (!mentions(&value, copies)).then_some((path, name, value))
                })
                .collect(),
        )
    }

    pub fn repair(copies: &Path, installed: &Path, fallback_app: &Path, before: Option<&Snapshot>) {
        for (path, name) in watched() {
            let Some(now) = read(&path, &name).filter(|v| mentions(v, copies)) else {
                continue;
            };
            let saved = before.and_then(|s| {
                s.0.iter()
                    .find(|(p, n, _)| {
                        p.eq_ignore_ascii_case(&path) && n.eq_ignore_ascii_case(&name)
                    })
                    .map(|(_, _, v)| v.as_str())
            });
            let fix = super::decide(
                &now,
                saved,
                before.is_some(),
                path == RUN,
                copies,
                installed,
                fallback_app,
            );
            let result = match &fix {
                super::Fix::Set(new) => CURRENT_USER
                    .create(&path)
                    .and_then(|k| k.set_string(&name, new)),
                super::Fix::Remove => CURRENT_USER
                    .create(&path)
                    .and_then(|k| k.remove_value(&name)),
                super::Fix::Skip => continue,
            };
            match result {
                Ok(()) => {
                    tracing::info!(key = %path, value = %name, fix = ?fix, "registry: copy trace removed")
                }
                Err(exc) => tracing::warn!(key = %path, error = %exc, "registry: cannot replace"),
            }
        }
        forget_mui(copies);
    }

    /// Портативный Claude без установленного: всё, что он записал о себе, убирается, а
    /// бывшее до него значение (из снимка) возвращается — в системе от него не остаётся
    /// ничего, кроме пустых ключей.
    pub fn forget(root: &Path, before: Option<&Snapshot>) {
        // Ключи, которые портативный Claude создаёт целиком (`claude://`, «Open in Claude
        // Code»): если их команда ушла, пустой ключ с `URL Protocol` давал бы ошибку вместо
        // «программа не найдена» — убираем ключ целиком.
        const ROOTS: [&str; 3] = [
            r"Software\Classes\claude",
            r"Software\Classes\Directory\shell\ClaudeCode",
            r"Software\Classes\Directory\Background\shell\ClaudeCode",
        ];
        for (path, name) in watched() {
            if read(&path, &name).is_none_or(|v| !mentions(&v, root)) {
                continue;
            }
            let saved = before.and_then(|s| {
                s.0.iter()
                    .find(|(p, n, _)| {
                        p.eq_ignore_ascii_case(&path) && n.eq_ignore_ascii_case(&name)
                    })
                    .map(|(_, _, v)| v.clone())
            });
            let result = match super::forget_fix(saved.as_deref()) {
                super::Fix::Set(old) => CURRENT_USER
                    .create(&path)
                    .and_then(|k| k.set_string(&name, &old)),
                _ => match DEFAULTS.iter().position(|d| d.eq_ignore_ascii_case(&path)) {
                    Some(i) => CURRENT_USER.remove_tree(ROOTS[i]),
                    None => CURRENT_USER
                        .create(&path)
                        .and_then(|k| k.remove_value(&name)),
                },
            };
            if let Err(exc) = result {
                tracing::warn!(key = %path, error = %exc, "registry: cannot clean");
            }
        }
        forget_mui(root);
    }

    /// Ключи установленного Claude, которые Claude с другим профилем (PGC, портативный)
    /// при старте удаляет целиком, если в этом профиле функции нет: пункт «Open in Claude
    /// Code» в меню папок (проверено вживую 23.11.2026, Claude 6.28899.6) и `claude://`.
    pub const OWNED: [&str; 6] = [
        r"Software\Classes\Directory\shell\ClaudeCode",
        r"Software\Classes\Directory\shell\ClaudeCode\command",
        r"Software\Classes\Directory\Background\shell\ClaudeCode",
        r"Software\Classes\Directory\Background\shell\ClaudeCode\command",
        r"Software\Classes\claude",
        r"Software\Classes\claude\shell\open\command",
    ];

    /// Все строковые значения ключей до запуска.
    pub struct Keys(Vec<(String, String, String)>);

    pub fn capture(paths: &[&str]) -> Keys {
        let mut out = Vec::new();
        for path in paths {
            let Ok(key) = CURRENT_USER.open(path) else {
                continue;
            };
            let Ok(values) = key.values() else {
                continue;
            };
            for (name, value) in values {
                if let Ok(text) = String::try_from(value) {
                    out.push((path.to_string(), name, text));
                }
            }
        }
        Keys(out)
    }

    /// Возвращает значения, которых после запуска не стало (существующие не трогает: их
    /// чинят `repair` и `forget`).
    pub fn put_back(keys: &Keys) {
        for (path, name, value) in &keys.0 {
            let gone = CURRENT_USER
                .open(path)
                .and_then(|k| k.get_string(name))
                .is_err();
            if !gone {
                continue;
            }
            match CURRENT_USER
                .create(path)
                .and_then(|k| k.set_string(name, value))
            {
                Ok(()) => tracing::info!(key = %path, value = %name, "registry: value put back"),
                Err(exc) => tracing::warn!(key = %path, error = %exc, "registry: cannot put back"),
            }
        }
    }

    /// MuiCache — кэш имён программ: путь копии в имени значения, просто убираем.
    fn forget_mui(copies: &Path) {
        let copies_lower = copies.display().to_string().to_ascii_lowercase();
        if let Ok(mui) = CURRENT_USER.create(MUI)
            && let Ok(values) = mui.values()
        {
            for (name, _) in values.collect::<Vec<_>>() {
                if name.to_ascii_lowercase().contains(&copies_lower) {
                    let _ = mui.remove_value(&name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// На своём временном ключе в HKCU: удалённый целиком ключ возвращается, а значение,
    /// которое после запуска есть (пусть другое), не трогается.
    #[cfg(windows)]
    #[test]
    fn deleted_keys_come_back_and_present_ones_stay() {
        use windows_registry::CURRENT_USER;
        let root = format!(r"Software\PGC-test-{}", uuid::Uuid::new_v4().simple());
        let verb = format!(r"{root}\verb");
        let command = format!(r"{verb}\command");
        let key = CURRENT_USER.create(&verb).unwrap();
        key.set_string("", "Open in Claude Code").unwrap();
        key.set_string("Icon", "\"C:\\x\\claude.exe\",0").unwrap();
        CURRENT_USER
            .create(&command)
            .unwrap()
            .set_string("", "old")
            .unwrap();
        let saved = registry::capture(&[&verb, &command]);
        CURRENT_USER.remove_tree(&verb).unwrap();
        CURRENT_USER
            .create(&command)
            .unwrap()
            .set_string("", "new")
            .unwrap();
        registry::put_back(&saved);
        let key = CURRENT_USER.open(&verb).unwrap();
        assert_eq!(key.get_string("").unwrap(), "Open in Claude Code");
        assert_eq!(key.get_string("Icon").unwrap(), "\"C:\\x\\claude.exe\",0");
        let command = CURRENT_USER.open(&command).unwrap().get_string("").unwrap();
        assert_eq!(command, "new", "a present value is left to repair/forget");
        CURRENT_USER.remove_tree(&root).unwrap();
    }

    #[test]
    fn portable_traces_go_back_to_what_was_there_or_away() {
        // Прежняя программа на месте — её значение возвращается; нет — значение убирается.
        let exe = std::env::current_exe().unwrap();
        let before = format!("\"{}\" \"%1\"", exe.display());
        assert_eq!(forget_fix(Some(&before)), Fix::Set(before.clone()));
        assert_eq!(
            forget_fix(Some("\"C:\\nope\\claude.exe\" \"%1\"")),
            Fix::Remove
        );
        assert_eq!(forget_fix(None), Fix::Remove);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-ruui-{tag}-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Список языков Claude 5.11492.5 и 6.6395.0, как он стоит в бандле.
    const LOCALES: &[u8] =
        br#"["en-US","de-DE","fr-FR","ko-KR","ja-JP","es-419","es-ES","it-IT","hi-IN","pt-BR","id-ID"]"#;
    const LOCALES_RU: &[u8] =
        br#"["en-US","de-DE","fr-FR","ko-KR","ja-JP","es-419","es-ES","it-IT","hi-IN","pt-BR","id-ID","ru"]"#;

    /// Маленький настоящий asar: index.html, бандл со списком языков и `i18n/*.json`.
    fn sample_asar(path: &Path) {
        sample_asar_with(path, LOCALES);
    }

    fn sample_asar_with(path: &Path, list: &[u8]) {
        sample_asar_full(path, list, b"console.log(1)");
    }

    fn sample_asar_full(path: &Path, list: &[u8], b_js: &[u8]) {
        let locales: Vec<String> = serde_json::from_slice(list).unwrap();
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("index.html".into(), b"<html></html>".to_vec()),
            (
                "assets/v1/a.js".into(),
                [b"var zm=".as_slice(), list, b";"].concat(),
            ),
            ("assets/v1/b.js".into(), b_js.to_vec()),
        ];
        files.extend(
            locales
                .iter()
                .map(|l| (format!("i18n/{l}.json"), br#"{"k":"v"}"#.to_vec())),
        );
        write_asar(path, &files);
    }

    /// Настоящий asar из файлов (путь через `/`, данные).
    fn write_asar(path: &Path, files: &[(String, Vec<u8>)]) {
        let mut json = json!({"files": {}});
        let mut payload = Vec::new();
        for (name, data) in files {
            *entry_mut(&mut json, name).unwrap() = json!({
                "size": data.len(), "offset": payload.len().to_string(), "integrity": integrity(data)
            });
            payload.extend_from_slice(data);
        }
        let raw = serde_json::to_vec(&json).unwrap();
        let payload_size = align4(4 + raw.len() as u64);
        let mut out = Vec::new();
        for word in [4, payload_size + 4, payload_size, raw.len() as u64] {
            out.extend_from_slice(&(word as u32).to_le_bytes());
        }
        out.extend_from_slice(&raw);
        out.resize(
            out.len() + (payload_size - 4 - raw.len() as u64) as usize,
            0,
        );
        out.extend_from_slice(&payload);
        fs::write(path, out).unwrap();
    }

    fn read_file(asar: &Path, path: &str) -> (Vec<u8>, Value) {
        let mut file = File::open(asar).unwrap();
        let header = read_header(&mut file).unwrap();
        let mut node = &header.json;
        for part in path.split('/') {
            node = &node["files"][part];
        }
        let offset: u64 = node["offset"].as_str().unwrap().parse().unwrap();
        let mut data = vec![0u8; node["size"].as_u64().unwrap() as usize];
        file.seek(SeekFrom::Start(header.base + offset)).unwrap();
        file.read_exact(&mut data).unwrap();
        (data, node["integrity"].clone())
    }

    /// Перевод больше 4 МиБ: два блока целостности, как у настоящего ru.json.
    fn big_translation() -> Vec<u8> {
        let map: serde_json::Map<String, Value> = (0..60_000)
            .map(|i| {
                (
                    format!("key{i:05}"),
                    Value::String(format!("строка {i} {}", "ы".repeat(30))),
                )
            })
            .collect();
        serde_json::to_vec(&map).unwrap()
    }

    #[test]
    fn copy_reports_and_patches_folderless_gate() {
        let dir = temp_dir("folderless");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        let gate = br#"function Ub(){return Ve("claudeai_cc_epitaxy_folderless_start_main")}"#;
        sample_asar_full(&src, LOCALES, gate);
        assert!(locale_changes(&src).unwrap().folderless);
        patch_ion_dist(&src, &dst, &big_translation(), &[]).unwrap();
        let (js, integ) = read_file(&dst, "assets/v1/b.js");
        assert_eq!(js, br#"function Ub(){return !0}"#.to_vec());
        assert_eq!(integ, integrity(&js));

        // A Claude version without the gate: Russian still works, the flag says so.
        sample_asar(&src);
        let patches = locale_changes(&src).unwrap();
        assert!(!patches.folderless);
        assert_eq!(patches.files.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folderless_gate_call_becomes_true() {
        let src = br#"function Ub(){return Ve("claudeai_cc_epitaxy_folderless_start_main")&&!!Pe?.createScratchWorkspace}"#;
        let out = enable_folderless(src).unwrap();
        assert_eq!(
            out,
            br#"function Ub(){return !0&&!!Pe?.createScratchWorkspace}"#.to_vec()
        );
        // Member calls and other gates stay as they are.
        assert!(
            enable_folderless(br#"a.Ve("claudeai_cc_epitaxy_folderless_start_main")"#).is_none()
        );
        assert!(enable_folderless(br#"Ve("claudeai_cc_epitaxy_folder_browser")"#).is_none());
        assert_eq!(
            enable_folderless(br#"x=$e("claudeai_cc_epitaxy_folderless_start_main")"#).unwrap(),
            b"x=!0".to_vec()
        );
    }

    #[test]
    fn patched_archive_has_translation_locale_and_valid_integrity() {
        let dir = temp_dir("asar");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        sample_asar(&src);
        let ru = big_translation();
        assert!(ru.len() > BLOCK);

        let (old_hash, new_hash) = patch_ion_dist(&src, &dst, &ru, &[]).unwrap();
        assert_ne!(old_hash, new_hash);

        let (data, integ) = read_file(&dst, "i18n/ru.json");
        assert_eq!(data, ru);
        assert_eq!(integ["blocks"].as_array().unwrap().len(), 2);
        assert_eq!(integ["hash"], sha256_hex(&ru));
        assert_eq!(integ["blocks"][1], sha256_hex(&ru[BLOCK..]));

        // The effort menu's catalog strings: Claude reads them from the dynamic language file.
        // This sample has no dynamic/en-US.json, so the bundled catalog goes in whole.
        let (raw, integ) = read_file(&dst, "i18n/dynamic/ru.json");
        let dynamic: serde_json::Map<String, Value> = serde_json::from_slice(&raw).unwrap();
        assert_eq!(dynamic["HoZQmASaw3"], "Очень высокий");
        assert_eq!(dynamic.len(), 51);
        assert_eq!(integ["hash"], sha256_hex(&raw));

        let (js, integ) = read_file(&dst, "assets/v1/a.js");
        assert_eq!(js, [b"var zm=".as_slice(), LOCALES_RU, b";"].concat());
        assert_eq!(integ["hash"], sha256_hex(&js));
        assert_eq!(read_file(&dst, "assets/v1/b.js").0, b"console.log(1)");
        assert_eq!(read_file(&dst, "index.html").0, b"<html></html>");

        // Хеш заголовка — то, что будет вписано в exe.
        let mut file = File::open(&dst).unwrap();
        assert_eq!(sha256_hex(&read_header(&mut file).unwrap().raw), new_hash);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// ADR-009: per key of this Claude's dynamic/en-US.json — the downloaded file, then the
    /// bundled catalog, then the main translation of the same English text; else no key.
    #[test]
    fn menu_translation_takes_sources_in_order() {
        let dir = temp_dir("catalog");
        let src = dir.join("src.asar");
        let english = json!({
            "official": "Official only",
            "2EcGZn58SU": "Auto thinking",  // bundled too: the downloaded one wins
            "HoZQmASaw3": "Medium",         // bundled; its text also matches m1
            "by-text": "Medium",
            "nowhere": "Nothing",
        });
        write_asar(
            &src,
            &[
                (
                    "i18n/en-US.json".into(),
                    br#"{"m0":"Other","m1":"Medium","m2":"Medium"}"#.to_vec(),
                ),
                (
                    "i18n/dynamic/en-US.json".into(),
                    serde_json::to_vec(&english).unwrap(),
                ),
            ],
        );
        let ru: serde_json::Map<String, Value> =
            serde_json::from_value(json!({"m1": "Средний", "m2": "Другой"})).unwrap();
        let official = r#"{"official":"Официальный","2EcGZn58SU":"Скачанный","stale":"Старый"}"#;

        let menu = catalog_ru(&src, &ru, official.as_bytes());
        let out: Value = serde_json::from_slice(&menu.json).unwrap();
        assert_eq!(
            out,
            json!({
                "official": "Официальный",
                "2EcGZn58SU": "Скачанный",
                "HoZQmASaw3": "Очень высокий",
                "by-text": "Средний",
            })
        );
        assert_eq!((menu.translated, menu.total), (4, 5));

        // Nothing downloaded: the bundled catalog and the text match still cover their keys.
        let out: Value = serde_json::from_slice(&catalog_ru(&src, &ru, b"").json).unwrap();
        assert_eq!(out["2EcGZn58SU"], "Автоматические размышления");
        assert_eq!(out["by-text"], "Средний");
        assert!(out.get("official").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_menu_translation_changes_only_the_translation_part_of_the_marker() {
        let dir = temp_dir("marker");
        let app = dir.join("app-1.0.0");
        fs::create_dir_all(app.join("resources")).unwrap();
        fs::write(app.join("claude.exe"), b"exe").unwrap();
        fs::write(app.join("resources").join("ion-dist.asar"), b"asar").unwrap();
        let (_, plain) = copy_marker(&app, b"ru", b"").unwrap();
        let (_, with) = copy_marker(&app, b"ru", b"{}").unwrap();
        let (_, other) = copy_marker(&app, b"ru", br#"{"a":"b"}"#).unwrap();
        assert_ne!(plain, with);
        assert_ne!(with, other);
        // An open copy keeps running until closed, as with a new main translation.
        fs::write(dir.join("marker"), &plain).unwrap();
        assert!(only_translation_differs(&dir.join("marker"), &other));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_unknown_bundle_and_bad_translation() {
        let dir = temp_dir("refuse");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        sample_asar(&src);
        assert!(patch_ion_dist(&src, &dst, b"not json", &[]).is_err());
        assert!(patch_ion_dist(&src, &dst, b"{\"a\":\"b\"}", &[]).is_err());

        // Бандл, где список не совпадает с файлами языков (другая версия), — отказ, а не
        // «тихо без русского».
        let bytes = fs::read(&src).unwrap();
        let altered = replace_all(&bytes, b"\"id-ID\"]", b"\"xx-XX\"]").unwrap();
        fs::write(&src, altered).unwrap();
        let error = patch_ion_dist(&src, &dst, &big_translation(), &[]).unwrap_err();
        assert!(error.to_string().contains("список языков"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_version_with_reordered_and_added_locales_is_patched() {
        let dir = temp_dir("reorder");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        let list = br#"["en-US","pl-PL","de-DE","ja-JP"]"#;
        sample_asar_with(&src, list);
        patch_ion_dist(&src, &dst, &big_translation(), &[]).unwrap();
        let (js, _) = read_file(&dst, "assets/v1/a.js");
        assert_eq!(js, br#"var zm=["en-US","pl-PL","de-DE","ja-JP","ru"];"#);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Новый поиск списка языков на настоящем Claude даёт те же байты, что прежняя
    /// замена строки (плюс патч «Без папки» поверх неё):
    /// `CTP_RU_APP=<…\app-x> cargo test -p ctp-app same_as_literal -- --ignored`
    #[test]
    #[ignore]
    fn same_as_literal_on_real_install() {
        let app = PathBuf::from(std::env::var_os("CTP_RU_APP").expect("CTP_RU_APP"));
        let asar = app.join("resources").join("ion-dist.asar");
        let patches = locale_changes(&asar).unwrap();
        assert!(patches.folderless, "folderless gate not found");
        let changes = patches.files;
        assert!(!changes.is_empty());
        for (path, patched) in &changes {
            let (orig, _) = read_file(&asar, path);
            let literal = replace_all(&orig, LOCALES, LOCALES_RU);
            let expected = enable_folderless(literal.as_deref().unwrap_or(&orig)).or(literal);
            assert_eq!(expected.as_deref(), Some(patched.as_slice()), "{path}");
        }
        println!("{} бандлов совпали", changes.len());
    }

    /// Ручная проверка на установленном Claude (оригинал только читается):
    /// `CTP_RU_APP=<…\app-x> CTP_RU_JSON=<ru.json> CTP_RU_OUT=<папка> cargo test -p ctp-app real_install -- --ignored`
    #[test]
    #[ignore]
    fn real_install() {
        let var = |name: &str| PathBuf::from(std::env::var_os(name).expect(name));
        let ru = fs::read(var("CTP_RU_JSON")).unwrap();
        let exe = ensure_copy(&var("CTP_RU_APP"), &var("CTP_RU_OUT"), &ru, &[]).unwrap();
        println!("copy: {}", exe.display());
    }

    #[test]
    fn exe_hash_must_occur_exactly_once() {
        let old = "a".repeat(64);
        let new = "b".repeat(64);
        let exe = format!("MZ..{old}..");
        let patched = patch_exe(exe.as_bytes(), &old, &new).unwrap();
        assert_eq!(patched, format!("MZ..{new}..").into_bytes());
        assert!(patch_exe(b"MZ nothing", &old, &new).is_err());
        assert!(patch_exe(format!("{old}{old}").as_bytes(), &old, &new).is_err());
    }

    fn header_hash(asar: &Path) -> String {
        sha256_hex(&read_header(&mut File::open(asar).unwrap()).unwrap().raw)
    }

    #[test]
    fn copy_gets_the_env_prelude_and_both_hashes() {
        let dir = temp_dir("prelude");
        let app = dir.join("install").join("app-9.9.9");
        let resources = app.join("resources");
        fs::create_dir_all(resources.join("app.asar.unpacked")).unwrap();
        sample_asar(&resources.join("ion-dist.asar"));
        let entry = br#""use strict";console.log(1)"#.to_vec();
        write_asar(
            &resources.join("app.asar"),
            &[
                (
                    "package.json".into(),
                    br#"{"main":".vite/build/index.pre.js"}"#.to_vec(),
                ),
                (".vite/build/index.pre.js".into(), entry),
            ],
        );
        fs::write(
            resources.join("app.asar.unpacked").join("native.node"),
            b"bin",
        )
        .unwrap();
        let ion = header_hash(&resources.join("ion-dist.asar"));
        let app_hash = header_hash(&resources.join("app.asar"));
        fs::write(app.join("claude.exe"), format!("MZ{ion}MZ{app_hash}MZ")).unwrap();

        let cache = dir.join("cache");
        let exe = ensure_copy(&app, &cache, &big_translation(), &[]).unwrap();
        let copy = cache.join("app-9.9.9").join("resources");
        let (patched, integ) = read_file(&copy.join("app.asar"), ".vite/build/index.pre.js");
        assert!(patched.starts_with(
            br#""use strict";;(()=>{try{const real=process.env.CTP_REAL_LOCALAPPDATA"#
        ));
        assert!(patched.ends_with(b"console.log(1)"));
        assert_eq!(integ, integrity(&patched));
        // Both asar hashes in the copy's exe are the new ones; unpacked files are copied.
        let exe_text = String::from_utf8(fs::read(&exe).unwrap()).unwrap();
        assert!(exe_text.contains(&header_hash(&copy.join("ion-dist.asar"))));
        assert!(exe_text.contains(&header_hash(&copy.join("app.asar"))));
        assert!(!exe_text.contains(&app_hash));
        assert!(copy.join("app.asar.unpacked").join("native.node").is_file());
        // Self-check reads the built copy: the prelude is there.
        assert!(env_prelude_ok(&app, &cache));

        // Without Russian: the same prelude, the interface bundle left as it is.
        let plain = dir.join("plain");
        let exe = ensure_copy(&app, &plain, &[], &[]).unwrap();
        let copy = plain.join("app-9.9.9").join("resources");
        let (patched, _) = read_file(&copy.join("app.asar"), ".vite/build/index.pre.js");
        assert!(patched.starts_with(br#""use strict";;(()=>{try{"#));
        assert_eq!(
            fs::read(copy.join("ion-dist.asar")).unwrap(),
            fs::read(resources.join("ion-dist.asar")).unwrap()
        );
        let exe_text = String::from_utf8(fs::read(&exe).unwrap()).unwrap();
        assert!(exe_text.contains(&ion));
        assert!(exe_text.contains(&header_hash(&copy.join("app.asar"))));
        assert_eq!(
            copy_state(&app, &plain, &[], &[]).unwrap(),
            CopyState::Current
        );
        // Turning Russian on later rebuilds it: the translation is part of the marker.
        assert_eq!(
            copy_state(&app, &plain, &big_translation(), &[]).unwrap(),
            CopyState::Stale
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_without_translation_needs_the_prelude() {
        // A Claude whose app.asar this version cannot patch: a plain copy would change nothing,
        // so it is refused and the original opens.
        let dir = temp_dir("plain-refused");
        let app = dir.join("install").join("app-9.9.9");
        fs::create_dir_all(app.join("resources")).unwrap();
        sample_asar(&app.join("resources").join("ion-dist.asar"));
        fs::write(app.join("resources").join("app.asar"), b"app").unwrap();
        fs::write(app.join("claude.exe"), b"MZ").unwrap();
        let cache = dir.join("cache");
        assert!(ensure_copy(&app, &cache, &[], &[]).is_err());
        assert!(!cache.join("app-9.9.9").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The prelude itself in a real Node: children get the real folder, an explicit other
    /// value stays, promisified execFile keeps {stdout, stderr}. Skipped without node.
    #[test]
    fn prelude_gives_children_the_real_folder_in_node() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("node not found: skipped");
            return;
        }
        let dir = temp_dir("node");
        let script = dir.join("probe.js");
        let body = r#"
const cp=require("child_process"),util=require("util");
const show=["-e","process.stdout.write(String(process.env.LOCALAPPDATA)+'|'+String(process.env.CTP_REAL_LOCALAPPDATA))"];
const out=[];
out.push(cp.execFileSync(process.execPath,show).toString());
out.push(cp.spawnSync(process.execPath,show,{env:Object.assign({},process.env,{LOCALAPPDATA:"custom"})}).stdout.toString());
out.push(cp.execSync("node -e process.stdout.write(process.env.LOCALAPPDATA)").toString());
out.push(String(process.env.LOCALAPPDATA)+'|'+String(process.env.CTP_REAL_LOCALAPPDATA));
// undefined/null where args or options go: the callback must still be called.
cp.execFile("hostname",undefined,undefined,(e0)=>{out.push("cb0:"+(e0?"err":"ok"));
cp.execFile(process.execPath,show,undefined,(e,so)=>{out.push("cb1:"+so);
cp.exec("node -e process.stdout.write(process.env.LOCALAPPDATA)",null,(e2,so2)=>{out.push("cb2:"+so2);
util.promisify(cp.execFile)(process.execPath,show).then((r)=>{out.push(typeof r.stdout);out.push(r.stdout);console.log(JSON.stringify(out));});});});});
"#;
        fs::write(&script, [ENV_PRELUDE, body].concat()).unwrap();
        let output = std::process::Command::new("node")
            .arg(&script)
            .env("LOCALAPPDATA", "C:/over")
            .env(REAL_LOCALAPPDATA_VAR, "C:/real")
            .output()
            .unwrap();
        let got: Vec<String> = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(
            got,
            [
                "C:/real|undefined",
                "custom|undefined",
                "C:/real",
                "C:/over|C:/real",
                "cb0:ok",
                "cb1:C:/real|undefined",
                "cb2:C:/real",
                "string",
                "C:/real|undefined"
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_is_built_once_and_rebuilt_when_translation_changes() {
        let dir = temp_dir("copy");
        let app = dir.join("install").join("app-9.9.9");
        fs::create_dir_all(app.join("resources")).unwrap();
        sample_asar(&app.join("resources").join("ion-dist.asar"));
        let mut file = File::open(app.join("resources").join("ion-dist.asar")).unwrap();
        let hash = sha256_hex(&read_header(&mut file).unwrap().raw);
        drop(file);
        fs::write(app.join("claude.exe"), format!("MZ{hash}MZ")).unwrap();
        fs::write(app.join("resources").join("app.asar"), b"app").unwrap();
        let before = fs::read(app.join("claude.exe")).unwrap();
        let asar_before = fs::read(app.join("resources").join("ion-dist.asar")).unwrap();

        let cache = dir.join("cache");
        let ru = big_translation();
        assert_eq!(
            copy_state(&app, &cache, &ru, &[]).unwrap(),
            CopyState::Stale
        );
        let exe = ensure_copy(&app, &cache, &ru, &[]).unwrap();
        assert_eq!(
            copy_state(&app, &cache, &ru, &[]).unwrap(),
            CopyState::Current
        );
        assert_eq!(exe, cache.join("app-9.9.9").join("claude.exe"));
        assert!(
            cache
                .join("app-9.9.9")
                .join("resources")
                .join("app.asar")
                .is_file()
        );
        let mut copied = File::open(
            cache
                .join("app-9.9.9")
                .join("resources")
                .join("ion-dist.asar"),
        )
        .unwrap();
        let new_hash = sha256_hex(&read_header(&mut copied).unwrap().raw);
        drop(copied);
        assert_eq!(
            fs::read(&exe).unwrap(),
            format!("MZ{new_hash}MZ").into_bytes()
        );
        assert_eq!(
            fs::read(app.join("claude.exe")).unwrap(),
            before,
            "оригинал не тронут"
        );

        // Повторный вызов ничего не пересобирает.
        let marker = cache.join("app-9.9.9").join(MARKER);
        let stamp_before = fs::metadata(&marker).unwrap().modified().unwrap();
        ensure_copy(&app, &cache, &ru, &[]).unwrap();
        assert_eq!(
            fs::metadata(&marker).unwrap().modified().unwrap(),
            stamp_before
        );

        // Другой перевод — новая сборка; копии других версий удаляются.
        fs::create_dir_all(cache.join("app-1.0.0")).unwrap();
        let mut other = ru.clone();
        other.splice(1..1, b"\"extra\":\"x\",".iter().copied());
        // Закрытая устаревшая копия — Stale (не Open), пробой не повреждена.
        assert_eq!(
            copy_state(&app, &cache, &other, &[]).unwrap(),
            CopyState::Stale
        );
        assert!(exe.is_file());
        ensure_copy(&app, &cache, &other, &[]).unwrap();
        assert_eq!(
            copy_state(&app, &cache, &other, &[]).unwrap(),
            CopyState::Current
        );
        assert!(!cache.join("app-1.0.0").exists());
        assert!(!cache.join("app-9.9.9.building").exists());
        let names = |cache: &Path| -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(cache)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(&cache), vec!["app-9.9.9"], "корзина убрана");

        // Пока идёт сборка (фоновая после обновления Claude), вторая ждёт её, а не
        // отказывает: иначе «Открыть Claude» в это время открыл бы английский.
        {
            let held = BUILD.lock().unwrap();
            let waiter = {
                let (app, cache) = (app.clone(), cache.clone());
                let other = other.clone();
                std::thread::spawn(move || ensure_copy(&app, &cache, &other, &[]))
            };
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(!waiter.is_finished(), "ждёт идущую сборку");
            drop(held);
            assert_eq!(waiter.join().unwrap().unwrap(), exe);
        }

        // Открытую копию (exe открыт без права удаления, как у запущенной программы)
        // Windows переименовать не даёт. Сменился только перевод — запускается она же;
        // сменился сам Claude — отказ. Копия цела, мусора нет.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let running = fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ
                .open(&exe)
                .unwrap();
            // Для фона: копия открыта и устарела (собрана с `other`) — Open, а при её
            // же переводе — Current без всякой пробы.
            assert_eq!(copy_state(&app, &cache, &ru, &[]).unwrap(), CopyState::Open);
            assert_eq!(
                copy_state(&app, &cache, &other, &[]).unwrap(),
                CopyState::Current
            );
            assert_eq!(ensure_copy(&app, &cache, &ru, &[]).unwrap(), exe);
            // «Обновлённый» Claude: тот же файл с другой отметкой времени.
            let original = fs::OpenOptions::new()
                .write(true)
                .open(app.join("claude.exe"))
                .unwrap();
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
            original.set_modified(later).unwrap();
            drop(original);
            let error = ensure_copy(&app, &cache, &ru, &[]).unwrap_err();
            assert!(error.to_string().contains("открыта"), "{error}");
            drop(running);
            assert!(
                fs::read_to_string(&marker)
                    .unwrap()
                    .contains(&sha256_hex(&other))
            );
            assert_eq!(names(&cache), vec!["app-9.9.9"]);
        }

        // Версия без списка языков: отказ до копирования, без недособранной папки.
        let asar = app.join("resources").join("ion-dist.asar");
        let altered = replace_all(&fs::read(&asar).unwrap(), b"\"id-ID\"]", b"\"xx-XX\"]").unwrap();
        fs::write(&asar, &altered).unwrap();
        assert!(ensure_copy(&app, &cache, &ru, &[]).is_err());
        assert_eq!(names(&cache), vec!["app-9.9.9"]);
        fs::write(&asar, &asar_before).unwrap();

        assert_eq!(fs::read(app.join("claude.exe")).unwrap(), before);
        assert_eq!(
            fs::read(&asar).unwrap(),
            asar_before,
            "ion-dist.asar оригинала не тронут"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decide_prefers_live_snapshot_then_removes_new_autostart_then_remaps() {
        let dir = temp_dir("decide");
        let copies = dir.join("claude-ru");
        let installed = dir.join("AnthropicClaude");
        let app = installed.join("app-6.0");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("claude.exe"), b"").unwrap();
        let live = format!("\"{}\\claude.exe\" -- \"%1\"", app.display());
        let gone = format!("\"{}\\app-5.0\\claude.exe\" -- \"%1\"", installed.display());
        let now = format!("\"{}\\app-6.0\\claude.exe\" -- \"%1\"", copies.display());
        let d = |saved, snap, run| decide(&now, saved, snap, run, &copies, &installed, &app);

        // Снимок с живой программой — возвращаем как было.
        assert_eq!(d(Some(live.as_str()), true, false), Fix::Set(live.clone()));
        // Программы из снимка уже нет (Claude обновился) — пересадка на найденный Claude.
        assert_eq!(d(Some(gone.as_str()), true, false), Fix::Set(live.clone()));
        // Автозапуска до копии не было — убрать, а не включать установленному.
        assert_eq!(d(None, true, true), Fix::Remove);
        // Без снимка (старт приложения) — пересадка.
        assert_eq!(d(None, false, true), Fix::Set(live.clone()));
        assert_eq!(d(None, true, false), Fix::Set(live));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remap_moves_copy_paths_back_to_installed_claude() {
        let dir = temp_dir("remap");
        let copies = dir.join("Gateway").join("claude-ru");
        let installed = dir.join("AnthropicClaude");
        fs::create_dir_all(installed.join("app-5.1")).unwrap();
        let fallback = installed.join("app-6.0");
        let c = copies.display().to_string();
        let i = installed.display().to_string();

        // Версия копии может быть старой: путь ведёт на Claude, которого нашёл шлюз
        // (как пишет его и сам установленный Claude), а не на ту же старую версию.
        let value = format!("\"{c}\\app-5.1\\claude.exe\" -- \"%1\"");
        let f = fallback.display().to_string();
        assert_eq!(
            remap(&value, &copies, &installed, &fallback).unwrap(),
            format!("\"{f}\\claude.exe\" -- \"%1\"")
        );
        // Регистр не важен.
        let upper = format!("{}\\APP-4.0\\claude.exe", c.to_uppercase());
        assert_eq!(
            remap(&upper, &copies, &installed, &fallback).unwrap(),
            format!("{}\\claude.exe", fallback.display())
        );
        // Чужие значения не трогаем.
        let foreign = format!("\"{i}\\app-5.1\\claude.exe\" -- \"%1\"");
        assert_eq!(remap(&foreign, &copies, &installed, &fallback), None);
        // Не `app-*` (Squirrel: Update.exe) — тот же файл у установленного; два вхождения.
        let squirrel = format!("\"{c}\\Update.exe\" --processStart \"{c}\\app-5.1\\claude.exe\"");
        assert_eq!(
            remap(&squirrel, &copies, &installed, &fallback),
            None,
            "Update.exe у установленного нет — значение не трогаем"
        );
        fs::write(installed.join("Update.exe"), b"").unwrap();
        // Без кавычек аргументы после версии сохраняются.
        let bare = format!("{c}\\app-5.1 --flag");
        assert_eq!(
            remap(&bare, &copies, &installed, &fallback).unwrap(),
            format!("{f} --flag")
        );
        assert_eq!(
            remap(&squirrel, &copies, &installed, &fallback).unwrap(),
            format!("\"{i}\\Update.exe\" --processStart \"{f}\\claude.exe\"")
        );
        // Путь к Claude в настройках указывает на саму копию: ничего не пишем и не зависаем.
        let inside = copies.join("app-5.1");
        assert_eq!(remap(&value, &copies, &copies, &inside), None);
        assert_eq!(remap(&value, &copies, &installed, &inside), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
