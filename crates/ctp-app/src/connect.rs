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
    /// Папка, на которую он зарегистрирован.
    pub marketplace_path: Option<PathBuf>,
    /// Источник из настроек PGC на случай, когда маркетплейса нет.
    pub source: Option<String>,
    /// Версия agent-kit, встроенного в эту сборку PGC (ADR-015); `None` — не встроен.
    pub embedded: Option<String>,
    /// Версия, которую сейчас даёт маркетплейс-папка человека (не PGC): `version` из
    /// plugin.json, иначе первые 12 знаков коммита — так Claude Code её и называет. `None` —
    /// узнать нельзя (не папка, нет ни версии, ни git).
    pub marketplace_version: Option<String>,
    /// Куда PGC распаковывает встроенный agent-kit.
    pub kit_dir: Option<PathBuf>,
    /// Версия и область установленной в проекте копии.
    pub installed_version: Option<String>,
    pub installed_scope: Option<String>,
    /// В рабочей копии есть незакоммиченные правки.
    pub dirty: bool,
    /// Claude Code ответил на оба списка (плагины и маркетплейсы); `false` — сбой или таймаут.
    pub lists_ok: bool,
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
    /// Встроенный agent-kit новее установленной копии: обновить можно по кнопке (случай 24).
    pub update: Option<KitUpdate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitUpdate {
    pub from: String,
    pub to: String,
}

impl Facts {
    /// Маркетплейс `agent-kit` зарегистрирован на папку PGC: им управляет PGC.
    pub fn ours(&self) -> bool {
        self.marketplace
            && matches!((&self.marketplace_path, &self.kit_dir),
                (Some(a), Some(b)) if same_path(a, b))
    }
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
    // 32. Claude Code не отдал список плагинов: по пустому списку ничего не ставить.
    if f.cli && !f.lists_ok {
        check.blockers.push(note(
            "lists",
            tr!(
                "Claude Code не ответил, какие плагины у вас стоят. Подключать вслепую нельзя: закройте лишние окна Claude Code и нажмите «Проверить» ещё раз.",
                "Claude Code did not say which plugins you have. Connecting blind is not safe: close extra Claude Code windows and press Check again."
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
    // 22. Маркетплейс на чужой папке (живая папка разработчика): PGC его не трогает.
    if f.embedded.is_some() && f.marketplace && !f.ours() {
        let path = f
            .marketplace_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        check.notes.push(note(
            "elsewhere",
            tr!(
                "agent-kit берётся из уже добавленного в Claude Code маркетплейса ({path}), а не из PGC. PGC его не трогает.",
                "agent-kit comes from the marketplace already added to Claude Code ({path}), not from PGC. PGC leaves it alone."
            ),
        ));
    }
    // 24. Встроенный новее установленного — только предложение, обновление по кнопке.
    if let (true, true, Some(to)) = (check.connected, f.ours(), f.embedded.as_ref())
        && f.installed_version.as_ref() != Some(to)
    {
        let from = f.installed_version.clone().unwrap_or_else(|| "?".into());
        check.notes.push(note(
            "update",
            tr!(
                "В PGC другая версия agent-kit ({to}), чем в проекте ({from}). Поставить её можно кнопкой ниже.",
                "PGC carries another agent-kit version ({to}) than the project has ({from}). Install it with the button below."
            ),
        ));
        check.update = Some(KitUpdate {
            from,
            to: to.clone(),
        });
    }
    // 31. Маркетплейс-папка человека новее копии в проекте: тоже по кнопке. Саму папку и
    // регистрацию маркетплейса PGC не трогает, обновляется только копия плагина.
    if let (true, false, Some(to)) = (check.connected, f.ours(), f.marketplace_version.as_ref())
        && f.installed_version.as_ref() != Some(to)
    {
        let from = f.installed_version.clone().unwrap_or_else(|| "?".into());
        check.notes.push(note(
            "update",
            tr!(
                "В маркетплейсе agent-kit другая версия ({to}), чем в проекте ({from}). Поставить её можно кнопкой ниже.",
                "The agent-kit marketplace has another version ({to}) than the project ({from}). Install it with the button below."
            ),
        ));
        check.update = Some(KitUpdate {
            from,
            to: to.clone(),
        });
    }
    // 9, 20. Откуда ставить: маркетплейс, встроенный в PGC или источник из настроек.
    if !check.connected && !f.marketplace && f.source.is_none() && f.embedded.is_none() {
        check.blockers.push(note(
            "source",
            tr!(
                "Неизвестно, откуда взять agent-kit: в эту сборку PGC он не встроен, его маркетплейс не добавлен в Claude Code, а источник в PGC не задан.",
                "Where to get agent-kit is unknown: it is not built into this PGC, its marketplace is not added to Claude Code and PGC has no source set."
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
    match nearest(list, root) {
        None => Plugin::Absent,
        Some(entry) if entry["enabled"] != true => Plugin::Disabled,
        Some(entry) => entry["installPath"]
            .as_str()
            .map_or(Plugin::Absent, |path| Plugin::Enabled {
                install_path: PathBuf::from(path),
            }),
    }
}

/// Запись плагина, которая решает для этого проекта: ближняя область.
fn nearest<'a>(list: &'a Value, root: &Path) -> Option<&'a Value> {
    let rank = |scope: &str| match scope {
        "local" => 3,
        "project" => 2,
        "user" => 1,
        _ => 0,
    };
    list.as_array()?
        .iter()
        .filter(|e| e["id"] == PLUGIN)
        .filter(|e| {
            e["scope"] == "user"
                || e["projectPath"]
                    .as_str()
                    .is_some_and(|p| same_path(Path::new(p), root))
        })
        .max_by_key(|e| rank(e["scope"].as_str().unwrap_or_default()))
}

/// Путь маркетплейса `agent-kit` из `claude plugin marketplace list --json`; `None` — нет.
pub fn marketplace_path(list: &Value) -> Option<Option<PathBuf>> {
    let entry = list.as_array()?.iter().find(|m| m["name"] == MARKETPLACE)?;
    Some(entry["path"].as_str().map(PathBuf::from))
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
    /// `false` — окно показывает пункт без галочки (черновик `critic.md`, ADR-019).
    #[serde(default = "yes")]
    pub default: bool,
    /// Правила черновика `critic.md`: окно показывает их целиком.
    #[serde(default)]
    pub rules: Vec<String>,
    /// Зачем этот пункт, простыми словами (agent-kit, на языке окна): окно открывает по «?».
    #[serde(default)]
    pub about: String,
    /// Короткое название для человека («Инструкция для агентов») и блок плана
    /// (`agent`, `checks`, `safety`, `docs`, `advice`); пусто — у старой копии agent-kit.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub group: String,
    /// Откуда взято каждое правило черновика (`rules[i]` ← `from[i]`); пусто — не сказано.
    #[serde(default)]
    pub from: Vec<CheckSource>,
}

fn yes() -> bool {
    true
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

/// Где проект записал проверку.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSource {
    pub file: String,
    #[serde(default)]
    pub line: u32,
}

/// Проверка из `setup --propose --json` (ADR-016): команда, которую проект уже записал.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckItem {
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub source: Option<CheckSource>,
    /// Почему отсеяна (только у `struck`): окно показывает её без галочки.
    #[serde(default)]
    pub why: String,
    /// Слово правила имён, по которому отсеяна (`fix`, `deploy`…): окно пишет причину по-своему.
    #[serde(default)]
    pub ruled: String,
    /// Своя `.claude/verify.json` уже запускает эту команду: окно не ставит галочку.
    #[serde(default)]
    pub present: bool,
    /// Сколько раз эта проверка в проекте падала, а потом проходила после правки (журнал опыта).
    #[serde(default)]
    pub caught: u32,
}

/// Отбор проверок кодом: `checks` — с галочкой, `struck` — без галочки, с причиной.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub checks: Vec<CheckItem>,
    #[serde(default)]
    pub struck: Vec<CheckItem>,
}

pub fn parse_proposal(text: &str) -> Result<Proposal, String> {
    serde_json::from_str(text).map_err(|exc| {
        tr!(
            "непонятный список проверок agent-kit: {exc}",
            "unreadable agent-kit list of checks: {exc}"
        )
    })
}

/// Выбранные проверки, которые можно передать: только id из отбора, в его порядке.
pub fn chosen_checks(proposal: &Proposal, chosen: &[String]) -> Vec<String> {
    proposal
        .checks
        .iter()
        .chain(&proposal.struck)
        .filter(|check| chosen.contains(&check.id))
        .map(|check| check.id.clone())
        .collect()
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
    let list = |args: &[&str]| {
        claude(config, &work)
            .ok()
            .and_then(|mut c| {
                c.args(args);
                run(c, TIMEOUT).ok()
            })
            .filter(|o| o.code == Some(0))
            .and_then(|o| serde_json::from_str::<Value>(&o.stdout).ok())
    };
    let plugins = list(&["plugin", "list", "--json"]);
    let plugin = plugins
        .as_ref()
        .map_or(Plugin::Absent, |list| plugin_state(list, &work));
    let entry = plugins.as_ref().and_then(|list| nearest(list, &work));
    let field = |name: &str| entry.and_then(|e| e[name].as_str()).map(str::to_string);
    let markets = list(&["plugin", "marketplace", "list", "--json"]);
    // A failed list is not "nothing installed": acting on it would turn a disabled plugin back
    // on or point a taken marketplace name at PGC's folder (case 32).
    let lists_ok = !cli || (plugins.is_some() && markets.is_some());
    let registered = markets.as_ref().and_then(marketplace_path);
    let marketplace_version = registered
        .clone()
        .flatten()
        .as_deref()
        .and_then(folder_version);
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
        marketplace: registered.is_some(),
        marketplace_path: registered.flatten(),
        source: config.kit_source.clone().filter(|s| !s.trim().is_empty()),
        embedded: crate::kit::embedded().map(str::to_string),
        marketplace_version,
        kit_dir: crate::kit::dir(),
        installed_version: field("version"),
        installed_scope: field("scope"),
        dirty,
        lists_ok,
        // A critic whose provider was deleted is not chosen any more: "restart the gateway" would not help.
        critic_chosen: critic_alias.is_some(),
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
        // Маркетплейса нет: встроенный в PGC (случай 23), иначе источник из настроек.
        Plugin::Absent => Step::Install {
            source: (!f.marketplace)
                // Источник, заданный человеком, важнее встроенного: он выбран нарочно.
                .then(|| match (&f.source, &f.embedded, &f.kit_dir) {
                    (Some(source), _, _) => Some(source.clone()),
                    (None, Some(_), Some(dir)) => Some(dir.display().to_string()),
                    _ => None,
                })
                .flatten(),
        },
    }
}

/// Нужно ли перед установкой распаковать встроенный agent-kit: установка берёт его из папки
/// PGC или маркетплейс — папка PGC, которая могла пропасть (случаи 21, 23).
fn needs_unpack(f: &Facts, source: Option<&str>) -> bool {
    let from_kit = source
        .zip(f.kit_dir.as_ref())
        .is_some_and(|(s, dir)| same_path(Path::new(s), dir));
    from_kit || f.ours()
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
        Step::Install { source } => {
            if needs_unpack(f, source.as_deref()) {
                crate::kit::extracted()?;
            }
            install_steps(team, source.as_deref())
        }
    };
    run_claude(config, &root, commands)?;
    Ok((installed(config, &root)?, true))
}

fn run_claude(config: &Config, root: &Path, commands: Vec<Vec<String>>) -> Result<(), String> {
    for args in commands {
        let mut command = claude(config, root)?;
        command.args(&args);
        ok(
            run(command, TIMEOUT)?,
            &format!("claude {}", args[..2].join(" ")),
        )?;
    }
    Ok(())
}

/// Обновить копию проекта до встроенной в PGC — только по кнопке (случай 24): распаковать,
/// перечитать маркетплейс PGC, обновить ту область, где плагин стоит.
pub fn update(config: &Config, f: &Facts) -> Result<(PathBuf, bool), String> {
    let root = f.root.clone().ok_or_else(|| {
        tr!(
            "папка — не git-репозиторий",
            "the folder is not a git repository"
        )
    })?;
    if decide(f).update.is_none() {
        return Ok((installed(config, &root)?, false));
    }
    // Встроенный распаковывается только для маркетплейса PGC (случай 24), не для чужого (31).
    if f.ours() {
        crate::kit::extracted()?;
    }
    run_claude(config, &root, update_steps(f.installed_scope.as_deref()))?;
    Ok((installed(config, &root)?, true))
}

/// Версия плагина в папке-маркетплейсе, как её называет Claude Code: `version` из
/// `.claude-plugin/plugin.json`, иначе первые 12 знаков текущего коммита папки.
fn folder_version(dir: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(dir.join(".claude-plugin").join("plugin.json")).ok()?;
    if let Some(version) = serde_json::from_str::<Value>(&manifest).ok()?["version"].as_str() {
        return Some(version.to_string());
    }
    let mut c = program("git");
    c.args(["rev-parse", "HEAD"]).current_dir(dir);
    run(c, Duration::from_secs(20))
        .ok()
        .filter(|o| o.code == Some(0))
        .and_then(|o| o.stdout.trim().get(..12).map(str::to_string))
}

fn update_steps(scope: Option<&str>) -> Vec<Vec<String>> {
    let scope = scope.unwrap_or("local");
    vec![
        ["plugin", "marketplace", "update", MARKETPLACE]
            .map(String::from)
            .to_vec(),
        ["plugin", "update", PLUGIN, "--scope", scope, "--json"]
            .map(String::from)
            .to_vec(),
    ]
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
    let source = source.map(str::trim).filter(|s| !s.is_empty());
    // С `--marketplace` Claude Code требует голое имя: «Name the marketplace once».
    let name = if source.is_some() {
        MARKETPLACE
    } else {
        PLUGIN
    };
    let mut install: Vec<String> = ["plugin", "install", name, "--scope", scope, "--json"]
        .map(String::from)
        .to_vec();
    if let Some(source) = source {
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

/// Язык текстов agent-kit для человека — как у окна PGC.
fn lang() -> &'static str {
    if crate::i18n::is_ru() { "ru" } else { "en" }
}

fn cli(kit: &Path) -> PathBuf {
    kit.join("scripts").join("cli.mjs")
}

/// Осмотр: `inspect --json` как есть, для окна.
pub fn inspect(kit: &Path, root: &Path) -> Result<Value, String> {
    let root_text = root.display().to_string();
    let text = ok(
        run(
            node(
                &cli(kit),
                &["inspect", "--json", "--lang", lang(), "--cwd", &root_text],
                root,
            ),
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
            node(
                &cli(kit),
                &["setup", "--json", "--lang", lang(), "--cwd", &root_text],
                root,
            ),
            TIMEOUT,
        )?,
        "agent-kit setup",
    )?)
}

/// Отбор проверок кодом. Без `--model`: файлы проекта никуда не уходят (ADR-016).
pub fn propose(kit: &Path, root: &Path) -> Result<Proposal, String> {
    let root_text = root.display().to_string();
    parse_proposal(&ok(
        run(
            node(
                &cli(kit),
                &[
                    "setup",
                    "--propose",
                    "--json",
                    "--lang",
                    lang(),
                    "--cwd",
                    &root_text,
                ],
                root,
            ),
            TIMEOUT,
        )?,
        "agent-kit setup --propose",
    )?)
}

/// Применить выбранные пункты плана.
pub fn apply(
    kit: &Path,
    root: &Path,
    chosen: &[String],
    checks: &[String],
) -> Result<Value, String> {
    let plan = plan(kit, root)?;
    let checks = if checks.is_empty() {
        Vec::new()
    } else {
        chosen_checks(&propose(kit, root)?, checks)
    };
    let Some(args) = apply_args(&plan, chosen, &checks) else {
        return Ok(serde_json::json!({"applied": [], "skipped": []}));
    };
    let root_text = root.display().to_string();
    let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
    args.extend(["--json", "--lang", lang(), "--cwd", &root_text]);
    let text = ok(
        run(node(&cli(kit), &args, root), TIMEOUT)?,
        "agent-kit setup --apply",
    )?;
    serde_json::from_str(&text).map_err(|exc| exc.to_string())
}

/// Аргументы `setup --apply`; `None` — применять нечего. Отмеченные проверки идут в новый
/// `.claude/verify.json` (его пункт плана), а если свой файл уже есть — в пункт `verify-checks`:
/// agent-kit его не меняет и отдаёт шаги текстом для ручного добавления (случай 28).
pub fn apply_args(plan: &[PlanItem], chosen: &[String], checks: &[String]) -> Option<Vec<String>> {
    let mut only = applicable(plan, chosen);
    if !checks.is_empty() {
        let verify = VERIFY_FILE;
        let item = if plan.iter().any(|item| item.id == verify) {
            verify
        } else {
            "verify-checks"
        };
        if !only.iter().any(|id| id == item) {
            only.push(item.to_string());
        }
    }
    if only.is_empty() {
        return None;
    }
    let mut args = vec![
        "setup".into(),
        "--apply".into(),
        "--only".into(),
        only.join(","),
    ];
    if !checks.is_empty() {
        args.extend(["--checks".into(), checks.join(",")]);
    }
    Some(args)
}

/// Пункт плана, который создаёт файл проверок проекта.
pub const VERIFY_FILE: &str = ".claude/verify.json";

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
            marketplace_path: None,
            source: None,
            embedded: None,
            marketplace_version: None,
            kit_dir: Some(PathBuf::from(
                r"C:\Users\u\AppData\Local\ClaudeProviderGateway\agent-kit",
            )),
            installed_version: None,
            installed_scope: None,
            dirty: false,
            lists_ok: true,
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
        // С источником — голое имя плагина, иначе Claude Code отказывает.
        assert_eq!(with[0][2], "agent-kit");
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

    fn kit_dir() -> PathBuf {
        base().kit_dir.unwrap()
    }

    #[test]
    fn case_20_no_built_in_kit_uses_the_marketplace_or_the_source() {
        // Встроенного нет (сборка без снимка), маркетплейса и источника нет — блокер.
        let mut f = base();
        f.marketplace = false;
        assert_eq!(ids(&decide(&f).blockers), ["source"]);
        // Встроенный есть — блокера нет, ставится из папки PGC.
        f.embedded = Some("92b16a29f355".into());
        assert!(decide(&f).blockers.is_empty());
        assert_eq!(
            step(&f),
            Step::Install {
                source: Some(kit_dir().display().to_string())
            }
        );
    }

    #[test]
    fn case_21_an_unpacked_copy_that_broke_is_unpacked_again() {
        // Распаковка — `kit::ensure` (её тесты — в kit.rs). Здесь — когда она нужна.
        let mut f = base();
        f.embedded = Some("v".into());
        let kit = kit_dir().display().to_string();
        // Маркетплейс — папка PGC (могла пропасть): распаковать, даже если ставим без источника.
        f.marketplace = true;
        f.marketplace_path = Some(kit_dir());
        assert!(f.ours() && needs_unpack(&f, None));
        // Чужой маркетплейс — не трогать.
        f.marketplace_path = Some(PathBuf::from(r"D:\dev\agent-kit"));
        assert!(!f.ours() && !needs_unpack(&f, None));
        // Маркетплейса нет, ставим из папки PGC — распаковать; из своего источника — нет.
        f.marketplace = false;
        f.marketplace_path = None;
        assert!(needs_unpack(&f, Some(&kit)));
        assert!(!needs_unpack(&f, Some("owner/agent-kit")));
    }

    #[test]
    fn case_22_a_marketplace_elsewhere_is_left_alone() {
        let mut f = base();
        f.embedded = Some("v2".into());
        f.marketplace = true;
        f.marketplace_path = Some(PathBuf::from(r"D:\dev\agent-kit"));
        let check = decide(&f);
        assert_eq!(ids(&check.notes), ["elsewhere"]);
        // Ставится из него, без --marketplace на папку PGC.
        assert_eq!(step(&f), Step::Install { source: None });
        // И обновление не предлагается: это не маркетплейс PGC.
        f.plugin = Plugin::Enabled {
            install_path: PathBuf::from(r"C:\cache"),
        };
        f.installed_version = Some("old".into());
        assert!(decide(&f).update.is_none());
        // Версию папки узнать не удалось — тоже не предлагается (случай 31 — только с ней).
        f.marketplace_version = None;
        assert!(decide(&f).update.is_none());
    }

    #[test]
    fn case_32_a_failed_plugin_list_blocks_instead_of_looking_empty() {
        let mut f = base();
        f.lists_ok = false;
        // Without the lists the plugin looks absent: installing would be blind.
        f.plugin = Plugin::Absent;
        let check = decide(&f);
        assert_eq!(ids(&check.blockers), ["lists"]);
        // No Claude Code at all: its own blocker says that, not this one.
        f.cli = false;
        assert!(!ids(&decide(&f).blockers).contains(&"lists"));
    }

    #[test]
    fn case_31_a_newer_kit_in_the_persons_marketplace_is_offered_by_a_button() {
        let mut f = base();
        f.embedded = Some("v2".into());
        f.marketplace = true;
        f.marketplace_path = Some(PathBuf::from(r"D:\dev\agent-kit"));
        f.plugin = Plugin::Enabled {
            install_path: PathBuf::from(r"C:\cache\old"),
        };
        f.installed_version = Some("6ac828f6b68f".into());
        f.marketplace_version = Some("f8cf4cd2cccf".into());
        let check = decide(&f);
        assert_eq!(
            check.update,
            Some(KitUpdate {
                from: "6ac828f6b68f".into(),
                to: "f8cf4cd2cccf".into()
            }),
            "берётся версия папки, а не встроенная в PGC"
        );
        assert!(ids(&check.notes).contains(&"update"));
        // Та же версия — предлагать нечего.
        f.installed_version = Some("f8cf4cd2cccf".into());
        assert!(decide(&f).update.is_none());
        // Ещё не подключён — сначала подключение, не обновление.
        f.plugin = Plugin::Absent;
        f.installed_version = None;
        assert!(decide(&f).update.is_none());
    }

    #[test]
    fn case_31_the_version_of_a_folder_is_named_as_claude_code_names_it() {
        let dir = std::env::temp_dir().join(format!("pgc-kitver-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "agent-kit", "version": "1.2.3"}"#,
        )
        .unwrap();
        assert_eq!(folder_version(&dir).as_deref(), Some("1.2.3"));
        // Без версии и без git — не узнать.
        std::fs::write(
            dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "agent-kit"}"#,
        )
        .unwrap();
        assert_eq!(folder_version(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
        // Папка этого репозитория: без version в plugin.json её нет, а коммит есть — но здесь
        // нет и plugin.json, так что `None` (git не спрашивается без манифеста).
        assert_eq!(folder_version(Path::new(env!("CARGO_MANIFEST_DIR"))), None);
    }

    #[test]
    fn case_23_no_marketplace_registers_the_pgc_folder() {
        let mut f = base();
        f.embedded = Some("v2".into());
        f.marketplace = false;
        let Step::Install { source } = step(&f) else {
            panic!("ожидалась установка")
        };
        let steps = install_steps(false, source.as_deref());
        assert!(steps[0].ends_with(&["--marketplace".into(), kit_dir().display().to_string()]));
        // Источник, заданный человеком, важнее встроенного.
        f.source = Some("owner/agent-kit".into());
        assert_eq!(
            step(&f),
            Step::Install {
                source: Some("owner/agent-kit".into())
            }
        );
    }

    #[test]
    fn case_24_a_newer_built_in_kit_is_offered_not_forced() {
        let mut f = base();
        f.embedded = Some("new".into());
        f.marketplace = true;
        f.marketplace_path = Some(kit_dir());
        f.plugin = Plugin::Enabled {
            install_path: PathBuf::from(r"C:\cache\old"),
        };
        f.installed_version = Some("old".into());
        f.installed_scope = Some("project".into());
        let check = decide(&f);
        assert_eq!(
            check.update,
            Some(KitUpdate {
                from: "old".into(),
                to: "new".into()
            })
        );
        assert!(ids(&check.notes).contains(&"update"));
        // «Подключить» на подключённом ничего не ставит: обновление — только кнопкой.
        assert_eq!(step(&f), Step::Nothing(PathBuf::from(r"C:\cache\old")));
        // Кнопка обновляет ту область, где плагин стоит.
        let steps = update_steps(f.installed_scope.as_deref());
        assert_eq!(steps[0], ["plugin", "marketplace", "update", MARKETPLACE]);
        assert_eq!(
            steps[1],
            ["plugin", "update", PLUGIN, "--scope", "project", "--json"]
        );
        // Та же версия — предложения нет.
        f.installed_version = Some("new".into());
        assert!(decide(&f).update.is_none());
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

    /// Вживую, как у человека без agent-kit: своя папка настроек Claude Code (маркетплейса нет)
    /// и свой профиль PGC. agent-kit распаковывается из exe, регистрируется и ставится с
    /// версией встроенного снимка. Нужен снимок (`tools/kit-snapshot.ps1`) до сборки.
    /// `cargo test -p ctp-app live_built_in -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_built_in_kit_for_a_new_user() {
        let Some(version) = crate::kit::embedded() else {
            panic!("в этой сборке нет снимка: запустите tools/kit-snapshot.ps1 и соберите заново");
        };
        let base = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-newuser {}", uuid::Uuid::new_v4().simple()));
        let (cfg, home, repo) = (base.join("cfg"), base.join("home"), base.join("repo"));
        for d in [&cfg, &home, &repo] {
            std::fs::create_dir_all(d).unwrap();
        }
        // SAFETY: тест запускается один (`--ignored`, по имени), до него потоков с окружением нет.
        unsafe {
            std::env::set_var("CLAUDE_CONFIG_DIR", &cfg);
            std::env::set_var("CLAUDE_GATEWAY_HOME", &home);
        }
        let git = |args: &[&str]| {
            let mut c = Command::new("git");
            c.args(args).current_dir(&repo);
            run(c, Duration::from_secs(30)).unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "i",
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
        let f = super::facts(&config, &status, &repo);
        assert!(
            !f.marketplace,
            "в новой папке настроек маркетплейса быть не должно"
        );
        assert!(decide(&f).blockers.is_empty(), "{:?}", decide(&f).blockers);
        let (kit, fresh) = open(&config, &f, false).unwrap();
        assert!(fresh);
        let again = super::facts(&config, &status, &repo);
        println!(
            "kit {} version {:?}",
            kit.display(),
            again.installed_version
        );
        assert!(again.ours(), "маркетплейс — папка PGC");
        assert_eq!(again.installed_version.as_deref(), Some(version));
        assert!(decide(&again).update.is_none());
        // Случай 24 целиком: в проекте старая версия (её ставим из подменённой папки PGC),
        // кнопка «обновить» возвращает встроенную.
        let dir = crate::kit::dir().unwrap();
        let manifest = dir.join(".claude-plugin").join("plugin.json");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, text.replace(version, "old000000000")).unwrap();
        std::fs::write(dir.join(".pgc-kit"), "old000000000").unwrap();
        run_claude(
            &config,
            &repo,
            update_steps(again.installed_scope.as_deref()),
        )
        .unwrap();
        let stale = super::facts(&config, &status, &repo);
        assert_eq!(stale.installed_version.as_deref(), Some("old000000000"));
        assert!(
            decide(&stale).update.is_some(),
            "должна быть кнопка «обновить»"
        );
        let (_, fresh) = update(&config, &stale).unwrap();
        assert!(fresh);
        let updated = super::facts(&config, &status, &repo);
        assert_eq!(updated.installed_version.as_deref(), Some(version));
        assert!(decide(&updated).update.is_none());
        unsafe {
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("CLAUDE_GATEWAY_HOME");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Полнота: у каждой строки таблицы ADR-014 есть тест `case_NN_…`.
    const PROPOSAL: &str = r#"{"format": 1, "via": "code", "model": "",
        "checks": [{"id": "cargo:test", "cmd": "cargo test", "kind": "test", "source": {"file": "Cargo.toml", "line": 1}}],
        "struck": [{"id": "npm:bench", "cmd": "npm run bench", "kind": "other", "why": "the name says bench"}],
        "usage": {"input": 0, "output": 0}}"#;

    #[test]
    fn case_25_checks_ticked_and_struck_with_a_reason() {
        let p = parse_proposal(PROPOSAL).unwrap();
        assert_eq!(p.checks[0].cmd, "cargo test");
        assert_eq!(p.checks[0].source.as_ref().unwrap().file, "Cargo.toml");
        assert_eq!(p.struck[0].why, "the name says bench");
        assert!(!p.checks[0].present);
        let own = parse_proposal(
            r#"{"checks": [{"id": "npm:test", "cmd": "npm test", "present": true}]}"#,
        );
        assert!(own.unwrap().checks[0].present, "уже в своём verify.json");
        let seen =
            parse_proposal(r#"{"checks": [{"id": "npm:test", "cmd": "npm test", "caught": 3}]}"#);
        assert_eq!(seen.unwrap().checks[0].caught, 3, "журнал опыта");
        assert_eq!(p.checks[0].caught, 0, "без журнала поля нет");
        // Без `--model` в команде: файлы проекта никуда не уходят.
        let source = include_str!("connect.rs");
        let call = source.split("pub fn propose(").nth(1).unwrap();
        let call = &call[..call.find("\n}").unwrap()];
        assert!(call.contains("\"--propose\"") && !call.contains("--model"));
        // Тексты для человека — на языке окна PGC.
        let flat: String = call.split_whitespace().collect();
        assert!(flat.contains("\"--lang\",lang(),"));
    }

    #[test]
    fn case_26_an_old_kit_or_an_unreadable_answer() {
        assert!(parse_proposal("Usage: agent-kit setup …").is_err());
        assert!(
            parse_proposal(r#"{"items": []}"#).is_err(),
            "ответ setup без --propose"
        );
        let empty = parse_proposal(r#"{"checks": []}"#).unwrap();
        assert!(empty.checks.is_empty() && empty.struck.is_empty());
    }

    #[test]
    fn case_27_only_ids_of_the_proposal_are_passed() {
        let p = parse_proposal(PROPOSAL).unwrap();
        let chosen = ["npm:bench", "cargo:test", "make:deploy"].map(String::from);
        assert_eq!(chosen_checks(&p, &chosen), ["cargo:test", "npm:bench"]);
        assert!(chosen_checks(&p, &[]).is_empty());
    }

    #[test]
    fn case_30_the_critic_draft_is_unticked_with_its_rules() {
        let plan = parse_plan(
            r#"{"items": [
                {"id": "critic-md", "action": "create", "path": ".claude/critic.md", "default": false,
                 "rules": ["Все суммы — только целые копейки."], "what": "Создать .claude/critic.md"},
                {"id": "issues-log", "action": "create", "path": "docs/knowledge/issues.md"}]}"#,
        )
        .unwrap();
        assert!(
            !plan[0].default && plan[1].default,
            "без поля — с галочкой, как раньше"
        );
        assert_eq!(plan[0].rules, ["Все суммы — только целые копейки."]);
        assert!(
            plan[0].about.is_empty(),
            "без пояснения — пусто, «?» не показывается"
        );
        let told = parse_plan(
            r#"{"items": [{"id": "gate", "action": "create", "about": "Не даёт закончить без проверок."}]}"#,
        );
        assert_eq!(told.unwrap()[0].about, "Не даёт закончить без проверок.");
        // Откуда взято правило: файл и строка, по одному на правило.
        let sourced = parse_plan(
            r#"{"items": [{"id": "critic-md", "action": "create", "rules": ["a"], "from": [{"file": "README.md", "line": 12}]}]}"#,
        );
        let sourced = sourced.unwrap();
        assert_eq!(sourced[0].from[0].file, "README.md");
        assert_eq!(sourced[0].from[0].line, 12);
        // Применяется, только когда человек его отметил: окно передаёт id лишь тогда.
        assert_eq!(applicable(&plan, &strings(&["issues-log"])), ["issues-log"]);
        assert_eq!(
            applicable(&plan, &strings(&["critic-md", "issues-log"])),
            ["critic-md", "issues-log"]
        );
    }

    fn item(id: &str, action: &str) -> PlanItem {
        PlanItem {
            id: id.into(),
            action: action.into(),
            path: id.into(),
            what: String::new(),
            default: true,
            rules: Vec::new(),
            about: String::new(),
            title: String::new(),
            group: String::new(),
            from: Vec::new(),
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn case_28_an_own_verify_file_gets_the_steps_as_text() {
        let plan = [item("AGENTS.md", "create"), item("pre-push", "keep")];
        let args = apply_args(&plan, &strings(&["AGENTS.md"]), &strings(&["cargo:test"])).unwrap();
        assert_eq!(
            args,
            strings(&[
                "setup",
                "--apply",
                "--only",
                "AGENTS.md,verify-checks",
                "--checks",
                "cargo:test"
            ])
        );
        // Только проверки, без пунктов плана — тоже применяется.
        let args = apply_args(&plan, &[], &strings(&["cargo:test", "npm:bench"])).unwrap();
        assert_eq!(args[3], "verify-checks");
        assert_eq!(args[5], "cargo:test,npm:bench");
        // Ничего не отмечено — нечего и запускать.
        assert!(apply_args(&plan, &strings(&["pre-push"]), &[]).is_none());
    }

    #[test]
    fn case_29_ticked_checks_bring_the_verify_file_along() {
        let plan = [item(VERIFY_FILE, "create"), item("deny-secrets", "add")];
        let args = apply_args(
            &plan,
            &strings(&["deny-secrets"]),
            &strings(&["cargo:test"]),
        )
        .unwrap();
        assert_eq!(args[3], format!("deny-secrets,{VERIFY_FILE}"));
        let args = apply_args(&plan, &strings(&[VERIFY_FILE]), &strings(&["cargo:test"])).unwrap();
        assert_eq!(args[3], VERIFY_FILE, "не дважды");
        // Без проверок файл создаётся только по своей галочке, с шагами, найденными setup.
        let args = apply_args(&plan, &strings(&["deny-secrets"]), &[]).unwrap();
        assert_eq!(
            args,
            strings(&["setup", "--apply", "--only", "deny-secrets"])
        );
    }

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
