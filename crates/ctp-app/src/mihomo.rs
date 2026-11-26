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
use std::time::{Duration, Instant};

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
/// sha256 самого exe из этого архива: его сверяет каждый запуск ядра (обычный и повышенный),
/// а не только скачивание — папка ядра доступна на запись программам без прав.
#[cfg(not(target_arch = "aarch64"))]
pub const EXE_SHA256: &str = "5a0231811a80b27477ebdee15684b41946d01afbd2114534e9a4b256125ca147";
#[cfg(target_arch = "aarch64")]
pub const EXE_SHA256: &str = "aaec9e412c6dc5103bec026c697c9447e6001a55e0151e2cce0419837d357ce5";
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
    // Напрямую: прокси приложения (и наш системный) смотрят в это самое ядро, а оно ещё не
    // скачано или остановлено для замены.
    download::to_file(
        &download::client_direct()?,
        &url,
        &archive,
        &expect,
        progress,
    )
    .await?;
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

/// id подписки идёт в путь: только буквы и цифры, как его выдаёт `proxy_add_sub`.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Удалённая подписка: её скачанный список узлов ядру больше не нужен.
pub fn forget_provider(id: &str) {
    if !valid_id(id) {
        return;
    }
    if let Some(dir) = dir() {
        let _ = std::fs::remove_file(
            dir.join("home")
                .join("providers")
                .join(format!("{id}.yaml")),
        );
    }
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
    // Локальные имена — напрямую (в TUN и системном режиме по ним приходят браузеры).
    let mut rules: Vec<String> = ["local", "lan", "internal", "home.arpa", "localdomain"]
        .iter()
        .map(|suffix| format!("DOMAIN-SUFFIX,{suffix},DIRECT"))
        .collect();
    rules.extend(
        DIRECT_NETS
            .iter()
            .map(|net| format!("IP-CIDR,{net},DIRECT,no-resolve")),
    );
    rules.push(format!("MATCH,{MAIN}"));
    let mut config = json!({
        "mixed-port": mixed,
        "bind-address": "127.0.0.1",
        "allow-lan": false,
        "mode": "rule",
        // `info` пишет строку на каждое соединение: в TUN журнал растёт на сотни МБ за дни.
        "log-level": "warning",
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
        // Своё DNS с подменными адресами (fake-ip): программы получают адрес из 198.18/16,
        // ядро знает по нему имя и ведёт соединение по имени. Без `respect-rules`: с ним
        // прямой выход разрешал имя тем же подменным DNS и стучался в 198.18.x (проверено в
        // Windows Sandbox). Домены локальной сети и проверки сети Windows — без подмены.
        config["dns"] = json!({
            "enable": true,
            "ipv6": false,
            "enhanced-mode": "fake-ip",
            "fake-ip-range": "198.18.0.1/16",
            "fake-ip-filter": ["+.lan", "+.local", "+.localdomain", "+.home.arpa", "+.msftconnecttest.com", "+.msftncsi.com"],
            "default-nameserver": ["1.1.1.1", "8.8.8.8"],
            "nameserver": ["https://1.1.1.1/dns-query", "https://dns.google/dns-query"],
        });
        // Имя по TLS SNI и HTTP Host: соединение по имени, даже если адрес пришёл не от нас.
        config["sniffer"] = json!({
            "enable": true,
            "sniff": { "TLS": { "ports": [443] }, "HTTP": { "ports": [80], "override-destination": true } },
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

/// mixed-порт: сначала из своего ряда рядом с портом шлюза. Случайный порт из динамического
/// диапазона после перезагрузки часто забирают зарезервированные диапазоны Hyper-V и WSL.
pub fn pick_port() -> Result<u16> {
    match (17990..18090).find(|&port| port_free(port)) {
        Some(port) => Ok(port),
        None => free_port(),
    }
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
    apply_with(|| config.clone())
}

/// То же, но настройки читаются уже под блокировкой: из двух быстрых смен режима
/// применится последняя сохранённая, а не та, чей поток дождался очереди позже.
pub fn apply_with(current: impl FnOnce() -> Config) -> Result<Option<u16>> {
    let _one = APPLYING.lock().unwrap_or_else(|e| e.into_inner());
    let config = &current();
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
    /// Номер запуска: сторож следит только за своим.
    generation: u64,
}

static CORE: Mutex<Option<Core>> = Mutex::new(None);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn urls_of_core() -> Vec<String> {
    CORE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|core| core.urls.clone())
        .unwrap_or_default()
}

/// Сторож запуска: ядро упало само (не остановлено нами) — сказать об этом в окне и, если
/// системный прокси наш, вернуть прежний, чтобы весь компьютер не остался без сети.
fn watch(generation: u64) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            // Под той же блокировкой, что смена режима (и в том же порядке: APPLYING, затем
            // CORE): новое ядро и его системный прокси старый сторож не тронет.
            let applying = APPLYING.lock().unwrap_or_else(|e| e.into_inner());
            let guard = CORE.lock().unwrap_or_else(|e| e.into_inner());
            let Some(core) = guard.as_ref().filter(|c| c.generation == generation) else {
                return; // остановлено нами или запущено заново
            };
            if core.process.alive() {
                continue;
            }
            // TUN поднимается через UAC — без человека его не перезапустить.
            let restart = !core.tun;
            drop(guard);
            tracing::warn!("mihomo core exited on its own");
            let system = crate::sysproxy::status().is_some_and(|s| s.on);
            set_error(Some(if system {
                tr!(
                    "ядро mihomo остановилось — запросы через VPN не проходят; системный прокси возвращён",
                    "the mihomo core stopped; requests through the VPN fail; the system proxy was put back"
                )
            } else {
                tr!(
                    "ядро mihomo остановилось — запросы через VPN не проходят",
                    "the mihomo core stopped; requests through the VPN fail"
                )
            }));
            if let Err(exc) = crate::sysproxy::disable() {
                tracing::warn!(error = %format!("{exc:#}"), "system proxy not restored");
            }
            drop(applying);
            // Упало после сна или смены сети — поднимаем снова тем же путём, что при старте
            // (порт, системный прокси); не больше двух раз за 10 минут, иначе ошибка остаётся.
            if restart && restart_allowed(Instant::now()) {
                std::thread::sleep(Duration::from_secs(3));
                if let Some(hook) = ON_CRASH.get() {
                    tracing::info!("restarting the mihomo core");
                    hook();
                }
            }
            return;
        }
    });
}

type CrashHook = Box<dyn Fn() + Send + Sync>;
static ON_CRASH: std::sync::OnceLock<CrashHook> = std::sync::OnceLock::new();
/// Перезапуски ядра сторожем: начало окна и их число в нём.
static RESTARTS: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// Что делать, когда ядро упало само (не TUN): приложение поднимает его своим путём.
pub fn on_crash(hook: CrashHook) {
    let _ = ON_CRASH.set(hook);
}

fn restart_allowed(now: Instant) -> bool {
    let mut window = RESTARTS.lock().unwrap_or_else(|e| e.into_inner());
    match window.as_mut() {
        Some((start, count)) if now.duration_since(*start) < Duration::from_secs(600) => {
            *count += 1;
            *count <= 2
        }
        _ => {
            *window = Some((now, 1));
            true
        }
    }
}
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
        mixed = pick_port()?;
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
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    *CORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Core {
        api: api.clone(),
        secret: secret.clone(),
        tun,
        process,
        urls,
        generation,
    });
    // Ждём REST: ядро поднимается за доли секунды; UAC в TUN — пока человек не ответит.
    let wait = if tun { 120 } else { 15 };
    let client = blocking_client();
    for _ in 0..wait * 5 {
        if !running() {
            // Хвост журнала ядра может цитировать адрес подписки — только хост.
            let tail = redact(&log_tail(&log), &urls_of_core());
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
            watch(generation);
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
    // Забираем ядро под мьютексом, а останавливаем уже без него: остановка в TUN ждёт
    // процесс до 10 с, и окно не должно всё это время стоять в running()/state().
    let core = CORE.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(core) = core {
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
    reload_with(|| config.clone())
}

/// Под той же блокировкой, что запуск: подписка, добавленная, пока ядро стартует (в TUN —
/// ждёт UAC), попадёт в него, а не потеряется до следующего перезапуска.
pub fn reload_with(current: impl FnOnce() -> Config) -> Result<()> {
    let _one = APPLYING.lock().unwrap_or_else(|e| e.into_inner());
    let config = &current();
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
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::process::CommandExt;
        // Сверка sha256 самого exe, и файл держится открытым без права записи и удаления до
        // запуска: подменить его между проверкой и стартом нельзя (папка ядра доступна на
        // запись без прав, а в TUN это запуск от администратора).
        let mut held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ: загрузчик Windows сможет его открыть
            .open(exe)
            .with_context(|| exe.display().to_string())?;
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        std::io::copy(&mut held, &mut hasher)?;
        let got: String = sha2::Digest::finalize(hasher)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if got != super::EXE_SHA256 {
            bail!(
                "{}",
                crate::tr!(
                    "файл ядра изменён (sha256 не совпадает) — скачайте ядро заново",
                    "the core file has been changed (sha256 mismatch); download the core again"
                )
            );
        }
        let job = kill_on_close_job()?;
        let out = std::fs::File::create(log)?;
        let child = std::process::Command::new(exe)
            // Секрет браузерного эндпоинта нужен только шлюзу.
            .env_remove(crate::fetch::URL_ENV)
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
        drop(held);
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
        // Тест в песочнице идёт из test-бинаря, который роль надсмотрщика не знает: ему
        // подставляется собранный exe приложения.
        #[cfg(test)]
        let me = std::env::var_os("PGC_SUPERVISOR_EXE").map_or(me, std::path::PathBuf::from);
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
        // С respect-rules прямой выход стучался в подменные 198.18.x (проверено в песочнице).
        assert!(tun["dns"].get("respect-rules").is_none());
        assert_eq!(tun["sniffer"]["enable"], true);

        // Без подписок — одна группа напрямую: ядро не примет пустую.
        let empty = build_config(&[], 7890, 9090, "k", false);
        assert_eq!(empty["proxy-groups"][0]["proxies"][0], "DIRECT");
        assert_eq!(empty["proxy-groups"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn only_plain_ids_become_paths() {
        assert!(valid_id("s1a2b3c"));
        for bad in ["", "..", r"..\..\x", "a/b", "s 1", "s.yaml"] {
            assert!(!valid_id(bad), "{bad}");
        }
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

    /// Живой TUN ТОЛЬКО в Windows Sandbox (`PGC_SANDBOX=1`, пользователь там администратор):
    /// настоящий путь — `apply` → повышенный надсмотрщик (`PGC_SUPERVISOR_EXE` — собранный exe
    /// приложения) → ядро с TUN. Без подписок группа идёт напрямую, поэтому интернет работает,
    /// а запрос без всякого прокси должен пройти через ядро (виден в его `/connections`).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_tun_in_sandbox() {
        assert_eq!(
            std::env::var("PGC_SANDBOX").as_deref(),
            Ok("1"),
            "sandbox only"
        );
        let home = std::env::var("PGC_LIVE_HOME").expect("set PGC_LIVE_HOME");
        assert!(
            std::env::var_os("PGC_SUPERVISOR_EXE").is_some(),
            "set PGC_SUPERVISOR_EXE"
        );
        // SAFETY: до потоков, читающих окружение.
        unsafe { std::env::set_var("CLAUDE_GATEWAY_HOME", &home) };
        if installed_version().is_none() {
            download_core(&|_, _| {}).await.unwrap();
        }
        // Ядро гаснет и если тест упадёт посередине.
        struct Down;
        impl Drop for Down {
            fn drop(&mut self) {
                shutdown();
            }
        }
        let _down = Down;
        let mut config = Config::default();
        config.proxy.mode = "tun".into();
        config.proxy.port = pick_port().unwrap();
        let run = config.clone();
        tokio::task::spawn_blocking(move || apply(&run))
            .await
            .unwrap()
            .unwrap();
        assert!(running() && is_tun());
        // TUN встаёт через несколько секунд после REST (адаптер, маршруты).
        let mut configs = Value::Null;
        for _ in 0..30 {
            configs = get("/configs").await.unwrap();
            if configs["tun"]["enable"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        println!("tun: {}", configs["tun"]);
        assert_eq!(configs["tun"]["enable"], true);
        // Маршруты Windows встают чуть позже адаптера.
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Прямой запрос, никакого прокси: его должен поймать TUN.
        let direct = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut answer = None;
        for attempt in 1..=3 {
            match direct
                .get("https://www.gstatic.com/generate_204")
                .send()
                .await
            {
                Ok(ok) => {
                    answer = Some(ok);
                    break;
                }
                Err(exc) => println!("attempt {attempt}: {exc:?}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let log = std::fs::read_to_string(dir().unwrap().join("mihomo.log")).unwrap_or_default();
        println!("core log:\n{log}");
        let answer = answer.expect("no answer through TUN");
        println!("direct request: {}", answer.status());
        let connections = get("/connections").await.unwrap();
        let hosts: Vec<String> = connections["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| {
                c["metadata"]["host"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        println!("core connections: {hosts:?}");
        assert!(
            hosts.iter().any(|h| h.contains("gstatic")),
            "TUN did not capture the request"
        );
        let adapters = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Get-NetAdapter | Format-Table Name, InterfaceDescription, Status | Out-String",
            ])
            .output()
            .unwrap();
        println!(
            "adapters (on):{}",
            String::from_utf8_lossy(&adapters.stdout)
        );

        shutdown();
        assert!(!running());
        std::thread::sleep(Duration::from_secs(3));
        let left = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq mihomo.exe"])
            .output()
            .unwrap();
        let left = String::from_utf8_lossy(&left.stdout).to_string();
        println!("mihomo after stop: {left}");
        assert!(
            !left.contains("mihomo.exe"),
            "the core outlived the supervisor"
        );
        ctp_gateway::proxy::set(None);
    }

    /// Подменённый exe в папке ядра (она доступна на запись без прав) не запускается.
    #[cfg(windows)]
    #[test]
    fn a_changed_core_file_is_refused() {
        let dir = crate::download::tests::tempdir();
        let exe = dir.join("mihomo.exe");
        std::fs::copy(std::env::var_os("ComSpec").unwrap(), &exe).unwrap();
        let err = win::start_in_job(&exe, &dir, &dir.join("c.yaml"), &dir.join("l.log"))
            .err()
            .expect("must refuse");
        assert!(err.to_string().contains("sha256"), "{err}");
        assert!(!dir.join("l.log").exists(), "nothing was started");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn quoting_survives_spaces_quotes_and_trailing_slashes() {
        assert_eq!(win::quote(r"C:\a b\x.exe"), r#""C:\a b\x.exe""#);
        assert_eq!(win::quote(r"C:\dir\"), r#""C:\dir\\""#);
        assert_eq!(win::quote(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn the_watchdog_restarts_a_crashed_core_twice_per_ten_minutes() {
        let start = Instant::now();
        assert!(restart_allowed(start));
        assert!(restart_allowed(start + Duration::from_secs(60)));
        // The third crash inside the window stays an error for the person to see.
        assert!(!restart_allowed(start + Duration::from_secs(120)));
        // A new window after ten minutes.
        assert!(restart_allowed(start + Duration::from_secs(700)));
    }
}
