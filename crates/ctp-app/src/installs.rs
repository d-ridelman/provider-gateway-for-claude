//! Скачивание Claude Code и Claude Desktop с официальных адресов Anthropic.
//!
//! Два варианта у каждого: портативно — в папку приложения (`ClaudeProviderGateway`),
//! ничего не ставится в систему (для доменных ПК без прав на установку), и обычная
//! установка — так же, как это делает официальный установщик.
//!
//! Проверка: Claude Code — sha256 из `manifest.json` релиза; Claude Desktop — sha1 из
//! `RELEASES` (так проверяет сам Squirrel); у обоих `claude.exe` — подпись Authenticode
//! Anthropic. Подпись GPG манифеста Claude Code не проверяется (shortcut: нужен
//! OpenPGP-крейт; Authenticode на exe закрывает подмену бинаря — добавить, если
//! Anthropic перестанет подписывать exe).

use crate::download::{self, Expect};
use crate::tr;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};

const CODE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const DESKTOP_RELEASES: &str = "https://downloads.claude.ai/releases/win32";
const DESKTOP_SETUP: &str = "https://claude.ai/api/desktop/win32";
/// Бинарь Claude Code около 400 МБ, nupkg Desktop около 280 МБ, установщик — меньше.
const MAX_CODE: u64 = 1024 * 1024 * 1024;
const MAX_DESKTOP: u64 = 1024 * 1024 * 1024;

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// Что лежит сейчас: для строк в Настройках.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    /// Версия портативного Claude Code (`None` — не скачан).
    pub code_portable: Option<String>,
    /// Портативный Claude Desktop: версия.
    pub desktop_portable: Option<String>,
    /// Путь к его `claude.exe`.
    pub desktop_portable_exe: Option<String>,
    /// Установленный обычным способом Claude Desktop (не портативный), если есть.
    pub desktop_installed: Option<String>,
}

pub fn installed() -> Installed {
    let desktop = portable_desktop();
    Installed {
        desktop_installed: crate::claude::detect(&crate::model::Config::default())
            .filter(|exe| !is_portable_desktop(exe))
            .map(|exe| exe.display().to_string()),
        code_portable: portable_code().and_then(|_| read_version(&code_home()?)),
        desktop_portable: desktop.as_ref().and_then(|exe| version_of_app(exe)),
        desktop_portable_exe: desktop.map(|exe| exe.display().to_string()),
    }
}

// ---------- Claude Code ----------

#[derive(Debug, PartialEq, Eq)]
pub struct CodeRelease {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

/// `manifest.json` релиза Claude Code: сумма и размер для своей платформы.
pub fn parse_code_manifest(text: &str, platform: &str) -> Result<CodeRelease> {
    let json: serde_json::Value = serde_json::from_str(text)?;
    let version = json["version"]
        .as_str()
        .context("manifest without version")?
        .to_string();
    let entry = &json["platforms"][platform];
    let sha256 = entry["checksum"]
        .as_str()
        .with_context(|| format!("manifest has no {platform}"))?
        .to_string();
    ensure!(
        sha256.len() == 64 && sha256.chars().all(|c| c.is_ascii_hexdigit()),
        "bad checksum in the manifest"
    );
    ensure!(valid_version(&version), "bad version in the manifest");
    Ok(CodeRelease {
        version,
        sha256,
        size: entry["size"].as_u64().unwrap_or(0),
    })
}

/// Версия идёт в путь и в URL: непустые числа через точку (`.`, `..` и `6.` не годятся).
fn valid_version(version: &str) -> bool {
    version.len() < 32
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

pub async fn latest_code(client: &reqwest::Client) -> Result<CodeRelease> {
    let version = download::text(client, &format!("{CODE_RELEASES}/latest"), 64).await?;
    let version = version.trim();
    ensure!(valid_version(version), "bad version: {version}");
    let manifest = download::text(
        client,
        &format!("{CODE_RELEASES}/{version}/manifest.json"),
        1024 * 1024,
    )
    .await?;
    parse_code_manifest(&manifest, &format!("win32-{}", arch()))
}

/// Папка портативного Claude Code: `claude.exe`, `version.txt` и `config` (его настройки).
pub fn code_home() -> Option<PathBuf> {
    Some(crate::claude::base_dir()?.join("claude-code"))
}

pub fn portable_code() -> Option<PathBuf> {
    let exe = code_home()?.join("claude.exe");
    exe.is_file().then_some(exe)
}

/// Своя папка настроек портативного Claude Code (`CLAUDE_CONFIG_DIR`): вход, плагины и
/// настройки не смешиваются с установленным Claude Code.
pub fn portable_code_config() -> Option<PathBuf> {
    Some(code_home()?.join("config"))
}

fn read_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("version.txt")).ok()?;
    let text = text.trim();
    valid_version(text).then(|| text.to_string())
}

/// Скачивает последний Claude Code. Портативно — в `code_home`; иначе запускает
/// `claude install` скачанного бинаря (как официальный install.ps1: `~/.local/bin`).
pub async fn get_code(portable: bool, progress: download::Progress<'_>) -> Result<String> {
    // Включён VPN приложения — загрузка идёт через него (`download::client`).
    download::vpn_ready(
        ctp_gateway::proxy::current().as_deref(),
        crate::mihomo::running(),
    )?;
    get_code_with(portable, progress)
        .await
        .map_err(download::through_vpn)
}

async fn get_code_with(portable: bool, progress: download::Progress<'_>) -> Result<String> {
    let client = download::client()?;
    let release = latest_code(&client).await?;
    let home = code_home().context("no AppData\\Local")?;
    let url = format!(
        "{CODE_RELEASES}/{}/win32-{}/claude.exe",
        release.version,
        arch()
    );
    let expect = Expect {
        sha256: Some(release.sha256.clone()),
        sha1: None,
        max: MAX_CODE,
    };
    let version = &release.version;
    if portable {
        let exe = home.join("claude.exe");
        let fresh = home.join("claude.exe.new");
        download::to_file(&client, &url, &fresh, &expect, progress).await?;
        if let Err(exc) = download::ensure_anthropic_async(&fresh).await {
            let _ = std::fs::remove_file(&fresh);
            return Err(exc);
        }
        // Занятый exe (открыта консоль) Windows не даст заменить — скажем об этом прямо.
        let shown = exe.display();
        std::fs::rename(&fresh, &exe).map_err(|exc| {
            let _ = std::fs::remove_file(&fresh);
            anyhow::anyhow!(
                "{}",
                tr!(
                    "не удалось заменить {shown}: {exc}. Закройте консоли Claude Code и повторите.",
                    "could not replace {shown}: {exc}. Close Claude Code consoles and try again."
                )
            )
        })?;
        std::fs::create_dir_all(home.join("config"))?;
        std::fs::write(home.join("version.txt"), version)?;
        return Ok(tr!(
            "Портативный Claude Code {version} готов: {shown}",
            "Portable Claude Code {version} is ready: {shown}"
        ));
    }
    let staging = home.join("downloads");
    let exe = staging.join(format!("claude-{version}.exe"));
    download::to_file(&client, &url, &exe, &expect, progress).await?;
    let result = match download::ensure_anthropic_async(&exe).await {
        Ok(()) => run_hidden(&exe, &["install"]).await,
        Err(exc) => Err(exc),
    };
    let _ = std::fs::remove_dir_all(&staging);
    let output = result?;
    tracing::info!(%version, output = %output.trim(), "Claude Code installed");
    Ok(tr!(
        "Claude Code {version} установлен",
        "Claude Code {version} installed"
    ))
}

/// Программа без окна консоли, с прокси приложения в окружении, в отдельном потоке; ответ —
/// её вывод. Вывод идёт в файл рядом, а не в пайп: установщик Claude сам запускает Claude,
/// тот унаследовал бы пайп, и ожидание конца вывода длилось бы до закрытия Claude.
async fn run_hidden(exe: &Path, args: &[&str]) -> Result<String> {
    use std::process::Stdio;
    let log = exe.with_extension("log");
    let out = std::fs::File::create(&log)?;
    let mut command = std::process::Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        // Секрет браузерного эндпоинта нужен только шлюзу (как в claude::launch).
        .env_remove(crate::fetch::URL_ENV);
    // Настоящая AppData\Local: PGC мог быть запущен из процесса с подменённой.
    if let Some(local) = crate::claude::local_appdata() {
        command.env("LOCALAPPDATA", local);
    }
    for (name, value) in proxy_env() {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let shown = exe.display().to_string();
    tokio::task::spawn_blocking(move || {
        let status = command.status().with_context(|| shown.clone())?;
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        ensure!(status.success(), "{shown}: {status}: {}", text.trim());
        Ok(text)
    })
    .await?
}

/// Переменные прокси для дочерних программ (Claude Code, установщики): пусто, если
/// прокси приложения выключен.
pub fn proxy_env() -> Vec<(String, String)> {
    let Some(url) = ctp_gateway::proxy::current() else {
        return Vec::new();
    };
    // Свой список обхода человека не заменяется, а дополняется.
    let own = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let no_proxy = no_proxy_with(&own);
    ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]
        .into_iter()
        .map(|name| (name.to_string(), url.clone()))
        .chain(
            ["NO_PROXY", "no_proxy"]
                .into_iter()
                .map(|name| (name.to_string(), no_proxy.clone())),
        )
        .collect()
}

fn no_proxy_with(own: &str) -> String {
    let own = own.trim().trim_matches(',');
    if own.is_empty() {
        ctp_gateway::proxy::NO_PROXY.to_string()
    } else {
        format!("{},{own}", ctp_gateway::proxy::NO_PROXY)
    }
}

// ---------- Claude Desktop ----------

#[derive(Debug, PartialEq, Eq)]
pub struct DesktopRelease {
    pub version: String,
    pub file: String,
    pub sha1: String,
    pub size: u64,
}

/// `RELEASES` Squirrel: строки `SHA1 имя.nupkg размер`; берётся полный пакет самой
/// новой версии (дельты не годятся без предыдущей версии).
pub fn parse_releases(text: &str) -> Result<DesktopRelease> {
    let mut best: Option<(Vec<u64>, DesktopRelease)> = None;
    // Файл начинается с BOM UTF-8 (проверено на живом RELEASES).
    for line in text.trim_start_matches('\u{feff}').lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [sha1, file, size] = parts[..] else {
            continue;
        };
        let Some(version) = file
            .strip_prefix("AnthropicClaude-")
            .and_then(|rest| rest.strip_suffix("-full.nupkg"))
        else {
            continue;
        };
        if !valid_version(version)
            || sha1.len() != 40
            || !sha1.chars().all(|c| c.is_ascii_hexdigit())
        {
            continue;
        }
        let key: Vec<u64> = version.split('.').filter_map(|p| p.parse().ok()).collect();
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((
                key,
                DesktopRelease {
                    version: version.to_string(),
                    file: file.to_string(),
                    sha1: sha1.to_string(),
                    size: size.parse().unwrap_or(0),
                },
            ));
        }
    }
    best.map(|(_, r)| r).context("RELEASES has no full package")
}

pub async fn latest_desktop(client: &reqwest::Client) -> Result<DesktopRelease> {
    let text = download::text(
        client,
        &format!("{DESKTOP_RELEASES}/{}/RELEASES", arch()),
        1024 * 1024,
    )
    .await?;
    parse_releases(&text)
}

/// Папка портативного Claude Desktop: `app-<версия>\claude.exe`, как у установленного.
pub fn desktop_home() -> Option<PathBuf> {
    Some(crate::claude::base_dir()?.join("claude-desktop"))
}

/// `claude.exe` самой новой распакованной версии.
pub fn portable_desktop() -> Option<PathBuf> {
    let home = desktop_home()?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in std::fs::read_dir(&home).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(version) = name.strip_prefix("app-") else {
            continue;
        };
        let exe = entry.path().join("claude.exe");
        if !valid_version(version) || !exe.is_file() {
            continue;
        }
        let key: Vec<u64> = version.split('.').filter_map(|p| p.parse().ok()).collect();
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, exe));
        }
    }
    best.map(|(_, exe)| exe)
}

/// Лежит ли этот `claude.exe` в портативной папке.
pub fn is_portable_desktop(exe: &Path) -> bool {
    desktop_home().is_some_and(|home| {
        let home = home.display().to_string().to_ascii_lowercase() + "\\";
        exe.display()
            .to_string()
            .to_ascii_lowercase()
            .starts_with(&home)
    })
}

fn version_of_app(exe: &Path) -> Option<String> {
    let name = exe.parent()?.file_name()?.to_string_lossy().to_string();
    name.strip_prefix("app-").map(str::to_string)
}

/// Итог скачивания Desktop: что сказать и (портативно) путь к новому `claude.exe`.
pub struct DesktopResult {
    pub message: String,
    pub exe: Option<PathBuf>,
}

/// Портативно: полный nupkg → `lib/net45/*` в `app-<версия>`; старые версии убираются,
/// если не заняты. Иначе — официальный установщик (Squirrel ставит в
/// `%LOCALAPPDATA%\AnthropicClaude` и сам открывает Claude в обычном профиле).
pub async fn get_desktop(
    portable: bool,
    progress: download::Progress<'_>,
) -> Result<DesktopResult> {
    download::vpn_ready(
        ctp_gateway::proxy::current().as_deref(),
        crate::mihomo::running(),
    )?;
    get_desktop_with(portable, progress)
        .await
        .map_err(download::through_vpn)
}

async fn get_desktop_with(
    portable: bool,
    progress: download::Progress<'_>,
) -> Result<DesktopResult> {
    let client = download::client()?;
    let home = desktop_home().context("no AppData\\Local")?;
    let staging = home.join("downloads");
    if !portable {
        let setup = staging.join("Claude-Setup.exe");
        let url = format!("{DESKTOP_SETUP}/{}/setup/latest/redirect", arch());
        let expect = Expect {
            max: MAX_DESKTOP,
            ..Default::default()
        };
        download::to_file(&client, &url, &setup, &expect, progress).await?;
        let result = match download::ensure_anthropic_async(&setup).await {
            Ok(()) => run_hidden(&setup, &[]).await,
            Err(exc) => Err(exc),
        };
        let _ = std::fs::remove_dir_all(&staging);
        result?;
        return Ok(DesktopResult {
            message: tr!(
                "Claude Desktop установлен в обычном месте",
                "Claude Desktop installed in the usual place"
            ),
            exe: None,
        });
    }
    let release = latest_desktop(&client).await?;
    let version = release.version.clone();
    let package = staging.join(&release.file);
    let expect = Expect {
        sha256: None,
        sha1: Some(release.sha1.clone()),
        max: MAX_DESKTOP,
    };
    let url = format!("{DESKTOP_RELEASES}/{}/{}", arch(), release.file);
    download::to_file(&client, &url, &package, &expect, progress).await?;
    let target = home.join(format!("app-{version}"));
    let unpacked = tokio::task::spawn_blocking({
        let (package, home, target) = (package.clone(), home.clone(), target.clone());
        move || unpack_desktop(&package, &home, &target)
    })
    .await?;
    let _ = std::fs::remove_dir_all(&staging);
    let exe = unpacked?;
    let shown = exe.display();
    Ok(DesktopResult {
        message: tr!(
            "Портативный Claude Desktop {version} готов: {shown}",
            "Portable Claude Desktop {version} is ready: {shown}"
        ),
        exe: Some(exe),
    })
}

/// Распаковка во временную папку и rename: незаконченная версия не видна `portable_desktop`.
fn unpack_desktop(package: &Path, home: &Path, target: &Path) -> Result<PathBuf> {
    let part = home.join(format!(
        "{}.part",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&part);
    let count = download::unzip(package, &part, |name| {
        name.strip_prefix("lib/net45/").map(PathBuf::from)
    })?;
    let exe = part.join("claude.exe");
    let checked = (|| {
        ensure!(
            count > 10 && exe.is_file() && part.join("resources").is_dir(),
            "the package has no Claude app (lib/net45)"
        );
        download::ensure_anthropic(&exe)
    })();
    if let Err(exc) = checked {
        let _ = std::fs::remove_dir_all(&part);
        return Err(exc);
    }
    if target.exists() {
        // Та же версия уже есть (повторное скачивание): заменить, если не занята.
        if let Err(exc) = discard(home, target) {
            let _ = std::fs::remove_dir_all(&part);
            let shown = target.display();
            bail!(
                "{}",
                tr!(
                    "{shown} занята ({exc}) — закройте портативный Claude и повторите",
                    "{shown} is in use ({exc}); close the portable Claude and try again"
                )
            );
        }
    }
    std::fs::rename(&part, target)?;
    remove_older(home, target);
    Ok(target.join("claude.exe"))
}

/// Убирает папку версии целиком или не трогает вовсе: сначала rename (у папки, где открыт
/// хоть один файл, он не выходит), потом удаление уже не видной никому копии.
fn discard(home: &Path, dir: &Path) -> std::io::Result<()> {
    let trash = home.join(format!("trash-{}", uuid::Uuid::new_v4().simple()));
    std::fs::rename(dir, &trash)?;
    let _ = std::fs::remove_dir_all(&trash);
    Ok(())
}

/// Прошлые версии и недоудалённые остатки: занятая (открытый Claude) остаётся целой.
fn remove_older(home: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(home) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path == keep || !path.is_dir() {
            continue;
        }
        if name.starts_with("trash-") {
            let _ = std::fs::remove_dir_all(&path);
        } else if name.starts_with("app-")
            && let Err(exc) = discard(home, &path)
        {
            tracing::info!(path = %path.display(), error = %exc, "old portable Claude kept");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_manifest_gives_the_platform_checksum() {
        let text = r#"{"version":"2.1.330","platforms":{
            "win32-x64":{"checksum":"fa797c1bc7d4a351aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":387754656},
            "darwin-arm64":{"checksum":"f7c5093b9dd6560daaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1}}}"#;
        let release = parse_code_manifest(text, "win32-x64").unwrap();
        assert_eq!(release.version, "2.1.330");
        assert_eq!(release.size, 387754656);
        assert!(release.sha256.starts_with("fa797c1b"));
        assert!(parse_code_manifest(text, "win32-arm64").is_err());
        // Версия идёт в путь: всё, кроме чисел через точку, отклоняется.
        for evil in ["../../x", "..", ".", "6.", "2..1"] {
            let text = text.replace("2.1.330", evil);
            assert!(parse_code_manifest(&text, "win32-x64").is_err(), "{evil}");
        }
        let short = text.replace("fa797c1bc7d4a351", "zz");
        assert!(parse_code_manifest(&short, "win32-x64").is_err());
    }

    #[test]
    fn releases_pick_the_newest_full_package() {
        let text = "\u{feff}3449DA9CA1E7540C0747309C262C0ED57818A052 AnthropicClaude-6.28899.6-full.nupkg 278585368\n\
            0000000000000000000000000000000000000000 AnthropicClaude-6.28899.6-delta.nupkg 1000\n\
            1111111111111111111111111111111111111111 AnthropicClaude-6.9999.1-full.nupkg 5\n\
            2222222222222222222222222222222222222222 AnthropicClaude-../x-full.nupkg 5\n";
        let release = parse_releases(text).unwrap();
        assert_eq!(release.version, "6.28899.6");
        assert_eq!(release.size, 278585368);
        assert_eq!(release.sha1, "3449DA9CA1E7540C0747309C262C0ED57818A052");
        assert!(parse_releases("garbage").is_err());
    }

    /// Настоящие загрузки (~700 МБ) в папку из `PGC_LIVE_HOME`: `cargo test -p ctp-app
    /// live_portable -- --ignored`. Обычный Claude и Claude Code не трогаются.
    #[tokio::test]
    #[ignore]
    async fn live_portable_downloads() {
        let home = std::env::var("PGC_LIVE_HOME").expect("set PGC_LIVE_HOME");
        // SAFETY: единственный поток теста до любых загрузок; base_dir читает переменную.
        unsafe { std::env::set_var("CLAUDE_GATEWAY_HOME", &home) };
        let quiet = |_: u64, _: Option<u64>| {};
        let text = get_code(true, &quiet).await.unwrap();
        println!("{text}");
        let exe = portable_code().unwrap();
        let version = std::process::Command::new(&exe)
            .arg("--version")
            .env("CLAUDE_CONFIG_DIR", portable_code_config().unwrap())
            .env("DISABLE_AUTOUPDATER", "1")
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&version.stdout));
        assert!(version.status.success());
        let desktop = get_desktop(true, &quiet).await.unwrap();
        println!("{}", desktop.message);
        let exe = desktop.exe.unwrap();
        assert!(is_portable_desktop(&exe));
        assert!(
            exe.parent()
                .unwrap()
                .join("resources")
                .join("app.asar")
                .is_file()
        );
        assert_eq!(portable_desktop().unwrap(), exe);
    }

    #[test]
    fn own_no_proxy_is_kept() {
        assert_eq!(no_proxy_with(" "), ctp_gateway::proxy::NO_PROXY);
        let merged = no_proxy_with("corp.example,gpu-box,");
        assert!(merged.starts_with(ctp_gateway::proxy::NO_PROXY));
        assert!(merged.ends_with(",corp.example,gpu-box"), "{merged}");
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_in_use_is_kept_whole() {
        use std::os::windows::fs::OpenOptionsExt;
        let home = crate::download::tests::tempdir();
        let app = home.join("app-1.0.0");
        std::fs::create_dir_all(app.join("resources")).unwrap();
        std::fs::write(app.join("resources").join("app.asar"), b"x").unwrap();
        std::fs::write(app.join("claude.exe"), b"x").unwrap();
        // Как открытый Claude: файл занят без права удаления.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(app.join("claude.exe"))
            .unwrap();
        assert!(discard(&home, &app).is_err());
        assert!(
            app.join("resources").join("app.asar").is_file(),
            "nothing removed"
        );
        drop(held);
        discard(&home, &app).unwrap();
        assert!(!app.exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
