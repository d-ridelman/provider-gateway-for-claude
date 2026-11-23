//! Ядро mihomo (Clash.Meta): VPN-подписки, выбор узла, замер задержки.
//!
//! Ядро — отдельная программа под GPL-3.0: PGC её не встраивает и не распространяет,
//! а скачивает с GitHub автора закреплённой версии со сверкой sha256, зашитой здесь (новая
//! версия ядра приходит с обновлением PGC, подписанным его ключом). Общение — по REST на
//! 127.0.0.1 с секретом; Go-код в PGC не линкуется.
//!
//! Конфиг строит PGC: подписки — `proxy-providers` (base64 v2ray-ссылки и YAML Clash ядро
//! разбирает само), у каждой префикс `<id>|`, чтобы одинаковые имена узлов разных подписок
//! не путались; группы `PGC` (выбор) и `AUTO` (самый быстрый); правила — частные сети
//! напрямую, остальное в `PGC`. Geo-базы не нужны и не качаются (проверено на 1.19.34).
//!
//! Режимы: `app` и `system` — ядро обычным процессом в Job Object (умирает вместе с PGC);
//! `tun` — ядро с правами администратора через UAC под присмотром повышенного экземпляра
//! PGC (`--mihomo-supervisor`), который гасит его, когда PGC закрывается или просит.

use crate::download::{self, Expect};
use crate::model::{Config, Subscription};
use crate::tr;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const VERSION: &str = "v1.19.34";
/// Ассет и его sha256 из релиза v1.19.34 (GitHub API `digest`, сверено скачиванием).
#[cfg(not(target_arch = "aarch64"))]
const ASSET: (&str, &str) = (
    "mihomo-windows-amd64-compatible-v1.19.34.zip",
    "cd6738d4648cc1a5f48049a1343b697ab510889d648f045c5c780fc1ab443a79",
);
#[cfg(target_arch = "aarch64")]
const ASSET: (&str, &str) = (
    "mihomo-windows-arm64-v1.19.34.zip",
    "076cb04a21377675bcf009b8d1dd83006ce2464aa537e37bce1faf6d3504f554",
);
const MAX_CORE: u64 = 128 * 1024 * 1024;
/// Главная группа (её выбор — выбранный узел) и группа «самый быстрый».
pub const MAIN: &str = "PGC";
pub const AUTO: &str = "AUTO";
const TEST_URL: &str = "https://www.gstatic.com/generate_204";
/// Частные сети и сам компьютер — всегда напрямую (интранет, локальные модели).
const DIRECT_NETS: [&str; 6] = [
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "100.64.0.0/10",
];

/// Папка ядра: `bin\mihomo.exe`, `bin\version.txt`, `home\` (конфиг, кэш подписок, выбор).
pub fn dir() -> Option<PathBuf> {
    Some(crate::claude::base_dir()?.join("mihomo"))
}

fn exe() -> Option<PathBuf> {
    Some(dir()?.join("bin").join("mihomo.exe"))
}

/// Скачанная версия ядра, если файл на месте (антивирус мог убрать его в карантин).
pub fn installed_version() -> Option<String> {
    let bin = dir()?.join("bin");
    if !bin.join("mihomo.exe").is_file() {
        return None;
    }
    let text = std::fs::read_to_string(bin.join("version.txt")).ok()?;
    Some(text.trim().to_string())
}

/// Скачивает ядро закреплённой версии и сверяет sha256 архива.
pub async fn download_core(progress: download::Progress<'_>) -> Result<String> {
    let dir = dir().context("no AppData\\Local")?;
    let bin = dir.join("bin");
    let staging = dir.join("downloads");
    let (asset, sha256) = ASSET;
    let url = format!("https://github.com/MetaCubeX/mihomo/releases/download/{VERSION}/{asset}");
    let archive = staging.join(asset);
    let expect = Expect {
        sha256: Some(sha256.into()),
        sha1: None,
        max: MAX_CORE,
    };
    download::to_file(&download::client()?, &url, &archive, &expect, progress).await?;
    let fresh = staging.join("unpacked");
    let _ = std::fs::remove_dir_all(&fresh);
    // В архиве один exe (`mihomo-windows-amd64-compatible.exe`) — кладём его под своим именем.
    let count = download::unzip(&archive, &fresh, |name| {
        name.ends_with(".exe").then(|| PathBuf::from("mihomo.exe"))
    })?;
    ensure!(count == 1, "unexpected mihomo archive ({count} exe files)");
    std::fs::create_dir_all(&bin)?;
    let target = bin.join("mihomo.exe");
    std::fs::rename(fresh.join("mihomo.exe"), &target).with_context(|| {
        tr!(
            "не удалось заменить ядро — остановите прокси и повторите",
            "could not replace the core; stop the proxy and try again"
        )
    })?;
    std::fs::write(bin.join("version.txt"), VERSION)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(tr!(
        "Ядро mihomo {VERSION} скачано",
        "mihomo core {VERSION} downloaded"
    ))
}

/// Адрес подписки — секрет: ключ в Credential Manager отдельно от ключей провайдеров.
pub fn sub_key(id: &str) -> String {
    format!("subscription:{id}")
}

/// Хост адреса подписки: всё, что показывается человеку или пишется в журнал.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    // user:pass@host — без учётных данных.
    host.rsplit('@').next().unwrap_or("").to_string()
}

/// Заменяет известные адреса подписок их хостами (ошибки ядра цитируют URL с токеном).
pub fn redact(text: &str, urls: &[String]) -> String {
    let mut out = text.to_string();
    for url in urls.iter().filter(|u| !u.is_empty()) {
        out = out.replace(url.as_str(), &format!("{}/…", host_of(url)));
    }
    out
}

/// Конфиг ядра. JSON — подмножество YAML, ядро читает его как `config.yaml`.
pub fn build_config(
    subs: &[(Subscription, String)],
    mixed: u16,
    controller: u16,
    secret: &str,
    tun: bool,
) -> Value {
    let mut providers = serde_json::Map::new();
    for (sub, url) in subs {
        providers.insert(
            sub.id.clone(),
            json!({
                "type": "http",
                "url": url,
                "interval": sub.interval_hours.max(1) * 3600,
                "path": format!("./providers/{}.yaml", sub.id),
                "override": { "additional-prefix": format!("{}|", sub.id) },
                "health-check": { "enable": true, "url": TEST_URL, "interval": 600, "lazy": true },
            }),
        );
    }
    let used: Vec<&str> = subs.iter().map(|(s, _)| s.id.as_str()).collect();
    let groups = if used.is_empty() {
        // Без подписок ядро не примет пустую группу: напрямую, пока подписку не добавят.
        json!([{ "name": MAIN, "type": "select", "proxies": ["DIRECT"] }])
    } else {
        json!([
            { "name": AUTO, "type": "url-test", "use": used, "url": TEST_URL, "interval": 600, "tolerance": 50 },
            { "name": MAIN, "type": "select", "proxies": [AUTO], "use": used },
        ])
    };
    let mut rules: Vec<String> = DIRECT_NETS
        .iter()
        .map(|net| format!("IP-CIDR,{net},DIRECT,no-resolve"))
        .collect();
    rules.push(format!("MATCH,{MAIN}"));
    let mut config = json!({
        "mixed-port": mixed,
        "bind-address": "127.0.0.1",
        "allow-lan": false,
        "mode": "rule",
        "log-level": "info",
        "ipv6": false,
        "find-process-mode": "off",
        "geo-auto-update": false,
        "external-controller": format!("127.0.0.1:{controller}"),
        "secret": secret,
        "profile": { "store-selected": true },
        "proxy-providers": providers,
        "proxy-groups": groups,
        "rules": rules,
    });
    if tun {
        config["tun"] = json!({
            "enable": true,
            "stack": "mixed",
            "auto-route": true,
            "auto-detect-interface": true,
            "dns-hijack": ["any:53"],
            "route-exclude-address": DIRECT_NETS,
        });
        // Своё DNS: запросы идут через выбранный узел (respect-rules), домены локальной
        // сети и проверки сети Windows — без подмены адресов.
        config["dns"] = json!({
            "enable": true,
            "ipv6": false,
            "enhanced-mode": "fake-ip",
            "fake-ip-range": "198.18.0.1/16",
            "fake-ip-filter": ["+.lan", "+.local", "+.localdomain", "+.home.arpa", "+.msftconnecttest.com", "+.msftncsi.com"],
            "respect-rules": true,
            "default-nameserver": ["1.1.1.1", "8.8.8.8"],
            "proxy-server-nameserver": ["1.1.1.1", "8.8.8.8"],
            "nameserver": ["https://1.1.1.1/dns-query", "https://dns.google/dns-query"],
        });
    }
    config
}

/// Свободный порт на 127.0.0.1 (окно между проверкой и запуском ядра невелико).
pub fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

fn port_free(port: u16) -> bool {
    port != 0 && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

// ---------- режим целиком ----------

fn proxy_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Одно применение режима за раз (старт приложения и окно могут совпасть).
static APPLYING: Mutex<()> = Mutex::new(());

/// Приводит ядро, системный прокси и исходящий прокси шлюза к настройкам. Ответ
/// `Some(порт)` — mixed-порт пришлось сменить (прежний занят), его надо сохранить.
///
/// При ошибке ядра прокси шлюза остаётся на его порту: запросы падают с понятной ошибкой,
/// а не уходят напрямую мимо VPN, которого человек ждёт.
pub fn apply(config: &Config) -> Result<Option<u16>> {
    let _one = APPLYING.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(exc) = crate::sysproxy::disable() {
        tracing::warn!(error = %format!("{exc:#}"), "system proxy not restored");
    }
    if !config.proxy.is_on() {
        stop();
        set_error(None);
        ctp_gateway::proxy::set(None);
        return Ok(None);
    }
    if config.proxy.port != 0 {
        ctp_gateway::proxy::set(Some(proxy_url(config.proxy.port)));
    }
    let changed = start(config).inspect_err(|exc| set_error(Some(format!("{exc:#}"))))?;
    let port = changed.unwrap_or(config.proxy.port);
    ctp_gateway::proxy::set(Some(proxy_url(port)));
    if config.proxy.mode == "system" {
        crate::sysproxy::enable(port)?;
    }
    Ok(changed)
}

/// Выход из приложения: ядро останавливается, системный прокси возвращается.
pub fn shutdown() {
    stop();
    if let Err(exc) = crate::sysproxy::disable() {
        tracing::warn!(error = %format!("{exc:#}"), "system proxy not restored");
    }
}

// ---------- процесс ядра ----------

struct Core {
    api: String,
    secret: String,
    tun: bool,
    /// Обычный процесс в Job Object или повышенный надсмотрщик.
    process: win::Running,
    /// Адреса подписок этого запуска — для `redact`.
    urls: Vec<String>,
}

static CORE: Mutex<Option<Core>> = Mutex::new(None);
/// Почему ядро не работает, хотя должно (упало, не скачано, отказ UAC).
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn set_error(text: Option<String>) {
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = text;
}

pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Адреса подписок из Credential Manager; подписка без адреса пропускается.
fn subscriptions(config: &Config) -> Vec<(Subscription, String)> {
    config
        .proxy
        .subscriptions
        .iter()
        .filter_map(|sub| Some((sub.clone(), crate::store::get_key(&sub_key(&sub.id))?)))
        .collect()
}

/// Запускает ядро по настройкам (`mode` не `off`). Порт mixed берётся из настроек, а занятый
/// заменяется свободным — тогда ответ `Some(новый порт)`, и его надо сохранить.
pub fn start(config: &Config) -> Result<Option<u16>> {
    stop();
    let exe = exe().context("no AppData\\Local")?;
    if !exe.is_file() {
        bail!(
            "{}",
            tr!(
                "ядро mihomo не скачано (или убрано антивирусом) — скачайте его в разделе «Прокси»",
                "the mihomo core is not downloaded (or was removed by an antivirus); download it in the Proxy section"
            )
        );
    }
    let home = dir().context("no AppData\\Local")?.join("home");
    std::fs::create_dir_all(&home)?;
    let tun = config.proxy.mode == "tun";
    let mut changed = None;
    let mut mixed = config.proxy.port;
    if !port_free(mixed) {
        mixed = free_port()?;
        changed = Some(mixed);
    }
    let controller = free_port()?;
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let subs = subscriptions(config);
    let urls: Vec<String> = subs.iter().map(|(_, url)| url.clone()).collect();
    let value = build_config(&subs, mixed, controller, &secret, tun);
    let path = home.join("config.yaml");
    std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    let log = dir().context("no AppData\\Local")?.join("mihomo.log");
    let process = if tun {
        win::start_elevated(&exe, &home, &path, &log)?
    } else {
        win::start_in_job(&exe, &home, &path, &log)?
    };
    let api = format!("http://127.0.0.1:{controller}");
    *CORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Core {
        api: api.clone(),
        secret: secret.clone(),
        tun,
        process,
        urls,
    });
    // Ждём REST: ядро поднимается за доли секунды; UAC в TUN — пока человек не ответит.
    let wait = if tun { 120 } else { 15 };
    let client = blocking_client();
    for _ in 0..wait * 5 {
        if !running() {
            let tail = log_tail(&log);
            set_error(Some(tr!(
                "ядро mihomo остановилось при запуске: {tail}",
                "the mihomo core stopped while starting: {tail}"
            )));
            stop();
            bail!("{}", last_error().unwrap_or_default());
        }
        let ok = client
            .get(format!("{api}/version"))
            .bearer_auth(&secret)
            .send()
            .is_ok_and(|r| r.status().is_success());
        if ok {
            set_error(None);
            tracing::info!(mixed, tun, "mihomo core started");
            return Ok(changed);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stop();
    bail!(
        "{}",
        tr!(
            "ядро mihomo не ответило вовремя",
            "the mihomo core did not answer in time"
        )
    )
}

pub fn stop() {
    if let Some(core) = CORE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        core.process.stop();
        tracing::info!(tun = core.tun, "mihomo core stopped");
    }
}

/// Работает ли ядро (процесс жив).
pub fn running() -> bool {
    CORE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|core| core.process.alive())
}

pub fn is_tun() -> bool {
    CORE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|core| core.tun)
}

fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let last: Vec<&str> = text.lines().rev().take(3).collect();
    last.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

fn blocking_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client")
}

/// Новый конфиг работающему ядру без перезапуска (в TUN перезапуск — это снова UAC).
pub fn reload(config: &Config) -> Result<()> {
    let (api, secret, tun, path) = {
        let guard = CORE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(core) = guard.as_ref() else {
            return Ok(());
        };
        let home = dir().context("no AppData\\Local")?.join("home");
        (
            core.api.clone(),
            core.secret.clone(),
            core.tun,
            home.join("config.yaml"),
        )
    };
    let controller: u16 = api.rsplit(':').next().unwrap_or("0").parse()?;
    let old: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mixed = old["mixed-port"].as_u64().unwrap_or(0) as u16;
    let subs = subscriptions(config);
    let urls: Vec<String> = subs.iter().map(|(_, url)| url.clone()).collect();
    let value = build_config(&subs, mixed, controller, &secret, tun);
    std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    let response = blocking_client()
        .put(format!("{api}/configs?force=true"))
        .bearer_auth(&secret)
        .json(&json!({ "path": path.display().to_string() }))
        .timeout(Duration::from_secs(60))
        .send()?;
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if let Some(core) = CORE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        core.urls = urls.clone();
    }
    ensure!(status.is_success(), "{}", redact(&body, &urls));
    Ok(())
}

// ---------- REST для окна ----------

/// Узел подписки для окна.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// Имя в ядре (`<подписка>|<узел>`) — его шлёт выбор.
    pub name: String,
    /// Имя без префикса подписки.
    pub label: String,
    pub sub: String,
    /// Тип: vless, ss, trojan…
    pub kind: String,
    /// Последняя задержка в мс; `Some(0)` — не ответил; `None` — не мерили.
    pub delay: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubState {
    pub id: String,
    pub name: String,
    pub host: String,
    pub count: usize,
    pub updated_at: Option<String>,
    pub upload: Option<u64>,
    pub download: Option<u64>,
    pub total: Option<u64>,
    /// Конец подписки, секунды Unix (0 или нет — бессрочно).
    pub expire: Option<u64>,
}

/// Всё о прокси для окна.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyState {
    pub mode: String,
    pub running: bool,
    pub tun: bool,
    pub port: u16,
    pub core_version: Option<String>,
    pub pinned_version: String,
    /// Выбранный в `PGC`: имя узла или `AUTO`.
    pub selected: Option<String>,
    /// Что выбрал `AUTO` сейчас.
    pub auto_now: Option<String>,
    pub nodes: Vec<Node>,
    pub subs: Vec<SubState>,
    pub error: Option<String>,
    /// Системный прокси: включён ли он сейчас нами, или почему не вышло.
    pub system: Option<crate::sysproxy::Status>,
}

fn api() -> Option<(String, String, Vec<String>)> {
    let guard = CORE.lock().unwrap_or_else(|e| e.into_inner());
    let core = guard.as_ref()?;
    Some((core.api.clone(), core.secret.clone(), core.urls.clone()))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client")
}

async fn get(path: &str) -> Result<Value> {
    let (api, secret, urls) = api().context("core is not running")?;
    let response = client()
        .get(format!("{api}{path}"))
        .bearer_auth(secret)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    ensure!(status.is_success(), "{status}: {}", redact(&text, &urls));
    Ok(serde_json::from_str(&text)?)
}

async fn put(path: &str, body: Value) -> Result<()> {
    let (api, secret, urls) = api().context("core is not running")?;
    let response = client()
        .put(format!("{api}{path}"))
        .bearer_auth(secret)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    ensure!(status.is_success(), "{status}: {}", redact(&text, &urls));
    Ok(())
}

/// Задержка из истории замеров ядра (последний замер).
fn last_delay(proxy: &Value) -> Option<u32> {
    proxy["history"]
        .as_array()?
        .last()
        .and_then(|h| h["delay"].as_u64())
        .map(|d| d as u32)
}

/// Состояние для окна из ответов `/providers/proxies` и `/proxies`.
pub fn parse_state(providers: &Value, proxies: &Value, config: &Config) -> ProxyState {
    let mut state = ProxyState::default();
    let all = &proxies["proxies"];
    state.selected = all[MAIN]["now"].as_str().map(str::to_string);
    state.auto_now = all[AUTO]["now"].as_str().map(str::to_string);
    for sub in &config.proxy.subscriptions {
        let provider = &providers["providers"][&sub.id];
        let list = provider["proxies"].as_array().cloned().unwrap_or_default();
        let prefix = format!("{}|", sub.id);
        for proxy in &list {
            let name = proxy["name"].as_str().unwrap_or_default().to_string();
            state.nodes.push(Node {
                label: name.strip_prefix(&prefix).unwrap_or(&name).to_string(),
                sub: sub.id.clone(),
                kind: proxy["type"].as_str().unwrap_or_default().to_lowercase(),
                // У `/proxies` история свежее (замер группы), у провайдера — своя.
                delay: last_delay(&all[&name]).or_else(|| last_delay(proxy)),
                name,
            });
        }
        let info = &provider["subscriptionInfo"];
        state.subs.push(SubState {
            id: sub.id.clone(),
            name: sub.name.clone(),
            host: crate::store::get_key(&sub_key(&sub.id))
                .map(|url| host_of(&url))
                .unwrap_or_default(),
            count: list.len(),
            updated_at: provider["updatedAt"].as_str().map(str::to_string),
            upload: info["Upload"].as_u64(),
            download: info["Download"].as_u64(),
            total: info["Total"].as_u64(),
            expire: info["Expire"].as_u64().filter(|e| *e > 0),
        });
    }
    state
}

pub async fn state(config: &Config) -> ProxyState {
    let mut state = if running() {
        match (get("/providers/proxies").await, get("/proxies").await) {
            (Ok(providers), Ok(proxies)) => parse_state(&providers, &proxies, config),
            (Err(exc), _) | (_, Err(exc)) => ProxyState {
                error: Some(format!("{exc:#}")),
                ..parse_state(&Value::Null, &Value::Null, config)
            },
        }
    } else {
        let mut state = parse_state(&Value::Null, &Value::Null, config);
        // Режим включён, а ядра нет: упало или не запускалось — шлюз без VPN не ходит.
        state.error = last_error().or_else(|| {
            config.proxy.is_on().then(|| {
                tr!(
                    "ядро mihomo не работает — запросы через прокси не проходят",
                    "the mihomo core is not running; requests through the proxy fail"
                )
            })
        });
        state
    };
    state.mode = if config.proxy.is_on() {
        config.proxy.mode.clone()
    } else {
        "off".into()
    };
    state.running = running();
    state.tun = is_tun();
    state.port = config.proxy.port;
    state.core_version = installed_version();
    state.pinned_version = VERSION.into();
    state.system = crate::sysproxy::status();
    state
}

/// Выбор узла (или `AUTO`) в главной группе; ядро помнит его между запусками.
pub async fn select(name: &str) -> Result<()> {
    put(&format!("/proxies/{MAIN}"), json!({ "name": name })).await
}

/// Замер задержки всех узлов главной группы. Ответ 504 — ни один не ответил (не ошибка).
pub async fn measure() -> Result<()> {
    let path = format!(
        "/group/{MAIN}/delay?url={}&timeout=5000",
        TEST_URL.replace(':', "%3A").replace('/', "%2F")
    );
    match get(&path).await {
        Ok(_) => Ok(()),
        Err(exc) if exc.to_string().starts_with("504") => Ok(()),
        Err(exc) => Err(exc),
    }
}

/// Обновить подписку сейчас (ядро скачивает её заново).
pub async fn update(id: &str) -> Result<()> {
    put(&format!("/providers/proxies/{id}"), Value::Null).await
}

// ---------- Windows: Job Object и повышенный надсмотрщик ----------

#[cfg(windows)]
mod win {
    use anyhow::{Context, Result, bail};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcessId, OpenEventW, OpenProcess,
        PROCESS_SYNCHRONIZE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    const SYNCHRONIZE: u32 = 0x0010_0000;

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain([0])
            .collect()
    }

    /// Владение дескриптором Windows: закрывается при drop.
    pub struct Owned(HANDLE);
    // SAFETY: дескрипторы ядра Windows можно использовать и закрывать из любого потока.
    unsafe impl Send for Owned {}

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: дескриптор наш и закрывается один раз.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// Job Object, который убивает свои процессы, когда закрыт последний его дескриптор
    /// (в том числе когда процесс-владелец умер).
    fn kill_on_close_job() -> Result<Owned> {
        // SAFETY: создаём безымянный объект; структура лимитов — наша, нужного размера.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                bail!("CreateJobObject: {}", std::io::Error::last_os_error());
            }
            let job = Owned(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                bail!(
                    "SetInformationJobObject: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(job)
        }
    }

    fn spawn_core(
        exe: &Path,
        home: &Path,
        config: &Path,
        log: &Path,
    ) -> Result<(std::process::Child, Owned)> {
        use std::os::windows::process::CommandExt;
        let job = kill_on_close_job()?;
        let out = std::fs::File::create(log)?;
        let child = std::process::Command::new(exe)
            .arg("-d")
            .arg(home)
            .arg("-f")
            .arg(config)
            .stdin(std::process::Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .with_context(|| exe.display().to_string())?;
        // SAFETY: оба дескриптора живы: job наш, процесс — только что запущенный ребёнок.
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) } == 0 {
            let error = std::io::Error::last_os_error();
            let mut child = child;
            let _ = child.kill();
            bail!("AssignProcessToJobObject: {error}");
        }
        Ok((child, job))
    }

    pub enum Running {
        /// Ядро — наш ребёнок в Job Object.
        Child(std::process::Child, #[allow(dead_code)] Owned),
        /// Повышенный надсмотрщик и событие «остановись».
        Elevated { process: Owned, stop: Owned },
    }

    impl Running {
        pub fn alive(&self) -> bool {
            match self {
                // `try_wait` требует `&mut`; ожидание с нулевым сроком — то же без него.
                // SAFETY: дескриптор процесса жив, пока жив `Child`.
                Running::Child(child, _) => unsafe {
                    WaitForSingleObject(child.as_raw_handle() as HANDLE, 0) == WAIT_TIMEOUT
                },
                // SAFETY: дескриптор наш.
                Running::Elevated { process, .. } => unsafe {
                    WaitForSingleObject(process.0, 0) == WAIT_TIMEOUT
                },
            }
        }

        pub fn stop(self) {
            match self {
                Running::Child(mut child, job) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(job);
                }
                Running::Elevated { process, stop } => {
                    // SAFETY: оба дескриптора наши.
                    unsafe {
                        SetEvent(stop.0);
                        WaitForSingleObject(process.0, 10_000);
                    }
                }
            }
        }
    }

    pub fn start_in_job(exe: &Path, home: &Path, config: &Path, log: &Path) -> Result<Running> {
        let (child, job) = spawn_core(exe, home, config, log)?;
        Ok(Running::Child(child, job))
    }

    /// Аргумент командной строки в кавычках по правилам CommandLineToArgvW: обратные
    /// слеши перед кавычкой удваиваются.
    pub fn quote(arg: &str) -> String {
        let mut out = String::from("\"");
        let mut slashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.push_str(&"\\".repeat(slashes * 2 + 1));
                    out.push('"');
                    slashes = 0;
                    continue;
                }
                _ => {}
            }
            if c != '\\' {
                out.push_str(&"\\".repeat(slashes));
                slashes = 0;
                out.push(c);
            }
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        out
    }

    /// Ядро с правами администратора: этот же exe через UAC в роли надсмотрщика.
    pub fn start_elevated(exe: &Path, home: &Path, config: &Path, log: &Path) -> Result<Running> {
        // SAFETY: GetCurrentProcessId без побочных эффектов.
        let pid = unsafe { GetCurrentProcessId() };
        let name = format!("Local\\pgc-mihomo-stop-{}", uuid::Uuid::new_v4().simple());
        let wname = wide(&name);
        // SAFETY: имя оканчивается нулём; событие с ручным сбросом, изначально не взведено.
        let stop = unsafe { CreateEventW(std::ptr::null(), 1, 0, wname.as_ptr()) };
        if stop.is_null() {
            bail!("CreateEvent: {}", std::io::Error::last_os_error());
        }
        let stop = Owned(stop);
        let me = std::env::current_exe()?;
        let params = [
            "--mihomo-supervisor".to_string(),
            pid.to_string(),
            name,
            exe.display().to_string(),
            home.display().to_string(),
            config.display().to_string(),
            log.display().to_string(),
        ]
        .iter()
        .map(|a| quote(a))
        .collect::<Vec<_>>()
        .join(" ");
        let verb = wide("runas");
        let file = wide(&me.display().to_string());
        let params = wide(&params);
        // SAFETY: строки живут до конца вызова; структура нулевая, кроме заданных полей.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = SW_HIDE;
            if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
                let exc = std::io::Error::last_os_error();
                bail!(
                    "{}",
                    crate::tr!(
                        "TUN не запущен: нужны права администратора ({exc})",
                        "TUN not started: administrator rights are needed ({exc})"
                    )
                );
            }
            Ok(Running::Elevated {
                process: Owned(info.hProcess),
                stop,
            })
        }
    }

    /// Роль надсмотрщика (повышенный процесс): ядро в Job Object, пока жив PGC и не взведено
    /// событие остановки. Возвращает код выхода.
    pub fn supervise(args: &[String]) -> i32 {
        let [pid, event, exe, home, config, log] = args else {
            return 2;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            return 2;
        };
        // SAFETY: открываем по PID и имени только на ожидание; null проверяется.
        let (parent, stop) = unsafe {
            (
                OpenProcess(PROCESS_SYNCHRONIZE, 0, pid),
                OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, 0, wide(event).as_ptr()),
            )
        };
        if parent.is_null() || stop.is_null() {
            return 3;
        }
        let (parent, stop) = (Owned(parent), Owned(stop));
        let Ok((child, _job)) = spawn_core(
            Path::new(exe),
            Path::new(home),
            Path::new(config),
            Path::new(log),
        ) else {
            return 4;
        };
        let handles = [parent.0, stop.0, child.as_raw_handle() as HANDLE];
        // SAFETY: три живых дескриптора; ждём любой.
        let which = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, u32::MAX) };
        // Выход закрывает job — ядро умирает вместе с надсмотрщиком.
        if which == WAIT_OBJECT_0 + 2 { 5 } else { 0 }
    }
}

#[cfg(not(windows))]
mod win {
    use anyhow::{Result, bail};
    use std::path::Path;
    pub enum Running {}
    impl Running {
        pub fn alive(&self) -> bool {
            match *self {}
        }
        pub fn stop(self) {}
    }
    pub fn start_in_job(_: &Path, _: &Path, _: &Path, _: &Path) -> Result<Running> {
        bail!("Windows only")
    }
    pub fn start_elevated(_: &Path, _: &Path, _: &Path, _: &Path) -> Result<Running> {
        bail!("Windows only")
    }
    pub fn supervise(_: &[String]) -> i32 {
        1
    }
}

/// Точка входа повышенного экземпляра: `main` зовёт до запуска окна.
pub fn supervise(args: &[String]) -> i32 {
    win::supervise(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str) -> Subscription {
        Subscription {
            id: id.into(),
            name: id.to_uppercase(),
            interval_hours: 6,
        }
    }

    #[test]
    fn config_has_providers_groups_and_rules() {
        let subs = vec![
            (
                sub("s1"),
                "https://panel.example/sub?token=SECRET".to_string(),
            ),
            (sub("s2"), "https://other.example/x".to_string()),
        ];
        let config = build_config(&subs, 7890, 9090, "k", false);
        assert_eq!(config["mixed-port"], 7890);
        assert_eq!(config["external-controller"], "127.0.0.1:9090");
        assert_eq!(config["bind-address"], "127.0.0.1");
        assert_eq!(config["allow-lan"], false);
        let s1 = &config["proxy-providers"]["s1"];
        assert_eq!(s1["interval"], 6 * 3600);
        assert_eq!(s1["override"]["additional-prefix"], "s1|");
        assert_eq!(config["proxy-groups"][1]["name"], MAIN);
        assert_eq!(config["proxy-groups"][1]["proxies"][0], AUTO);
        assert_eq!(config["proxy-groups"][1]["use"][1], "s2");
        let rules = config["rules"].as_array().unwrap();
        assert_eq!(rules.last().unwrap(), &json!(format!("MATCH,{MAIN}")));
        assert!(
            rules
                .iter()
                .any(|r| r == "IP-CIDR,192.168.0.0/16,DIRECT,no-resolve")
        );
        assert!(config.get("tun").is_none() && config.get("dns").is_none());

        let tun = build_config(&subs, 7890, 9090, "k", true);
        assert_eq!(tun["tun"]["enable"], true);
        assert_eq!(tun["dns"]["enable"], true);

        // Без подписок — одна группа напрямую: ядро не примет пустую.
        let empty = build_config(&[], 7890, 9090, "k", false);
        assert_eq!(empty["proxy-groups"][0]["proxies"][0], "DIRECT");
        assert_eq!(empty["proxy-groups"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn subscription_urls_show_only_their_host() {
        let url = "https://user:pass@panel.example:8443/api/sub/abc?token=SECRET#x";
        assert_eq!(host_of(url), "panel.example:8443");
        let text = format!("fetch {url} failed");
        let clean = redact(&text, &[url.to_string()]);
        assert!(
            !clean.contains("SECRET") && !clean.contains("pass"),
            "{clean}"
        );
        assert!(clean.contains("panel.example:8443/…"));
    }

    #[test]
    fn state_strips_prefixes_and_reads_delays_and_traffic() {
        let providers = json!({"providers": {"s1": {
            "proxies": [
                {"name": "s1|Netherlands A", "type": "Ss", "history": [{"delay": 0}]},
                {"name": "s1|Germany B", "type": "Vless", "history": []},
            ],
            "updatedAt": "2026-11-23T14:05:32+04:00",
            "subscriptionInfo": {"Upload": 1, "Download": 2, "Total": 10, "Expire": 0},
        }}});
        let proxies = json!({"proxies": {
            "PGC": {"now": "s1|Germany B"},
            "AUTO": {"now": "s1|Netherlands A"},
            "s1|Germany B": {"history": [{"delay": 87}]},
        }});
        let mut config = Config::default();
        config.proxy.subscriptions = vec![sub("s1")];
        let state = parse_state(&providers, &proxies, &config);
        assert_eq!(state.selected.as_deref(), Some("s1|Germany B"));
        assert_eq!(state.auto_now.as_deref(), Some("s1|Netherlands A"));
        assert_eq!(state.nodes[0].label, "Netherlands A");
        assert_eq!(state.nodes[0].delay, Some(0));
        assert_eq!(state.nodes[0].kind, "ss");
        assert_eq!(state.nodes[1].delay, Some(87));
        assert_eq!(state.subs[0].count, 2);
        assert_eq!(state.subs[0].total, Some(10));
        assert_eq!(state.subs[0].expire, None);
    }

    /// Настоящее ядро (~22 МБ с GitHub) на выдуманной base64-подписке, в папке
    /// `PGC_LIVE_HOME` и со своим именем службы ключей: `cargo test -p ctp-app live_core --
    /// --ignored`. Режим `app`: ни системного прокси, ни TUN.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_core_with_a_fake_subscription() {
        use base64::Engine;
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let home = std::env::var("PGC_LIVE_HOME").expect("set PGC_LIVE_HOME");
        // SAFETY: до любых потоков, читающих окружение.
        unsafe { std::env::set_var("CLAUDE_GATEWAY_HOME", &home) };
        let service = format!("pgc-live-test-{}", uuid::Uuid::new_v4().simple());
        crate::store::use_key_service(service);

        let links = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ@203.0.113.10:8388#Netherlands%20A\n\
            trojan://secret@203.0.113.12:443?sni=example.com#Finland%20C";
        let body = base64::engine::general_purpose::STANDARD.encode(links);
        let server = MockServer::start().await;
        Mock::given(path("/sub"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "subscription-userinfo",
                        "upload=1; download=2; total=10737418240; expire=1893456000",
                    )
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let url = format!("{}/sub?token=SECRET", server.uri());

        if installed_version().is_none() {
            download_core(&|_, _| {}).await.unwrap();
        }
        let mut config = Config::default();
        config.proxy.mode = "app".into();
        config.proxy.port = free_port().unwrap();
        config.proxy.subscriptions = vec![sub("s1")];
        crate::store::set_key(&sub_key("s1"), &url).unwrap();

        let run = config.clone();
        tokio::task::spawn_blocking(move || apply(&run))
            .await
            .unwrap()
            .unwrap();
        assert!(running());
        let proxy = ctp_gateway::proxy::current().unwrap();
        assert_eq!(proxy, format!("http://127.0.0.1:{}", config.proxy.port));
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let state = state(&config).await;
        assert_eq!(state.error, None);
        assert_eq!(state.nodes.len(), 2, "{state:?}");
        assert_eq!(state.subs[0].total, Some(10737418240));
        assert_eq!(state.subs[0].host, host_of(&url));
        select("s1|Finland C").await.unwrap();
        measure().await.unwrap();
        assert_eq!(
            super::state(&config).await.selected.as_deref(),
            Some("s1|Finland C")
        );

        // Вторая подписка — без перезапуска ядра, выбор остаётся.
        config.proxy.subscriptions.push(sub("s2"));
        crate::store::set_key(&sub_key("s2"), &url).unwrap();
        let run = config.clone();
        tokio::task::spawn_blocking(move || reload(&run))
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let state = super::state(&config).await;
        assert_eq!(state.nodes.len(), 4);
        assert_eq!(state.selected.as_deref(), Some("s1|Finland C"));
        let log = std::fs::read_to_string(dir().unwrap().join("mihomo.log")).unwrap();
        assert!(!log.contains("SECRET"), "the token must not reach the log");

        shutdown();
        assert!(!running());
        crate::store::delete_key(&sub_key("s1"));
        crate::store::delete_key(&sub_key("s2"));
        ctp_gateway::proxy::set(None);
    }

    #[cfg(windows)]
    #[test]
    fn quoting_survives_spaces_quotes_and_trailing_slashes() {
        assert_eq!(win::quote(r"C:\a b\x.exe"), r#""C:\a b\x.exe""#);
        assert_eq!(win::quote(r"C:\dir\"), r#""C:\dir\\""#);
        assert_eq!(win::quote(r#"a"b"#), r#""a\"b""#);
    }
}
