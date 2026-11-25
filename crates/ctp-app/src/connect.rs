//! «Подключить проект»: agent-kit в проект без ручных команд (ADR-014). Решения — чистые
//! функции над фактами (`Facts`), у каждого случая из таблицы ADR свой тест; запуск
//! программ — тонкий слой вокруг них.

use crate::model::Config;
use crate::tr;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PLUGIN: &str = "agent-kit@agent-kit";
const MARKETPLACE: &str = "agent-kit";
/// Код agent-kit — на Node; старше 22 он не проверялся.
pub const MIN_NODE: u32 = 22;
/// Дольше команда не идёт: `claude plugin install` может качать маркетплейс.
const TIMEOUT: Duration = Duration::from_secs(180);

/// Пункт для окна: машинный id и текст для человека.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub text: String,
}

fn note(id: &str, text: String) -> Note {
    Note {
        id: id.to_string(),
        text,
    }
}

/// Состояние плагина для этого проекта по `claude plugin list --json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plugin {
    Absent,
    Enabled { install_path: PathBuf },
    Disabled,
}

/// Что известно до установки: машина, папка, плагин, критик.
#[derive(Clone, Debug)]
pub struct Facts {
    /// Старшая версия Node; `None` — Node нет.
    pub node: Option<u32>,
    /// Найден Claude Code CLI (установленный или портативный).
    pub cli: bool,
    /// Есть git.
    pub git: bool,
    /// Что сказал git, если папка не открылась как репозиторий (чужой владелец и т. п.).
    pub git_error: Option<String>,
    /// Папка, которую выбрал человек.
    pub requested: PathBuf,
    /// Корень git-репозитория этой папки; `None` — не репозиторий.
    pub root: Option<PathBuf>,
    pub plugin: Plugin,
    /// Маркетплейс agent-kit уже добавлен в Claude Code.
    pub marketplace: bool,
    /// Источник из настроек PGC на случай, когда маркетплейса нет.
    pub source: Option<String>,
    /// В рабочей копии есть незакоммиченные правки.
    pub dirty: bool,
    /// Критик выбран в настройках PGC.
    pub critic_chosen: bool,
    /// Критик выбран и шлюз его отдаёт: переменные `AGENT_KIT_*` будут.
    pub critic: bool,
    /// Критик — та же модель, что основная.
    pub critic_is_main: bool,
}

/// Итог проверки: что мешает подключить и что нужно знать.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub root: Option<String>,
    pub blockers: Vec<Note>,
    pub notes: Vec<Note>,
    /// Плагин уже включён: подключать заново не надо, сразу осмотр.
    pub connected: bool,
    /// Плагин явно выключен: включать только по согласию.
    pub disabled: bool,
}

/// Решение по фактам — таблица ADR-014.
pub fn decide(f: &Facts) -> Check {
    let mut check = Check {
        root: f.root.as_ref().map(|root| root.display().to_string()),
        connected: matches!(f.plugin, Plugin::Enabled { .. }),
        disabled: f.plugin == Plugin::Disabled,
        ..Check::default()
    };
    // 1. Node.
    match f.node {
        None => check.blockers.push(note(
            "node",
            tr!(
                "Нет Node.js: agent-kit работает на Node {MIN_NODE} или новее. Поставьте его с nodejs.org и нажмите «Проверить» ещё раз.",
                "Node.js is missing: agent-kit needs Node {MIN_NODE} or newer. Install it from nodejs.org and press Check again."
            ),
        )),
        Some(major) if major < MIN_NODE => check.blockers.push(note(
            "node",
            tr!(
                "Node {major} слишком старый: нужен {MIN_NODE} или новее (nodejs.org).",
                "Node {major} is too old: {MIN_NODE} or newer is needed (nodejs.org)."
            ),
        )),
        Some(_) => {}
    }
    // 2. Claude Code.
    if !f.cli {
        check.blockers.push(note(
            "cli",
            tr!(
                "Не найден Claude Code: скачайте его в Настройках PGC.",
                "Claude Code not found: download it in PGC's settings."
            ),
        ));
    }
    // 16. Нет git; 4. репозиторий; 5. подпапка.
    match &f.root {
        None if !f.git => check.blockers.push(note(
            "git",
            tr!(
                "Нет git: agent-kit сравнивает изменения через git. Поставьте его (git-scm.com) и нажмите «Проверить» ещё раз.",
                "git is missing: agent-kit compares changes through git. Install it (git-scm.com) and press Check again."
            ),
        )),
        None => {
            let said = f
                .git_error
                .as_deref()
                .map(str::trim)
                .filter(|said| !said.is_empty() && !said.contains("not a git repository"))
                .map(|said| format!(" git: {said}"))
                .unwrap_or_default();
            check.blockers.push(note(
                "repo",
                tr!(
                    "Эта папка — не git-репозиторий. agent-kit сравнивает изменения через git: откройте папку проекта с git.{said}",
                    "This folder is not a git repository. agent-kit compares changes through git: pick a project folder with git.{said}"
                ),
            ));
        }
        Some(root) if !same_path(root, &f.requested) => {
            let root = root.display();
            check.notes.push(note(
                "subfolder",
                tr!(
                    "Подключится весь репозиторий: {root}.",
                    "The whole repository is connected: {root}."
                ),
            ));
        }
        Some(_) => {}
    }
    // 9. Откуда ставить.
    if !check.connected && !f.marketplace && f.source.is_none() {
        check.blockers.push(note(
            "source",
            tr!(
                "Неизвестно, откуда взять agent-kit: его маркетплейс не добавлен в Claude Code, а источник в PGC не задан.",
                "Where to get agent-kit is unknown: its marketplace is not added to Claude Code and PGC has no source set."
            ),
        ));
    }
    // 7. Выключен самим человеком.
    if check.disabled {
        check.notes.push(note(
            "disabled",
            tr!(
                "agent-kit в этом проекте выключен. «Подключить» включит его снова.",
                "agent-kit is turned off in this project. Connect turns it back on."
            ),
        ));
    }
    // 10, 17, 11. Критик.
    if !f.critic_chosen {
        check.notes.push(note(
            "critic",
            tr!(
                "Критик в PGC не выбран: критик agent-kit пойдёт по подписке Claude, а без неё не сработает. Выбрать — в Настройках.",
                "No critic is chosen in PGC: agent-kit's critic goes through a Claude subscription and fails without one. Choose one in the settings."
            ),
        ));
    } else if !f.critic {
        check.notes.push(note(
            "critic-off",
            tr!(
                "Критик выбран, но шлюз остановлен или ещё не отдаёт эту модель: запустите или перезапустите шлюз, потом откройте Claude из PGC.",
                "A critic is chosen, but the gateway is stopped or does not serve that model yet: start or restart the gateway, then open Claude from PGC."
            ),
        ));
    } else if f.critic_is_main {
        check.notes.push(note(
            "critic-same",
            tr!(
                "Критик — та же модель, что основная: проверка своей же работы слабее. Лучше выбрать другую.",
                "The critic is the same model as the main one: reviewing its own work is weaker. Better pick another."
            ),
        ));
    }
    // 15. Незакоммиченные правки.
    if f.dirty {
        check.notes.push(note(
            "dirty",
            tr!(
                "В проекте есть незакоммиченные правки. Подключение их не тронет, новые файлы лягут рядом.",
                "The project has uncommitted edits. Connecting leaves them alone; new files go next to them."
            ),
        ));
    }
    check
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.display()
            .to_string()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Старшая версия из `node --version` (`v22.3.0`).
pub fn node_major(text: &str) -> Option<u32> {
    text.trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Состояние плагина для проекта из `claude plugin list --json`. Запись области `user` — для
/// всех проектов; `project`/`local` — только этого проекта. Как в Claude Code, ближняя
/// область решает: `local` важнее `project`, а та — `user` (выключен локально — выключен).
pub fn plugin_state(list: &Value, root: &Path) -> Plugin {
    let rank = |scope: &str| match scope {
        "local" => 3,
        "project" => 2,
        "user" => 1,
        _ => 0,
    };
    let entries = list.as_array().map(Vec::as_slice).unwrap_or_default();
    let nearest = entries
        .iter()
        .filter(|e| e["id"] == PLUGIN)
        .filter(|e| {
            e["scope"] == "user"
                || e["projectPath"]
                    .as_str()
                    .is_some_and(|p| same_path(Path::new(p), root))
        })
        .max_by_key(|e| rank(e["scope"].as_str().unwrap_or_default()));
    match nearest {
        None => Plugin::Absent,
        Some(entry) if entry["enabled"] != true => Plugin::Disabled,
        Some(entry) => entry["installPath"]
            .as_str()
            .map_or(Plugin::Absent, |path| Plugin::Enabled {
                install_path: PathBuf::from(path),
            }),
    }
}

/// Пункт плана `agent-kit setup --json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub what: String,
}

/// План из ответа `setup --json`; пункты `keep` — только совет человеку.
pub fn parse_plan(text: &str) -> Result<Vec<PlanItem>, String> {
    #[derive(Deserialize)]
    struct Plan {
        items: Vec<PlanItem>,
    }
    serde_json::from_str::<Plan>(text)
        .map(|plan| plan.items)
        .map_err(|exc| {
            tr!(
                "непонятный план agent-kit: {exc}",
                "unreadable agent-kit plan: {exc}"
            )
        })
}

/// Выбранные человеком пункты, которые можно применить: только известные и не `keep`.
pub fn applicable(plan: &[PlanItem], chosen: &[String]) -> Vec<String> {
    plan.iter()
        .filter(|item| item.action != "keep" && chosen.contains(&item.id))
        .map(|item| item.id.clone())
        .collect()
}

/// Вывод программы: код выхода, stdout и stderr. Окно консоли не мелькает, а зависшая
/// программа снимается по таймауту вместе со своими детьми.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(mut command: Command, timeout: Duration) -> Result<Output, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let shown = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|exc| format!("{shown}: {exc}"))?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|exc| exc.to_string())? {
            break status;
        }
        if started.elapsed() > timeout {
            // `claude.cmd` из npm — это cmd.exe с node внутри: снимать всё дерево, иначе node
            // доделает установку уже после показанной ошибки.
            kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            let secs = timeout.as_secs();
            return Err(tr!(
                "{shown}: не ответила за {secs} с",
                "{shown}: no answer in {secs} s"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok(Output {
        code: status.code(),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

fn kill_tree(pid: u32) {
    let mut command = Command::new("taskkill");
    command.args(["/T", "/F", "/PID", &pid.to_string()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

fn ok(output: Output, what: &str) -> Result<String, String> {
    if output.code == Some(0) {
        Ok(output.stdout)
    } else {
        let text = if output.stderr.trim().is_empty() {
            output.stdout
        } else {
            output.stderr
        };
        Err(format!("{what}: {}", text.trim()))
    }
}

/// PATH из реестра Windows (система, затем пользователь). Окно PGC держит PATH со своего
/// запуска: Node или git, поставленные после, иначе не нашлись бы до перезапуска PGC.
fn fresh_path() -> Option<String> {
    static CACHE: std::sync::Mutex<Option<(Instant, String)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|exc| exc.into_inner());
    if let Some((at, path)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return Some(path.clone());
    }
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-Command",
        "[Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')",
    ]);
    let registry = ok(run(command, Duration::from_secs(20)).ok()?, "PATH")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| text.len() > 1)?;
    // PATH самого процесса — в конце: Node от fnm или nvm бывает только в нём.
    let path = match std::env::var("PATH") {
        Ok(own) if !own.is_empty() => format!("{registry};{own}"),
        _ => registry,
    };
    *cache = Some((Instant::now(), path.clone()));
    Some(path)
}

/// Программа по свежему PATH (Rust на Windows ищет её по PATH, заданному ребёнку).
fn program(name: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(name);
    if let Some(path) = fresh_path() {
        command.env("PATH", path);
    }
    command
}

/// `claude` с окружением PGC: портативный — со своей папкой настроек (`CLAUDE_CONFIG_DIR`).
fn claude(config: &Config, root: &Path) -> Result<Command, String> {
    let (cli, extra) = crate::claude::code_cli(config)?;
    Ok(claude_with(cli, extra, root))
}

fn claude_with(cli: PathBuf, extra: Vec<(String, String)>, root: &Path) -> Command {
    let mut command = program(cli);
    command.current_dir(root).envs(extra);
    command
}

fn node(script: &Path, args: &[&str], root: &Path) -> Command {
    let mut command = program("node");
    command.arg(script).args(args).current_dir(root);
    command
}

/// Факты о папке и машине. Ничего не меняет.
pub fn facts(config: &Config, status: &crate::model::UiStatus, requested: &Path) -> Facts {
    let quick = |command: Command| run(command, Duration::from_secs(20));
    let node = quick({
        let mut c = program("node");
        c.arg("--version");
        c
    })
    .ok()
    .filter(|o| o.code == Some(0))
    .and_then(|o| node_major(&o.stdout));
    let git = quick({
        let mut c = program("git");
        c.arg("--version");
        c
    })
    .is_ok_and(|o| o.code == Some(0));
    let repo = quick({
        let mut c = program("git");
        c.args(["rev-parse", "--show-toplevel"])
            .current_dir(requested);
        c
    })
    .ok();
    let root = repo
        .as_ref()
        .filter(|o| o.code == Some(0))
        .map(|o| PathBuf::from(o.stdout.trim().replace('/', "\\")));
    let git_error = repo
        .filter(|o| o.code != Some(0))
        .map(|o| o.stderr.lines().next().unwrap_or_default().to_string());
    let cli = crate::claude::code_cli(config).is_ok();
    let work = root.clone().unwrap_or_else(|| requested.to_path_buf());
    let plugin = claude(config, &work)
        .ok()
        .and_then(|mut c| {
            c.args(["plugin", "list", "--json"]);
            run(c, TIMEOUT).ok()
        })
        .filter(|o| o.code == Some(0))
        .and_then(|o| serde_json::from_str::<Value>(&o.stdout).ok())
        .map_or(Plugin::Absent, |list| plugin_state(&list, &work));
    let marketplace = claude(config, &work)
        .ok()
        .and_then(|mut c| {
            c.args(["plugin", "marketplace", "list", "--json"]);
            run(c, TIMEOUT).ok()
        })
        .filter(|o| o.code == Some(0))
        .and_then(|o| serde_json::from_str::<Value>(&o.stdout).ok())
        .is_some_and(|list| {
            list.as_array()
                .is_some_and(|all| all.iter().any(|m| m["name"] == MARKETPLACE))
        });
    let dirty = root.as_ref().is_some_and(|root| {
        let mut c = program("git");
        c.args(["status", "--porcelain"]).current_dir(root);
        run(c, Duration::from_secs(30)).is_ok_and(|o| !o.stdout.trim().is_empty())
    });
    let main = crate::claude::main_model(status, &crate::claude::role_aliases(config))
        .map(|m| m.alias.clone());
    let critic_alias = crate::store::role_alias(config, config.model_roles.critic.as_ref());
    Facts {
        node,
        cli,
        git,
        git_error,
        requested: requested.to_path_buf(),
        root,
        plugin,
        marketplace,
        source: config.kit_source.clone().filter(|s| !s.trim().is_empty()),
        dirty,
        critic_chosen: config.model_roles.critic.is_some(),
        critic: !crate::claude::critic_env(config, status).is_empty(),
        critic_is_main: critic_alias.is_some() && critic_alias == main,
    }
}

/// Что делать с плагином, чтобы он заработал в проекте (случаи 6, 7, 9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Уже включён: ничего не ставить.
    Nothing(PathBuf),
    /// Выключен человеком: включить обратно (он согласился, нажав «Подключить»).
    Enable,
    /// Нет: поставить и обновить; источник — только если маркетплейса ещё нет.
    Install { source: Option<String> },
}

pub fn step(f: &Facts) -> Step {
    match &f.plugin {
        Plugin::Enabled { install_path } => Step::Nothing(install_path.clone()),
        Plugin::Disabled => Step::Enable,
        Plugin::Absent => Step::Install {
            source: (!f.marketplace).then(|| f.source.clone()).flatten(),
        },
    }
}

/// Сделать шаг. Возвращает папку установленной копии и признак «плагин только что включён»:
/// открытые сессии Claude его не увидят, нужна новая (случай 18).
pub fn open(config: &Config, f: &Facts, team: bool) -> Result<(PathBuf, bool), String> {
    let root = f.root.clone().ok_or_else(|| {
        tr!(
            "папка — не git-репозиторий",
            "the folder is not a git repository"
        )
    })?;
    let commands = match step(f) {
        Step::Nothing(path) => return Ok((path, false)),
        Step::Enable => vec![enable_args()],
        Step::Install { source } => install_steps(team, source.as_deref()),
    };
    for args in commands {
        let mut command = claude(config, &root)?;
        command.args(&args);
        ok(
            run(command, TIMEOUT)?,
            &format!("claude {}", args[..2].join(" ")),
        )?;
    }
    Ok((installed(config, &root)?, true))
}

/// Включить обратно только в этом проекте: область `local` ближе всех, перекрывает и
/// `project`, и `user`, не трогает git и другие проекты.
fn enable_args() -> Vec<String> {
    ["plugin", "enable", PLUGIN, "--scope", "local", "--json"]
        .map(String::from)
        .to_vec()
}

/// Команды `claude` для установки: включить в нужной области, затем обновить ту же
/// область — уже стоявшая копия могла быть старой (случай 8), а свежую обновлять дёшево.
fn install_steps(team: bool, source: Option<&str>) -> Vec<Vec<String>> {
    let scope = if team { "project" } else { "local" };
    let mut install: Vec<String> = ["plugin", "install", PLUGIN, "--scope", scope, "--json"]
        .map(String::from)
        .to_vec();
    if let Some(source) = source.map(str::trim).filter(|s| !s.is_empty()) {
        install.extend(["--marketplace".to_string(), source.to_string()]);
    }
    let update = ["plugin", "update", PLUGIN, "--scope", scope, "--json"]
        .map(String::from)
        .to_vec();
    vec![install, update]
}

/// Корень репозитория папки, если она в нём (для шагов после проверки: окно могло отстать).
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    let mut c = program("git");
    c.args(["rev-parse", "--show-toplevel"]).current_dir(dir);
    run(c, Duration::from_secs(20))
        .ok()
        .filter(|o| o.code == Some(0))
        .map(|o| PathBuf::from(o.stdout.trim().replace('/', "\\")))
}

/// Папка установленной копии для этого проекта.
pub fn installed(config: &Config, root: &Path) -> Result<PathBuf, String> {
    let mut list = claude(config, root)?;
    list.args(["plugin", "list", "--json"]);
    let text = ok(run(list, TIMEOUT)?, "claude plugin list")?;
    let value: Value = serde_json::from_str(&text).map_err(|exc| exc.to_string())?;
    match plugin_state(&value, root) {
        Plugin::Enabled { install_path } => Ok(install_path),
        _ => Err(tr!(
            "agent-kit не включился в этом проекте",
            "agent-kit did not turn on in this project"
        )),
    }
}

fn cli(kit: &Path) -> PathBuf {
    kit.join("scripts").join("cli.mjs")
}

/// Осмотр: `inspect --json` как есть, для окна.
pub fn inspect(kit: &Path, root: &Path) -> Result<Value, String> {
    let root_text = root.display().to_string();
    let text = ok(
        run(
            node(&cli(kit), &["inspect", "--json", "--cwd", &root_text], root),
            TIMEOUT,
        )?,
        "agent-kit inspect",
    )?;
    serde_json::from_str(&text).map_err(|exc| exc.to_string())
}

/// План `setup --json`.
pub fn plan(kit: &Path, root: &Path) -> Result<Vec<PlanItem>, String> {
    let root_text = root.display().to_string();
    parse_plan(&ok(
        run(
            node(&cli(kit), &["setup", "--json", "--cwd", &root_text], root),
            TIMEOUT,
        )?,
        "agent-kit setup",
    )?)
}

/// Применить выбранные пункты плана.
pub fn apply(kit: &Path, root: &Path, chosen: &[String]) -> Result<Value, String> {
    let ids = applicable(&plan(kit, root)?, chosen);
    if ids.is_empty() {
        return Ok(serde_json::json!({"applied": [], "skipped": []}));
    }
    let root_text = root.display().to_string();
    let only = ids.join(",");
    let text = ok(
        run(
            node(
                &cli(kit),
                &[
                    "setup", "--apply", "--only", &only, "--json", "--cwd", &root_text,
                ],
                root,
            ),
            TIMEOUT,
        )?,
        "agent-kit setup --apply",
    )?;
    serde_json::from_str(&text).map_err(|exc| exc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Facts {
        Facts {
            node: Some(24),
            cli: true,
            git: true,
            git_error: None,
            requested: PathBuf::from(r"D:\Проекты\мой проект"),
            root: Some(PathBuf::from(r"D:\Проекты\мой проект")),
            plugin: Plugin::Absent,
            marketplace: true,
            source: None,
            dirty: false,
            critic_chosen: true,
            critic: true,
            critic_is_main: false,
        }
    }

    fn ids(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|n| n.id.as_str()).collect()
    }

    /// Каждая строка таблицы ADR-014 — тест; номер строки в имени.
    #[test]
    fn case_01_no_or_old_node_blocks() {
        let mut f = base();
        f.node = None;
        assert_eq!(ids(&decide(&f).blockers), ["node"]);
        f.node = Some(MIN_NODE - 1);
        assert_eq!(ids(&decide(&f).blockers), ["node"]);
        f.node = Some(MIN_NODE);
        assert!(decide(&f).blockers.is_empty());
        assert_eq!(node_major("v22.3.0\n"), Some(22));
        assert_eq!(node_major("garbage"), None);
    }

    #[test]
    fn case_02_no_claude_code_blocks() {
        let mut f = base();
        f.cli = false;
        assert_eq!(ids(&decide(&f).blockers), ["cli"]);
    }

    #[test]
    fn case_03_portable_code_gets_its_own_config_dir() {
        // `claude()` = `code_cli` + `claude_with`: окружение портативного (CLAUDE_CONFIG_DIR)
        // доходит до каждой команды `claude plugin`, и плагин ставится в его настройки.
        let extra = vec![(
            "CLAUDE_CONFIG_DIR".to_string(),
            r"C:\PGC\code\config".to_string(),
        )];
        let command = claude_with(
            PathBuf::from(r"C:\PGC\code\claude.exe"),
            extra,
            Path::new(r"D:\p"),
        );
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR"
            && *v == Some(std::ffi::OsStr::new(r"C:\PGC\code\config"))));
        assert_eq!(command.get_current_dir(), Some(Path::new(r"D:\p")));
    }

    #[test]
    fn case_04_not_a_repository_blocks() {
        let mut f = base();
        f.root = None;
        assert_eq!(ids(&decide(&f).blockers), ["repo"]);
    }

    #[test]
    fn case_05_subfolder_connects_the_repository_root() {
        let mut f = base();
        f.requested = PathBuf::from(r"D:\Проекты\мой проект\packages\web");
        let check = decide(&f);
        assert!(check.blockers.is_empty());
        assert_eq!(ids(&check.notes), ["subfolder"]);
        // Тот же путь в другом написании — не подпапка.
        f.requested = PathBuf::from("d:/проекты/мой проект/");
        assert!(decide(&f).notes.is_empty());
    }

    #[test]
    fn case_06_already_enabled_skips_install() {
        let root = Path::new(r"D:\p");
        let list = json!([{"id": PLUGIN, "scope": "local", "enabled": true,
            "projectPath": "D:\\p", "installPath": "C:\\kit\\v2"}]);
        let state = plugin_state(&list, root);
        assert_eq!(
            state,
            Plugin::Enabled {
                install_path: PathBuf::from(r"C:\kit\v2")
            }
        );
        let mut f = base();
        f.plugin = state;
        f.marketplace = false; // уже стоит — источник не нужен
        let check = decide(&f);
        assert!(check.connected && check.blockers.is_empty());
        // Запись другого проекта не считается.
        let other = json!([{"id": PLUGIN, "scope": "local", "enabled": true,
            "projectPath": "D:\\other", "installPath": "C:\\kit"}]);
        assert_eq!(plugin_state(&other, root), Plugin::Absent);
        // Включён для всех проектов.
        let user =
            json!([{"id": PLUGIN, "scope": "user", "enabled": true, "installPath": "C:\\kit"}]);
        assert!(matches!(plugin_state(&user, root), Plugin::Enabled { .. }));
        // Включён — ничего не ставить, сразу эта копия.
        assert_eq!(step(&f), Step::Nothing(PathBuf::from(r"C:\kit\v2")));
        // Ближняя область решает: включён для всех, но выключен в этом проекте — выключен.
        let both = json!([
            {"id": PLUGIN, "scope": "user", "enabled": true, "installPath": "C:\\kit"},
            {"id": PLUGIN, "scope": "local", "enabled": false, "projectPath": "D:\\p", "installPath": "C:\\kit"}]);
        assert_eq!(plugin_state(&both, root), Plugin::Disabled);
        // Запись без папки копии — не включён.
        let broken = json!([{"id": PLUGIN, "scope": "user", "enabled": true}]);
        assert_eq!(plugin_state(&broken, root), Plugin::Absent);
    }

    #[test]
    fn case_07_turned_off_is_not_turned_on_silently() {
        let list = json!([{"id": PLUGIN, "scope": "project", "enabled": false,
            "projectPath": "D:\\p", "installPath": "C:\\kit"}]);
        let state = plugin_state(&list, Path::new(r"D:\p"));
        assert_eq!(state, Plugin::Disabled);
        let mut f = base();
        f.plugin = state;
        let check = decide(&f);
        assert!(check.disabled && !check.connected);
        assert_eq!(ids(&check.notes), ["disabled"]);
        // «Подключить» на выключенном — включить, а не ставить заново.
        assert_eq!(step(&f), Step::Enable);
        // Только в этом проекте: без области Claude Code мог бы включить его везде.
        assert_eq!(
            enable_args(),
            ["plugin", "enable", PLUGIN, "--scope", "local", "--json"]
        );
    }

    #[test]
    fn case_08_an_installed_copy_is_always_updated() {
        let steps = install_steps(false, None);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0][..3], ["plugin", "install", PLUGIN]);
        assert_eq!(steps[1][..3], ["plugin", "update", PLUGIN]);
        // Обе — в одной области: локальной по умолчанию, проектной — для всей команды.
        for (team, scope) in [(false, "local"), (true, "project")] {
            for step in install_steps(team, None) {
                let at = step.iter().position(|a| a == "--scope").unwrap();
                assert_eq!(step[at + 1], scope);
            }
        }
        // Источник — только установке и только непустой.
        let with = install_steps(false, Some(" owner/agent-kit "));
        assert!(with[0].ends_with(&["--marketplace".into(), "owner/agent-kit".into()]));
        assert!(!with[1].contains(&"--marketplace".to_string()));
        assert!(!install_steps(false, Some("  "))[0].contains(&"--marketplace".to_string()));
    }

    #[test]
    fn case_09_no_source_blocks_only_when_nothing_is_installed() {
        let mut f = base();
        f.marketplace = false;
        assert_eq!(ids(&decide(&f).blockers), ["source"]);
        f.source = Some("owner/agent-kit".into());
        assert!(decide(&f).blockers.is_empty());
        // Источник идёт в установку, только если маркетплейса нет.
        assert_eq!(
            step(&f),
            Step::Install {
                source: Some("owner/agent-kit".into())
            }
        );
        f.marketplace = true;
        assert_eq!(step(&f), Step::Install { source: None });
    }

    #[test]
    fn case_10_no_critic_is_said() {
        let mut f = base();
        f.critic_chosen = false;
        f.critic = false;
        assert_eq!(ids(&decide(&f).notes), ["critic"]);
    }

    #[test]
    fn case_16_no_git_is_not_called_not_a_repository() {
        let mut f = base();
        f.root = None;
        f.git = false;
        assert_eq!(ids(&decide(&f).blockers), ["git"]);
        // git есть, но отказал не по «не репозиторий»: его слова — в тексте.
        f.git = true;
        f.git_error = Some("fatal: detected dubious ownership in repository".into());
        let text = &decide(&f).blockers[0].text;
        assert!(text.contains("dubious ownership"), "{text}");
        f.git_error = Some("fatal: not a git repository (or any parent)".into());
        assert!(!decide(&f).blockers[0].text.contains("fatal"));
    }

    #[test]
    fn case_17_critic_chosen_but_not_served() {
        let mut f = base();
        f.critic = false; // выбран, но шлюз стоит или модель ещё не отдаёт
        assert_eq!(ids(&decide(&f).notes), ["critic-off"]);
    }

    #[test]
    fn case_18_a_fresh_connection_says_new_sessions_are_needed() {
        // Признак «только что включён» — из `open`: уже включённый не свежий, остальные свежие.
        let mut f = base();
        f.plugin = Plugin::Enabled {
            install_path: PathBuf::from(r"C:\kit"),
        };
        assert_eq!(
            open(&Config::default(), &f, false).unwrap(),
            (PathBuf::from(r"C:\kit"), false)
        );
        // Свежее включение проверяет живой тест: там `open` ставит и возвращает true.
    }

    #[test]
    fn case_19_a_hung_cmd_shim_is_killed_with_its_children() {
        // cmd.exe с ребёнком, который сам не выйдет: по таймауту должно уйти всё дерево.
        // ping с редким числом повторов — метка, по которой его потом ищем.
        let count = 50 + std::process::id() % 40;
        let mut shim = Command::new("cmd.exe");
        shim.args(["/c", &format!("ping -n {count} 127.0.0.9 >nul")]);
        assert!(run(shim, Duration::from_secs(3)).is_err());
        std::thread::sleep(Duration::from_millis(500));
        let mut check = Command::new("powershell.exe");
        check.args([
            "-NoProfile",
            "-Command",
            &format!("@(Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*-n {count} 127.0.0.9*' }}).Count"),
        ]);
        let left = run(check, Duration::from_secs(30)).unwrap().stdout;
        assert_eq!(left.trim(), "0", "дочерний процесс остался жить");
    }

    #[test]
    fn case_11_critic_same_as_main_is_warned() {
        let mut f = base();
        f.critic_is_main = true;
        assert_eq!(ids(&decide(&f).notes), ["critic-same"]);
    }

    #[test]
    fn case_12_spaces_and_cyrillic_in_paths() {
        // base() уже с пробелом и кириллицей; тот же путь иным регистром и слешами — один.
        assert!(same_path(
            Path::new(r"D:\Проекты\Мой Проект"),
            Path::new("d:/проекты/мой проект")
        ));
        assert!(decide(&base()).blockers.is_empty());
    }

    #[test]
    fn case_13_empty_plan_and_keep_items() {
        let plan = parse_plan(r#"{"root": "D:\\p", "items": []}"#).unwrap();
        assert!(plan.is_empty());
        let plan = parse_plan(
            r#"{"items": [
                {"id": "issues-log", "action": "create", "path": "docs/knowledge/issues.md", "what": "log"},
                {"id": "pre-push", "action": "keep", "path": ".githooks/pre-push", "what": "own hook"}]}"#,
        )
        .unwrap();
        let chosen = vec![
            "issues-log".to_string(),
            "pre-push".into(),
            "unknown".into(),
        ];
        assert_eq!(applicable(&plan, &chosen), ["issues-log"]);
        assert!(
            parse_plan("create docs/x").is_err(),
            "человеческий текст не разбираем"
        );
    }

    #[test]
    fn case_14_a_hung_or_failed_command_stops_with_its_output() {
        let mut slow = Command::new("powershell.exe");
        slow.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        let started = Instant::now();
        assert!(run(slow, Duration::from_secs(1)).is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        let mut fail = Command::new("powershell.exe");
        fail.args([
            "-NoProfile",
            "-Command",
            "[Console]::Error.Write('boom'); exit 3",
        ]);
        let out = run(fail, Duration::from_secs(30)).unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(ok(out, "step").unwrap_err(), "step: boom");
    }

    #[test]
    fn case_15_uncommitted_edits_are_said() {
        let mut f = base();
        f.dirty = true;
        assert_eq!(ids(&decide(&f).notes), ["dirty"]);
    }

    /// Вживую: временный git-репозиторий, настоящие `node`, `git` и `claude plugin`. Ставит
    /// agent-kit в область `local` этой временной папки, осматривает и убирает за собой.
    /// `cargo test -p ctp-app live_connect -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_connect_a_temporary_repository() {
        let dir = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-connect {}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let git = |args: &[&str]| {
            let mut c = Command::new("git");
            c.args(args).current_dir(&dir);
            run(c, Duration::from_secs(30)).unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("README.md"), "test\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ]);
        let config = Config::default();
        let status = crate::model::UiStatus {
            running: false,
            port: 0,
            token: String::new(),
            models: Vec::new(),
            log_path: String::new(),
            claude: None,
            update: None,
        };
        // Подпапка: подключается корень.
        let facts = facts(&config, &status, &dir.join("src"));
        let check = decide(&facts);
        println!("check: {check:#?}");
        assert!(check.blockers.is_empty(), "{:?}", check.blockers);
        assert!(check.notes.iter().any(|n| n.id == "subfolder"));
        let root = facts.root.clone().unwrap();
        let (kit, fresh) = open(&config, &facts, false).unwrap();
        assert!(fresh, "только что включён — нужна новая сессия (случай 18)");
        println!("kit: {}", kit.display());
        // Случай 6: повторное «Подключить» на уже включённом ничего не ставит.
        let again = super::facts(&config, &status, &root);
        assert!(decide(&again).connected);
        assert_eq!(open(&config, &again, false).unwrap(), (kit.clone(), false));
        // Случай 7: выключить в проекте — «Подключить» включает обратно только здесь.
        let user_before = super::facts(&config, &status, &std::env::temp_dir()).plugin;
        let mut disable = claude(&config, &root).unwrap();
        disable.args(["plugin", "disable", PLUGIN, "--scope", "local"]);
        ok(run(disable, TIMEOUT).unwrap(), "disable").unwrap();
        let off = super::facts(&config, &status, &root);
        assert_eq!(off.plugin, Plugin::Disabled);
        let (back, fresh) = open(&config, &off, false).unwrap();
        assert!(fresh);
        assert_eq!(back, kit);
        // Для чужой папки (временной вне репозитория) ничего не поменялось.
        assert_eq!(
            super::facts(&config, &status, &std::env::temp_dir()).plugin,
            user_before
        );
        let inspect = inspect(&kit, &root).unwrap();
        assert!(
            inspect["findings"]
                .as_array()
                .is_some_and(|f| !f.is_empty())
        );
        match plan(&kit, &root) {
            Ok(plan) => println!("plan: {plan:#?}"),
            Err(exc) => println!("plan not available: {exc}"),
        }
        // Ничего не попало в git: только .claude/settings.local.json, и он вне индекса.
        let tracked = git(&["status", "--porcelain"]).stdout;
        println!("git status: {tracked}");
        let mut uninstall = claude(&config, &root).unwrap();
        uninstall.args(["plugin", "uninstall", PLUGIN, "--scope", "local"]);
        ok(run(uninstall, TIMEOUT).unwrap(), "uninstall").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Полнота: у каждой строки таблицы ADR-014 есть тест `case_NN_…`.
    #[test]
    fn every_case_of_the_table_has_a_test() {
        let adr = include_str!("../../../docs/adr/014-connect-project.md");
        let source = include_str!("connect.rs");
        let rows: Vec<u32> = adr
            .lines()
            .filter_map(|line| line.strip_prefix("| ")?.split(' ').next()?.parse().ok())
            .collect();
        assert!(rows.len() >= 15, "строки таблицы не найдены");
        for row in rows {
            let name = format!("fn case_{row:02}_");
            assert!(source.contains(&name), "у случая {row} нет теста {name}…");
        }
    }
}
