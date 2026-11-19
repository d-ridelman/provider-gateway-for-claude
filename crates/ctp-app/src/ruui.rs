//! Русский интерфейс Claude Desktop.
//!
//! Интерфейс (`ion-dist.asar`) знает 11 языков, русского среди них нет, хотя у
//! claude.ai есть официальный перевод (`/i18n/ru.json`). Мы собираем копию
//! установленной версии, в которой:
//! - `i18n/ru.json` — этот перевод (файл кладёт пользователь, в репозитории его нет);
//! - в список языков бандлов добавлен `"ru"`: без этого русская Windows получает `en-US`;
//! - хеш заголовка asar в `claude.exe` обновлён, иначе Electron не откроет архив.
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
const PATCH_REV: u32 = 1;
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

/// `ion-dist.asar` с русским: `i18n/ru.json` и `"ru"` в списке языков бандлов.
pub fn patch_ion_dist(src: &Path, dst: &Path, ru_json: &[u8]) -> Result<(String, String)> {
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

    let mut changes = locale_changes(src)?;
    changes.push(("i18n/ru.json".to_string(), ru_json.to_vec()));
    rebuild(src, dst, &changes)
}

/// Бандлы интерфейса с `"ru"` в списке языков — без записи на диск: годится и для
/// самопроверки новой версии Claude. Нет списка — версия устроена иначе: отказ.
pub fn locale_changes(src: &Path) -> Result<Vec<(String, Vec<u8>)>> {
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
    for (path, offset, size) in files.into_iter().filter(|(p, ..)| p.starts_with("assets/")) {
        let mut data = vec![0u8; size as usize];
        input.seek(SeekFrom::Start(header.base + offset))?;
        input.read_exact(&mut data)?;
        if let Some(patched) = add_ru(&data, &locales) {
            changes.push((path, patched));
        }
    }
    ensure!(
        !changes.is_empty(),
        "{}",
        tr!(
            "в интерфейсе этой версии Claude не найден список языков",
            "no language list found in this Claude version's interface"
        )
    );
    Ok(changes)
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
pub fn ensure_copy(app_dir: &Path, cache_root: &Path, ru_json: &[u8]) -> Result<PathBuf> {
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
    let src_exe = app_dir.join("claude.exe");
    let src_asar = app_dir.join("resources").join("ion-dist.asar");
    let marker = json!({
        "rev": PATCH_REV,
        "exe": stamp(&src_exe)?,
        "asar": stamp(&src_asar)?,
        "ru": sha256_hex(ru_json),
    })
    .to_string();

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
    if let Err(exc) = build(app_dir, &building, ru_json, &marker) {
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
    fs::rename(&building, &target)?;
    remove_leftovers(cache_root, &version);
    Ok(ready)
}

static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
/// потом копия остальных файлов, метка последней.
fn build(app_dir: &Path, building: &Path, ru_json: &[u8], marker: &str) -> Result<()> {
    if building.exists() {
        fs::remove_dir_all(building)?;
    }
    let src_exe = app_dir.join("claude.exe");
    let src_asar = app_dir.join("resources").join("ion-dist.asar");
    fs::create_dir_all(building.join("resources"))?;
    let (old_hash, new_hash) = patch_ion_dist(
        &src_asar,
        &building.join("resources").join("ion-dist.asar"),
        ru_json,
    )?;
    let exe = patch_exe(&fs::read(&src_exe)?, &old_hash, &new_hash)?;
    fs::write(building.join("claude.exe"), exe)?;
    copy_tree(app_dir, building, &[src_exe, src_asar])?;
    fs::write(building.join(MARKER), marker)?;
    Ok(())
}

/// Недособранные папки, корзина и копии прошлых версий (~700 МБ каждая).
/// Открытую копию переименовать нельзя — её не трогаем.
fn remove_leftovers(cache_root: &Path, keep: &str) {
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
        // MuiCache — кэш имён программ: путь копии в имени значения, просто убираем.
        let copies_lower = copies.display().to_string().to_ascii_lowercase();
        if let Ok(mui) = CURRENT_USER.create(MUI) {
            if let Ok(values) = mui.values() {
                for (name, _) in values.collect::<Vec<_>>() {
                    if name.to_ascii_lowercase().contains(&copies_lower) {
                        let _ = mui.remove_value(&name);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let locales: Vec<String> = serde_json::from_slice(list).unwrap();
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("index.html".into(), b"<html></html>".to_vec()),
            (
                "assets/v1/a.js".into(),
                [b"var zm=".as_slice(), list, b";"].concat(),
            ),
            ("assets/v1/b.js".into(), b"console.log(1)".to_vec()),
        ];
        files.extend(
            locales
                .iter()
                .map(|l| (format!("i18n/{l}.json"), br#"{"k":"v"}"#.to_vec())),
        );
        let mut json = json!({"files": {}});
        let mut payload = Vec::new();
        for (name, data) in &files {
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
    fn patched_archive_has_translation_locale_and_valid_integrity() {
        let dir = temp_dir("asar");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        sample_asar(&src);
        let ru = big_translation();
        assert!(ru.len() > BLOCK);

        let (old_hash, new_hash) = patch_ion_dist(&src, &dst, &ru).unwrap();
        assert_ne!(old_hash, new_hash);

        let (data, integ) = read_file(&dst, "i18n/ru.json");
        assert_eq!(data, ru);
        assert_eq!(integ["blocks"].as_array().unwrap().len(), 2);
        assert_eq!(integ["hash"], sha256_hex(&ru));
        assert_eq!(integ["blocks"][1], sha256_hex(&ru[BLOCK..]));

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

    #[test]
    fn refuses_unknown_bundle_and_bad_translation() {
        let dir = temp_dir("refuse");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        sample_asar(&src);
        assert!(patch_ion_dist(&src, &dst, b"not json").is_err());
        assert!(patch_ion_dist(&src, &dst, b"{\"a\":\"b\"}").is_err());

        // Бандл, где список не совпадает с файлами языков (другая версия), — отказ, а не
        // «тихо без русского».
        let bytes = fs::read(&src).unwrap();
        let altered = replace_all(&bytes, b"\"id-ID\"]", b"\"xx-XX\"]").unwrap();
        fs::write(&src, altered).unwrap();
        let error = patch_ion_dist(&src, &dst, &big_translation()).unwrap_err();
        assert!(error.to_string().contains("список языков"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_version_with_reordered_and_added_locales_is_patched() {
        let dir = temp_dir("reorder");
        let (src, dst) = (dir.join("src.asar"), dir.join("dst.asar"));
        let list = br#"["en-US","pl-PL","de-DE","ja-JP"]"#;
        sample_asar_with(&src, list);
        patch_ion_dist(&src, &dst, &big_translation()).unwrap();
        let (js, _) = read_file(&dst, "assets/v1/a.js");
        assert_eq!(js, br#"var zm=["en-US","pl-PL","de-DE","ja-JP","ru"];"#);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Новый поиск списка языков на настоящем Claude даёт те же байты, что прежняя
    /// замена строки (иначе у пользователя зря пересоберётся копия):
    /// `CTP_RU_APP=<…\app-x> cargo test -p ctp-app same_as_literal -- --ignored`
    #[test]
    #[ignore]
    fn same_as_literal_on_real_install() {
        let app = PathBuf::from(std::env::var_os("CTP_RU_APP").expect("CTP_RU_APP"));
        let asar = app.join("resources").join("ion-dist.asar");
        let changes = locale_changes(&asar).unwrap();
        assert!(!changes.is_empty());
        for (path, patched) in &changes {
            let (orig, _) = read_file(&asar, path);
            assert_eq!(
                replace_all(&orig, LOCALES, LOCALES_RU).as_deref(),
                Some(patched.as_slice()),
                "{path}"
            );
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
        let exe = ensure_copy(&var("CTP_RU_APP"), &var("CTP_RU_OUT"), &ru).unwrap();
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
        let exe = ensure_copy(&app, &cache, &ru).unwrap();
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
        ensure_copy(&app, &cache, &ru).unwrap();
        assert_eq!(
            fs::metadata(&marker).unwrap().modified().unwrap(),
            stamp_before
        );

        // Другой перевод — новая сборка; копии других версий удаляются.
        fs::create_dir_all(cache.join("app-1.0.0")).unwrap();
        let mut other = ru.clone();
        other.splice(1..1, b"\"extra\":\"x\",".iter().copied());
        ensure_copy(&app, &cache, &other).unwrap();
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
                std::thread::spawn(move || ensure_copy(&app, &cache, &other))
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
            assert_eq!(ensure_copy(&app, &cache, &ru).unwrap(), exe);
            // «Обновлённый» Claude: тот же файл с другой отметкой времени.
            let original = fs::OpenOptions::new()
                .write(true)
                .open(app.join("claude.exe"))
                .unwrap();
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
            original.set_modified(later).unwrap();
            drop(original);
            let error = ensure_copy(&app, &cache, &ru).unwrap_err();
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
        assert!(ensure_copy(&app, &cache, &ru).is_err());
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
