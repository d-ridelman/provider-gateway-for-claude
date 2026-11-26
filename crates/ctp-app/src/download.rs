//! Загрузка файлов из сети с проверкой: лимит размера, sha256/sha1, временный файл и
//! замена одним rename, безопасная распаковка zip и подпись Authenticode.
//!
//! Скачанное — недоверенные данные: пока сумма не сошлась, файл называется `*.part`
//! и ничем не запускается; распаковка не выпускает пути за пределы своей папки.

use crate::tr;
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Что ожидается от файла. Без суммы файл всё равно ограничен по размеру.
#[derive(Clone, Debug, Default)]
pub struct Expect {
    pub sha256: Option<String>,
    pub sha1: Option<String>,
    /// Предел в байтах: сервер, отдающий больше, обрывается.
    pub max: u64,
}

/// Прогресс: сколько скачано и сколько всего (если сервер сказал).
pub type Progress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(ctp_gateway::state::USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
}

/// HTTP-клиент для загрузок Claude: через VPN приложения, если он включён
/// (`ctp_gateway::proxy` — mixed-порт ядра mihomo; в TUN это тоже он).
pub fn client() -> Result<reqwest::Client> {
    Ok(ctp_gateway::proxy::apply(builder()).build()?)
}

/// `claude.ai` отдаёт ссылку на установщик Desktop только браузерному User-Agent,
/// со своим — 403 Forbidden.
pub fn client_browser() -> Result<reqwest::Client> {
    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
    Ok(ctp_gateway::proxy::apply(builder().user_agent(UA)).build()?)
}

/// Клиент мимо прокси приложения и системного прокси — для самого ядра mihomo: пока оно
/// не скачано или остановлено для замены, оба указывают на неработающий порт.
pub fn client_direct() -> Result<reqwest::Client> {
    Ok(builder().no_proxy().build()?)
}

/// Перед загрузкой через VPN: включён, а ядро не работает — сказать это, а не «connection
/// refused». Узел не отвечает — к ошибке загрузки добавляется, что она шла через VPN.
pub fn vpn_ready(proxy: Option<&str>, core_running: bool) -> Result<()> {
    if proxy.is_some() && !core_running {
        bail!(
            "{}",
            crate::tr!(
                "VPN включён, но ядро mihomo не работает — загрузка через VPN невозможна. Откройте «Прокси и VPN»: запустите ядро или выключите VPN.",
                "The VPN is on, but the mihomo core is not running, so nothing can be downloaded through it. Open Proxy and VPN: start the core or turn the VPN off."
            )
        );
    }
    Ok(())
}

/// Ошибка загрузки при включённом VPN: с пометкой, что шла через него (узел мог не ответить).
pub fn through_vpn(exc: anyhow::Error) -> anyhow::Error {
    match ctp_gateway::proxy::current() {
        Some(proxy) => exc.context(crate::tr!(
            "загрузка шла через VPN ({proxy}); если узел не отвечает — выберите другой в «Прокси и VPN»",
            "the download went through the VPN ({proxy}); if the node does not answer, pick another one in Proxy and VPN"
        )),
        None => exc,
    }
}

/// Короткий ответ (текст, JSON) с пределом размера.
pub async fn text(client: &reqwest::Client, url: &str, max: u64) -> Result<String> {
    let response = client.get(url).send().await?.error_for_status()?;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        ensure!(
            body.len() as u64 <= max,
            "{url}: answer larger than {max} bytes"
        );
    }
    Ok(String::from_utf8(body)?)
}

/// Скачивает `url` в `dest`: сначала в `dest.part`, после проверки — rename поверх `dest`.
pub async fn to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expect: &Expect,
    progress: Progress<'_>,
) -> Result<()> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let part = part_path(dest);
    let result = fetch(client, url, &part, expect, progress).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        return result;
    }
    // rename поверх существующего файла в Windows работает (MoveFileEx REPLACE_EXISTING).
    tokio::fs::rename(&part, dest)
        .await
        .with_context(|| format!("{} → {}", part.display(), dest.display()))?;
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

async fn fetch(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expect: &Expect,
    progress: Progress<'_>,
) -> Result<()> {
    let response = client.get(url).send().await?.error_for_status()?;
    let total = response.content_length();
    if let Some(total) = total {
        ensure!(
            total <= expect.max,
            "{url}: {total} bytes, the limit is {}",
            expect.max
        );
    }
    let mut file = tokio::fs::File::create(part).await?;
    let mut sha256 = Sha256::new();
    let mut sha1 = Sha1::new();
    let mut done = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        done += chunk.len() as u64;
        ensure!(done <= expect.max, "{url}: more than {} bytes", expect.max);
        sha256.update(&chunk);
        sha1.update(&chunk);
        file.write_all(&chunk).await?;
        progress(done, total);
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    check_digest("sha256", expect.sha256.as_deref(), &hex(&sha256.finalize()))?;
    check_digest("sha1", expect.sha1.as_deref(), &hex(&sha1.finalize()))?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn check_digest(kind: &str, want: Option<&str>, got: &str) -> Result<()> {
    if let Some(want) = want
        && !want.trim().eq_ignore_ascii_case(got)
    {
        bail!("{kind} mismatch: expected {want}, got {got}");
    }
    Ok(())
}

/// sha256 файла на диске (проверка уже лежащего бинаря).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

/// Распаковывает из zip записи, для которых `map` вернул относительный путь, в `dest`.
/// Пути с `..`, абсолютные и прочие выходы за `dest` отклоняются (zip-slip).
/// Возвращает число распакованных файлов.
pub fn unzip(archive: &Path, dest: &Path, map: impl Fn(&str) -> Option<PathBuf>) -> Result<usize> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    let mut count = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let Some(safe) = entry.enclosed_name() else {
            bail!("unsafe path in the archive: {:?}", entry.name());
        };
        let name = safe.to_string_lossy().replace('\\', "/");
        let Some(relative) = map(&name) else {
            continue;
        };
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "unsafe path in the archive: {name}"
        );
        let out = dest.join(&relative);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut target = std::fs::File::create(&out).with_context(|| out.display().to_string())?;
        std::io::copy(&mut entry, &mut target)?;
        count += 1;
    }
    Ok(count)
}

/// Что известно о подписанте файла: имя из CN и отпечаток того, кто выпустил его сертификат.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signer {
    pub name: String,
    pub issuer: Option<String>,
}

/// Подписант файла по Authenticode, если подпись действительна и цепочка доверена.
/// Отзыв проверяется, когда это возможно: за прокси CRL/OCSP часто недоступны (ADR-022).
#[cfg(windows)]
pub fn signer(path: &Path) -> Result<Signer> {
    use windows_sys::Win32::Security::WinTrust::{WTD_REVOKE_NONE, WTD_REVOKE_WHOLECHAIN};
    let (status, found) = verify(path, WTD_REVOKE_WHOLECHAIN);
    let (status, found) = if revocation_unavailable(status) {
        verify(path, WTD_REVOKE_NONE)
    } else {
        (status, found)
    };
    match found {
        Some(signer) => Ok(signer),
        None if status == 0 => bail!("{}: signed, but the signer is unreadable", path.display()),
        None => bail!(
            "{}: no valid Authenticode signature (0x{:08x})",
            path.display(),
            status as u32
        ),
    }
}

/// Отзыв не проверить (CRL/OCSP недоступны): не повод отказывать, подпись и цепочка целы.
#[cfg(windows)]
fn revocation_unavailable(status: i32) -> bool {
    // CRYPT_E_REVOCATION_OFFLINE и CERT_E_REVOCATION_FAILURE.
    matches!(status as u32, 0x8009_2013 | 0x800B_010E)
}

/// Один проход `WinVerifyTrust`: код возврата и данные подписанта, если проверка прошла.
#[cfg(windows)]
fn verify(path: &Path, revocation: u32) -> (i32, Option<Signer>) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Cryptography::{
        CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: revocation,
        dwUnionChoice: WTD_CHOICE_FILE,
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    data.Anonymous.pFile = &mut file;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` и `file` живут до закрытия состояния ниже; `wide` оканчивается нулём.
    let status =
        unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    let found = if status == 0 {
        // SAFETY: после успешной проверки состояние держит данные провайдера; указатели
        // проверяются на null перед разыменованием, буфер имени — нашего размера.
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() {
                std::ptr::null_mut()
            } else {
                WTHelperGetProvSignerFromChain(provider, 0, 0, 0)
            };
            if signer.is_null() || (*signer).csCertChain == 0 {
                None
            } else {
                let chain = (*signer).pasCertChain;
                if chain.is_null() {
                    return (status, None);
                }
                let mut buffer = [0u16; 256];
                let len = CertGetNameStringW(
                    (*chain).pCert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    std::ptr::null(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                );
                // `len` — сколько нужно с нулём на конце; длиннее буфера — взять что есть.
                let name = (len > 1).then(|| {
                    String::from_utf16_lossy(&buffer[..(len as usize - 1).min(buffer.len())])
                });
                // Второй элемент цепочки — тот, кто выпустил листовой сертификат.
                let issuer = ((*signer).csCertChain >= 2)
                    .then(|| thumbprint((*chain.add(1)).pCert))
                    .flatten();
                name.map(|name| Signer { name, issuer })
            }
        }
    } else {
        None
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: закрываем то же состояние той же структурой.
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    (status, found)
}

/// Отпечаток SHA-1 в том же виде, что показывает Windows (`Get-AuthenticodeSignature`).
#[cfg(windows)]
fn thumbprint(
    cert: *const windows_sys::Win32::Security::Cryptography::CERT_CONTEXT,
) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_SHA1_HASH_PROP_ID, CertGetCertificateContextProperty,
    };
    let mut bytes = [0u8; 20];
    let mut size = bytes.len() as u32;
    // SAFETY: буфер нашего размера, и `size` его описывает.
    let ok = unsafe {
        CertGetCertificateContextProperty(
            cert,
            CERT_SHA1_HASH_PROP_ID,
            bytes.as_mut_ptr().cast(),
            &mut size,
        )
    };
    (ok != 0 && size as usize == bytes.len())
        .then(|| bytes.iter().map(|byte| format!("{byte:02X}")).collect())
}

#[cfg(not(windows))]
pub fn signer(path: &Path) -> Result<Signer> {
    bail!("{}: signature checks need Windows", path.display())
}

/// Подписант Claude Desktop и Claude Code (CN сертификата, сверено на обоих 23.11.2026).
pub const ANTHROPIC: &str = "Anthropic, PBC";

/// Удостоверяющий центр Anthropic — DigiCert Trusted G4 Code Signing RSA4096 SHA384 2021
/// CA1. Лист перевыпускается каждый год под тем же центром; цепочка через корпоративный
/// прокси сюда не придёт (ADR-022).
pub const ANTHROPIC_CA: &str = "7B0F360B775F76C94A12CA48445AA2D2A875701C";

/// Файл подписан Anthropic (Claude Desktop, Claude Code) и его сертификат выпущен их центром.
pub fn ensure_anthropic(path: &Path) -> Result<()> {
    check_anthropic(&signer(path)?, &path.display().to_string())
}

/// Решение по данным подписи — без вызовов Windows, чтобы его можно было проверить тестом.
fn check_anthropic(signer: &Signer, shown: &str) -> Result<()> {
    ensure!(
        signer.name == ANTHROPIC,
        "{shown}: signed by {}, not by {ANTHROPIC}",
        signer.name
    );
    let issuer = signer.issuer.as_deref().unwrap_or("?");
    let pinned = ANTHROPIC_CA;
    ensure!(
        signer.issuer.as_deref() == Some(ANTHROPIC_CA),
        "{}",
        tr!(
            "{shown}: сертификат выпущен не центром Anthropic {pinned}, а {issuer} — подмена или корпоративный прокси",
            "{shown}: the certificate was issued by {issuer}, not by Anthropic's CA {pinned} — a substitute or a corporate proxy"
        )
    );
    Ok(())
}

/// То же вне async-воркера: проверка хеширует весь файл (сотни МБ).
pub async fn ensure_anthropic_async(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || ensure_anthropic(&path)).await?
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn quiet(_: u64, _: Option<u64>) {}

    async fn serve(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        server
    }

    fn plain() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn good_sum_lands_and_bad_sum_leaves_nothing() {
        let server = serve(b"hello").await;
        let dir = tempdir();
        let dest = dir.join("x.bin");
        let url = format!("{}/f", server.uri());
        let good = Expect {
            sha256: Some("2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824".into()),
            sha1: Some("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d".into()),
            max: 100,
        };
        to_file(&plain(), &url, &dest, &good, &quiet).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");

        let other = dir.join("y.bin");
        let bad = Expect {
            sha256: Some("00".repeat(32)),
            ..good.clone()
        };
        let err = to_file(&plain(), &url, &other, &bad, &quiet)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        assert!(!other.exists() && !part_path(&other).exists());
        // Неудачная загрузка не трогает уже лежащий файл.
        let err = to_file(&plain(), &url, &dest, &bad, &quiet).await;
        assert!(err.is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn size_limit_cuts_the_download() {
        let server = serve(b"0123456789").await;
        let dir = tempdir();
        let dest = dir.join("x.bin");
        let expect = Expect {
            max: 5,
            ..Default::default()
        };
        let url = format!("{}/f", server.uri());
        assert!(
            to_file(&plain(), &url, &dest, &expect, &quiet)
                .await
                .is_err()
        );
        assert!(!dest.exists());
        assert!(text(&plain(), &url, 5).await.is_err());
        assert_eq!(text(&plain(), &url, 10).await.unwrap(), "0123456789");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unzip_maps_entries_and_rejects_escapes() {
        use std::io::Write;
        let dir = tempdir();
        let archive = dir.join("a.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("lib/net45/claude.exe", options).unwrap();
        zip.write_all(b"exe").unwrap();
        zip.start_file("lib/net45/resources/app.asar", options)
            .unwrap();
        zip.write_all(b"asar").unwrap();
        zip.start_file("other.txt", options).unwrap();
        zip.write_all(b"skip").unwrap();
        zip.finish().unwrap();

        let out = dir.join("out");
        let count = unzip(&archive, &out, |name| {
            name.strip_prefix("lib/net45/").map(PathBuf::from)
        })
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(std::fs::read(out.join("claude.exe")).unwrap(), b"exe");
        assert!(out.join("resources").join("app.asar").is_file());
        assert!(!out.join("other.txt").exists());

        // Карта, выводящая за пределы папки, отклоняется.
        let err = unzip(&archive, &out, |_| Some(PathBuf::from("../evil"))).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Включённый VPN: загрузчик идёт через прокси приложения (здесь — wiremock в роли
    /// прокси), а без ядра загрузка не начинается и говорит почему.
    #[tokio::test]
    async fn downloads_take_the_vpn_when_it_is_on() {
        let proxy = MockServer::start().await;
        Mock::given(path("/claude.exe"))
            .respond_with(ResponseTemplate::new(200).set_body_string("via vpn"))
            .mount(&proxy)
            .await;
        ctp_gateway::proxy::set(Some(proxy.uri()));
        let client = client().unwrap();
        let direct = client_direct().unwrap();
        ctp_gateway::proxy::set(None);
        let url = "http://downloads.example.invalid/claude.exe";
        assert_eq!(text(&client, url, 100).await.unwrap(), "via vpn");
        assert!(
            text(&direct, url, 100).await.is_err(),
            "the core download goes direct"
        );

        let uri = proxy.uri();
        let err = vpn_ready(Some(&uri), false).unwrap_err();
        assert!(err.to_string().contains("mihomo"), "{err}");
        vpn_ready(Some(&uri), true).unwrap();
        vpn_ready(None, false).unwrap();
    }

    #[test]
    fn only_anthropics_name_and_ca_pass() {
        let anthropic = |issuer: Option<&str>| Signer {
            name: ANTHROPIC.to_string(),
            issuer: issuer.map(str::to_string),
        };
        check_anthropic(&anthropic(Some(ANTHROPIC_CA)), "claude.exe").unwrap();
        // Другое имя — не Anthropic, даже с их центром.
        let other = Signer {
            name: "Example, Inc".into(),
            issuer: Some(ANTHROPIC_CA.into()),
        };
        assert!(check_anthropic(&other, "x.exe").is_err());
        // Чужой центр (корпоративный прокси) — не Anthropic, даже с их именем.
        assert!(check_anthropic(&anthropic(Some("AA")), "x.exe").is_err());
        assert!(check_anthropic(&anthropic(None), "x.exe").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn unsigned_file_has_no_signer() {
        let dir = tempdir();
        let file = dir.join("x.exe");
        std::fs::write(&file, b"MZ not really").unwrap();
        assert!(signer(&file).is_err());
        assert!(ensure_anthropic(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn installed_claude_is_signed_by_anthropic() {
        // Только на машине с установленным Claude Desktop; иначе проверять нечего.
        let Some(exe) = crate::claude::detect(&crate::model::Config::default()) else {
            return;
        };
        ensure_anthropic(&exe).unwrap();
    }

    pub(crate) fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("pgc-dl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
