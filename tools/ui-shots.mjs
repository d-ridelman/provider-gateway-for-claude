// UI snapshots of the gateway window: the real ui/ files in headless Chrome with a stub of the
// Tauri API. Light and dark theme, 1920 / 1280 / 900 wide, and the states of every screen
// (empty, error, loading, long text, warning). Files go to state/shots/, plus report.json with
// measurements (horizontal scroll, clipped text, panels that need scrolling).
//
//   node tools/ui-shots.mjs [--only <substring of the shot name>] [--out <dir>]
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const uiDir = path.join(root, "crates", "ctp-app", "ui");
// The stub's version comes from Cargo.toml: a copy in the stub drifts with every release.
const VERSION =
  /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(root, "Cargo.toml"), "utf8"))?.[1] ?? "0.0.0";
const args = process.argv.slice(2);
const opt = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
const outDir = path.resolve(opt("--out", path.join(root, "state", "shots")));
const only = opt("--only", "");
const CHROME = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
].find((p) => fs.existsSync(p));
if (!CHROME) throw new Error("Chrome or Edge not found");

// ---- the stub of window.__TAURI__ that the page gets before its own scripts -------------
const STUB = String.raw`
(() => {
  const q = new URLSearchParams(location.search);
  const scenario = q.get("s") || "normal";
  let lang = q.get("lang") || "ru";
  // forcedlight / forceddark: the user chose a theme in the app, whatever Windows says.
  let themePref = scenario === "forcedlight" ? "light" : scenario === "forceddark" ? "dark" : "auto";
  const long = scenario === "long";
  const alias = (p, m, i) => ({ model: m, alias: "claude-" + p + "-" + (1000 + i), supports1m: m.includes("[1m]"), label: m.replace("[1m]", "") });
  const mk = (id, name, base, models, key = true) => ({
    id, name, kind: "openai", baseUrl: base, models, hasKey: key, needsKey: true,
    aliases: models.map((m, i) => alias(id, m, i)), maxEffort: "max", defaultEffort: null, labels: {},
  });
  // docs: neutral demo data for the screenshots in README (no real provider names or addresses).
  const demo = () => {
    const ollama = mk("ollama", "Ollama", "http://127.0.0.1:11434/v1", ["qwen3:8b"], false);
    ollama.needsKey = false;
    return [
      mk("openrouter", "OpenRouter", "https://openrouter.ai/api/v1", ["deepseek/deepseek-v4-pro[1m]", "z-ai/glm-5.3", "qwen/qwen4-max"]),
      mk("deepseek", "DeepSeek", "https://api.deepseek.com", ["deepseek-v4-flash[1m]", "deepseek-v4-pro[1m]"]),
      ollama,
    ];
  };
  let providers = scenario === "empty" ? [] : scenario === "docs" ? demo() : long ? [
    mk("very-long-provider-identifier-for-the-gateway", "A provider with a name that is much longer than the sidebar can ever show without wrapping", "https://example.com/some/very/long/path/that/keeps/going/and/going/and/going/for/a/while/v1",
      ["a-model-with-an-extremely-long-identifier-that-should-not-break-the-layout-1234567890[1m]", "short"], false),
    mk("second", "Second", "https://api.example.com/v1", ["m1", "m2", "m3"]),
  ] : [
    mk("opencode", "OpenCode", "https://opencode.ai/inference/go/v1", ["deepseek-v4.1-flash[1m]", "qwen4-max[1m]", "gemini-4-argon[1m]", "claude-haiku-5-5[1m]", "glm-5.3-flash[1m]", "glm-5.3[1m]"]),
    mk("together", "Together AI", "https://api.together.xyz/v1", ["moonshotai/kimi-k2.5"]),
    mk("zai", "Z.ai", "https://api.z.ai/api/anthropic", ["glm-5.3"]),
  ];
  const models = () => providers.flatMap((p) => p.aliases.map((a) => ({
    alias: a.alias, model: a.model, provider: p.id, supports1m: a.supports1m, maxEffort: "max", openai: true, label: a.label })));
  const items = scenario === "warn" ? [
    { ok: true, text: "Rule for model selection read from Claude", optional: false },
    { ok: false, text: "Gateway model names fall under Claude's ban", optional: false },
    { ok: true, text: "Model config fields are in place", optional: false },
  ] : [
    { ok: true, text: "Rule for model selection read from Claude (as in the previous version)", optional: false },
    { ok: true, text: "Gateway model names do not fall under Claude's ban", optional: false },
    { ok: true, text: "Model config fields are in place", optional: false },
  ];
  const home = long ? "C:\\Users\\a-user-with-a-very-long-name-that-goes-on\\AppData\\Local\\AnthropicClaude\\app-6.28899.6\\claude.exe" : "C:\\Users\\user\\AppData\\Local\\AnthropicClaude\\app-6.28899.6\\claude.exe";
  const cfg = () => ({
    port: 17899, token: "ctp-0000000000000000000000000000000000000", claudePath: null, detectedClaude: home, providers,
    russianUi: false, builtinBrowser: false, artifacts: false, coworkMode: "host", russianFile: true, configDir: "C:\\Users\\user\\AppData\\Roaming\\app.provider-gateway-for-claude",
    codeDir: null, language: lang, languagePref: "auto", themePref, autoUpdate: true, version: "${VERSION}",
    modelRoles: {
      sonnet: providers.length ? { provider: providers[0].id, model: providers[0].models[0] } : null,
      backup: providers.length > 1 ? { provider: providers[1].id, model: providers[1].models[0] } : null,
    },
    codePortable: scenario === "portable",
    detectedCode: scenario === "empty" ? null : "C:\\Users\\user\\.local\\bin\\claude.exe",
    installed: scenario === "portable"
      ? { codePortable: "2.1.330", desktopPortable: "6.28899.6", desktopPortableExe: "C:\\Users\\user\\AppData\\Local\\ClaudeProviderGateway\\claude-desktop\\app-6.28899.6\\claude.exe", desktopInstalled: null }
      : { codePortable: null, desktopPortable: null, desktopPortableExe: null, desktopInstalled: scenario === "empty" ? null : home },
  });
  let running = scenario !== "empty";
  const status = () => ({ running, port: 17899, token: cfg().token, models: running ? models() : [], logPath: "C:\\logs\\gateway.log",
    claude: { version: "6.28899.6", items }, update: null });
  const delay = scenario === "slow" ? 1500 : 0;
  const fail = (msg) => Promise.reject(msg);
  const handlers = {
    get_config: () => cfg(), status: () => status(),
    set_ui: (a) => { if (a?.theme) themePref = a.theme; if (a?.language) lang = a.language; return cfg(); },
    claude_state: () => ({ running: scenario === "open" || scenario === "stale", stale: scenario === "stale" }),
    save_provider: () => scenario === "error" ? fail("HTTP 500: Internal Server Error (stubbed)") : cfg(),
    delete_provider: () => cfg(), fetch_models: () => ({ ids: ["m1", "m2"], catalog: {} }),
    test_provider: () => scenario === "error" ? fail("HTTP 500: Internal Server Error (stubbed)") : "OK",
    save_settings: () => cfg(), start_gateway: () => { running = true; return status(); }, stop_gateway: () => { running = false; return status(); },
    launch_claude: () => scenario === "error" ? fail("Could not start claude.exe: access denied (stubbed)") : ({ message: "Claude started (PID 4242)", warning: false }),
    launch_claude_code: () => "Claude Code opened", tail_log: () => "2026-11-22T10:00:00Z INFO gateway started port=17899\n".repeat(30),
    open_logs: () => null, copy_text: () => null, pick_folder: () => null, app_exit: () => null, check_update: () => ({ available: false }),
    install_update: () => null, win_minimize: () => null, win_toggle_maximize: () => null, win_close: () => null,
    // Downloads: progress events while the answer waits (the "downloading" shot catches the middle).
    download_code: () => downloadStub("code"), download_desktop: () => downloadStub("desktop"),
    // Proxy: "vpn" — on with two subscriptions; "vpnerror" — the system proxy blocked by a policy;
    // everything else — off, core not downloaded.
    proxy_state: () => proxy, proxy_set_mode: (a) => { proxy = { ...proxy, mode: a.mode }; return proxy; },
    proxy_download_core: () => downloadStub("core").then(() => ({ ...proxy, coreVersion: "v1.19.34" })),
    proxy_add_sub: () => scenario === "error" ? fail("The subscription address must start with https:// or http://") : proxy,
    proxy_remove_sub: () => proxy, proxy_update_sub: () => proxy, proxy_select: (a) => { proxy = { ...proxy, selected: a.name }; return proxy; },
    proxy_measure: () => proxy,
    // Connect a project (agent-kit): "kitnonode" — no Node; "kitnorepo" — not a git repository;
    // "kitold" — an installed copy that cannot give a plan; "kitlong" — a long path; "error" — the
    // install fails; everything else — ready, then the inspection and a plan.
    connect_check: () => kitCheck(),
    connect_open: () => { if (scenario === "error") return fail("claude plugin install: marketplace 'agent-kit' not found (stubbed)"); kitConnected = true; return kitData(); },
    connect_apply: () => ({ ...kitData(), plan: [kitPlan[2]], applied: { applied: ["docs/knowledge/issues.md"], skipped: [] } }),
    connect_set_source: () => null,
    // "kitundo" — a connected project and its disconnect plan (ADR-017).
    connect_undo_open: () => kitUndo(),
    connect_undo_apply: () => { kitConnected = false; return { ...kitUndo(), applied: { applied: ["AGENTS.md", "plugin-local"], skipped: [] } }; },
    connect_update: () => kitData(),
  };
  // Connect turns the plugin on: a check after it says "connected" (the Back button checks again).
  let kitConnected = scenario === "kitupdate" || scenario === "kitundo";
  const kitRoot = scenario === "kitlong"
    ? "D:\\Projects\\Очень длинное имя папки клиента с пробелами\\packages\\web-frontend-monorepo-workspace"
    : "D:\\Projects\\my-project";
  const kitNote = (id, text) => ({ id, text });
  const kitCheck = () => ({
    root: scenario === "kitnorepo" ? null : kitRoot,
    blockers: scenario === "kitsource"
      ? [kitNote("source", lang === "ru" ? "Неизвестно, откуда взять agent-kit: его маркетплейс не добавлен в Claude Code, а источник в PGC не задан." : "Where to get agent-kit is unknown: its marketplace is not added to Claude Code and PGC has no source set.")]
      : scenario === "kitnonode"
      ? [kitNote("node", lang === "ru" ? "Нет Node.js: agent-kit работает на Node 22 или новее. Поставьте его с nodejs.org и нажмите «Проверить» ещё раз." : "Node.js is missing: agent-kit needs Node 22 or newer. Install it from nodejs.org and press Check again.")]
      : scenario === "kitnorepo"
        ? [kitNote("repo", lang === "ru" ? "Эта папка — не git-репозиторий. agent-kit сравнивает изменения через git: откройте папку проекта с git." : "This folder is not a git repository. agent-kit compares changes through git: pick a project folder with git.")]
        : [],
    notes: [kitNote("critic", lang === "ru" ? "Критик в PGC не выбран: критик agent-kit пойдёт по подписке Claude, а без неё не сработает. Выбрать — в Настройках." : "No critic is chosen in PGC: agent-kit's critic goes through a Claude subscription and fails without one. Choose one in the settings."),
      kitNote("dirty", lang === "ru" ? "В проекте есть незакоммиченные правки. Подключение их не тронет, новые файлы лягут рядом." : "The project has uncommitted edits. Connecting leaves them alone; new files go next to them.")],
    connected: kitConnected, disabled: false,
    update: scenario === "kitupdate" ? { from: "92b16a29f355", to: "a1b2c3d4e5f6" } : null,
  });
  // "kitold" — an agent-kit that gives no title/group: the flat list of before.
  const titled = scenario !== "kitold";
  // agent-kit answers in the window's language (--lang), so the stub does too.
  const L = (ru, en) => (lang === "ru" ? ru : en);
  const steps = L("Шаблоны задания, приёмки и отчёта для задач больше одной правки.", "Templates of a task, its acceptance and its report, for work bigger than one edit.");
  const kitPlan = [
    { id: "issues-log", action: "create", path: "docs/knowledge/issues.md", what: L("Создать docs/knowledge/issues.md", "create docs/knowledge/issues.md"), ...(titled && { title: L("Журнал находок", "Findings log"), group: "docs", about: L("Сюда агент записывает то, что заметил вне задачи, вместо того чтобы молча чинить.", "The agent writes here what it noticed outside the task, instead of quietly fixing it.") }) },
    { id: "deny-secrets", action: "add", path: ".claude/settings.json", what: "deny reading secret files: Read(**/.env*), Read(**/*.pem), Read(**/*.key)", ...(titled && { title: L("Не читать пароли и ключи", "Do not read passwords and keys"), group: "safety", about: L("Агент не сможет открыть файлы, где обычно лежат пароли и ключи.", "The agent cannot open the files where passwords and keys usually live.") }) },
    { id: "gate", action: "create", path: ".claude/gates.json", what: "change gate in observe mode", about: L("Не даёт агенту закончить работу, пока он не прогнал проверки по своим правкам. Сначала только наблюдает и ничего не блокирует.", "Keeps the agent from finishing before it ran the checks on its edits. At first it only watches and blocks nothing."), ...(titled && { title: L("Не закончить без проверок", "No finishing without checks"), group: "checks" }) },
    { id: "critic-md", action: "create", path: ".claude/critic.md", default: false, rules: L(["Все суммы — только целые копейки.", "День отчёта считается по местному времени, а не по UTC."], ["Money amounts are whole cents only.", "A report day is counted in local time, not UTC."]), from: [{ file: "README.md", line: 12 }, { file: "docs/adr/007-report-day.md", line: 9 }], what: L("Создать .claude/critic.md из правил проекта (черновик кодом, правил — 2); по умолчанию без галочки", "create .claude/critic.md from the project's own rules (a draft by code: 2 rules); not ticked by default"), ...(titled && { title: L("Правила для критика", "Rules for the critic"), group: "checks" }) },
    { id: "agents-md", action: "create", path: "AGENTS.md", what: L("Создать AGENTS.md", "create AGENTS.md"), ...(titled && { title: L("Инструкция для агентов", "Instructions for agents"), group: "agent", about: L("Короткая инструкция для любого агента: как здесь собирать, проверять и что нельзя трогать.", "A short guide for any agent: how to build and check here, and what not to touch.") }) },
    // Two files under one title: one row, one tick.
    { id: "docs/steps/_TEMPLATE.md", action: "create", path: "docs/steps/_TEMPLATE.md", what: L("Создать docs/steps/_TEMPLATE.md", "create docs/steps/_TEMPLATE.md"), ...(titled && { title: L("Заготовки для больших задач", "Templates for big tasks"), group: "agent", about: steps }) },
    { id: "docs/steps/_TEMPLATE_CHECK.md", action: "create", path: "docs/steps/_TEMPLATE_CHECK.md", what: L("Создать docs/steps/_TEMPLATE_CHECK.md", "create docs/steps/_TEMPLATE_CHECK.md"), ...(titled && { title: L("Заготовки для больших задач", "Templates for big tasks"), group: "agent", about: steps }) },
    { id: "pre-push", action: "keep", path: ".githooks/pre-push", what: "the project's own hook: add the kit's checks to it", ...(titled && { title: L("Дополнить свою проверку перед отправкой", "Add to your own pre-push check"), group: "advice" }) },
  ];
  const kitData = () => ({
    root: kitRoot,
    inspect: { findings: [
      { id: "agents-file", status: "ok", what: "AGENTS.md: 82 lines" },
      { id: "slow-step", status: "ok", advice: true, step: "e2e", what: L("шаг e2e: прогонов — 20, ни разу не падал, около 90 с каждый: можно запускать реже (решает человек)", "step e2e: 20 runs, never failed, about 90 s each: it may run less often (a person decides)") },
      { id: "decisions", status: "ok", what: "decision log: docs/adr (+13)" },
      { id: "issues", status: "missing", what: L("Не хватает журнала находок (docs/knowledge/issues.md или TODO.md)", "no findings log (docs/knowledge/issues.md or TODO.md)"), fix: L("находки вне задачи — туда, а не в тихую правку", "findings outside the task go there, not into a quiet fix") },
      { id: "gate", status: "missing", what: L("Не хватает .claude/gates.json", "no .claude/gates.json"), fix: L("сначала шлюз в режиме наблюдения", "a gate in observe mode first"), about: L("Без него агент может сказать «готово», не запустив ни одной проверки.", "Without it the agent can say “done” without running a single check.") },
      { id: "skills", status: "problem", what: L("some-plugin: skills/x/: описание говорит, когда применять, но не когда не применять", "some-plugin: skills/x/: the description says when to use it, not when not to"), fix: L("допишите, когда не применять", "add when not to use it") },
    ] },
    // "kitown" — the project has its own verify.json, and the steps are shown to add by hand.
    plan: scenario === "kitold" ? null : scenario === "kitown" ? kitPlan : [{ id: ".claude/verify.json", action: "create", path: ".claude/verify.json", what: "the checks of this project" }, ...kitPlan],
    planError: scenario === "kitold" ? "agent-kit setup: unknown option --json" : null,
    checks: scenario === "kitold" ? null : kitChecks,
    fresh: true,
    applied: scenario === "kitown" ? { applied: [], skipped: [{ id: "verify-checks", why: 'advice for a person, nothing to apply: .claude/verify.json is the project\'s own and is never rewritten: add the ticked checks to its steps by hand: {"name":"npm-test","cmd":"npm test"}, {"name":"cargo-clippy","cmd":"cargo clippy --all-targets"}' }] } : null,
  });
  const kitUndo = () => ({
    root: kitRoot, kitOld: false, applied: null,
    items: [
      { id: "AGENTS.md", action: "remove", path: "AGENTS.md", title: L("Инструкция для агентов", "Instructions for agents"), about: L("Короткая инструкция для любого агента.", "A short guide for any agent.") },
      { id: ".claude/gates.json", action: "remove", path: ".claude/gates.json", title: L("Не закончить без проверок", "No finishing without checks") },
      { id: ".claude/verify.json", action: "keep", path: ".claude/verify.json", title: L("Список проверок проекта", "The project's checks"), what: L("вы меняли файл — не трогаю", "you changed this file — left alone") },
      { id: "deny-secrets", action: "unadd", path: ".claude/settings.json", title: L("Не читать пароли и ключи", "Do not read passwords and keys") },
      { id: "plugin-local", action: "plugin", path: ".claude/settings.local.json", default: true, title: L("Выключить agent-kit в этом проекте", "Turn agent-kit off in this project"), about: L("Только у вас.", "For you only.") },
      { id: "plugin-team", action: "plugin", path: ".claude/settings.json", default: false, title: L("Выключить для всей команды (в git проекта)", "Turn it off for the whole team (in the project's git)") },
    ],
  });
  const src = (file, line) => ({ file, line });
  const kitChecks = {
    checks: [
      { id: "npm:test", cmd: "npm test", kind: "test", source: src("package.json", 7), present: scenario === "kitown", caught: 3 },
      { id: "cargo:clippy", cmd: "cargo clippy --all-targets", kind: "lint", source: src("Cargo.toml", 1) },
      scenario === "kitlong"
        ? { id: "npm:packages/web-frontend-monorepo-workspace#test:integration", cmd: "npm --prefix packages/web-frontend-monorepo-workspace run test:integration -- --runInBand --coverage", kind: "test", source: src("packages/web-frontend-monorepo-workspace/package.json", 24) }
        : { id: "ci:e50f3703cb", cmd: "npx tsc --noEmit", kind: "typecheck", source: src(".github/workflows/ci.yml", 31) },
    ],
    struck: [
      { id: "npm:lint:fix", cmd: "npm run lint:fix", kind: "other", source: src("package.json", 9), ruled: "fix", why: "name rule" },
      { id: "ci:a1b2c3d4e5", cmd: "npm test -- --shard \${{ matrix.n }}", kind: "test", source: src(".github/workflows/ci.yml", 40), why: L("шаг CI, которому нужен сам CI (значение из matrix)", "a CI step that needs the CI itself (a matrix value)") },
    ],
  };
  const vpn = scenario === "vpn" || scenario === "vpnerror" || scenario === "vpnlong";
  const node = (sub, label, kind, delay) => ({ name: sub + "|" + label, label, sub, kind, delay });
  const longName = "🇳🇱 Netherlands · Amsterdam · premium node with an unusually long name 01";
  let proxy = vpn ? {
    mode: scenario === "vpnerror" ? "system" : "app", running: scenario !== "vpnerror", tun: false, port: 7890,
    coreVersion: "v1.19.34", pinnedVersion: "v1.19.34", selected: "s1|Finland C", autoNow: "s1|Germany B",
    nodes: [
      node("s1", scenario === "vpnlong" ? longName : "Netherlands A", "vless", 0), node("s1", "Germany B", "vless", 87),
      node("s1", "Finland C", "trojan", 142), node("s1", "USA West", "ss", 612), node("s2", "Japan Tokyo", "hysteria2", null),
      node("s2", "Singapore", "vmess", 1240),
    ],
    subs: [
      { id: "s1", name: scenario === "vpnlong" ? "A very long subscription name from the provider's panel that goes on" : "Home VPN", host: "panel.example.com:8443", count: 4, updatedAt: "2026-11-23T14:05:32+04:00", upload: 1073741824, download: 2147483648, total: 10737418240, expire: 1893456000 },
      { id: "s2", name: "Backup", host: "sub.example.org", count: 2, updatedAt: "2026-11-23T09:00:00+04:00", upload: null, download: null, total: null, expire: null },
    ],
    // The backend sends its texts in the window's language.
    error: scenario !== "vpnerror" ? null : lang === "ru"
      ? "настройки прокси Windows закреплены групповой политикой — системный прокси здесь не включить; режим «Только Claude и провайдеры» работает"
      : "Windows proxy settings are locked by a group policy, so the system proxy cannot be set here; the Claude and providers only mode works",
    system: scenario === "vpnerror" ? { on: false, error: null } : null,
  } : { mode: "off", running: false, tun: false, port: 0, coreVersion: null, pinnedVersion: "v1.19.34", selected: null, autoNow: null, nodes: [], subs: [], error: null, system: null };
  const listeners = {};
  const downloadStub = async (id) => {
    for (let done = 0; done <= 100; done += 25) {
      (listeners.download ?? []).forEach((f) => f({ payload: { id, done: done * 2621440, total: 262144000 } }));
      await new Promise((r) => setTimeout(r, 300));
    }
    if (scenario === "error") return fail("sha1 mismatch: expected 3449da9c…, got 00000000… (stubbed)");
    return { message: "Portable " + id + " is ready", config: cfg() };
  };
  const invoke = async (cmd, a) => {
    if (delay) await new Promise((r) => setTimeout(r, delay));
    const h = handlers[cmd];
    if (!h) return fail("unknown command " + cmd);
    return h(a);
  };
  const listen = async (name, f) => { (listeners[name] ??= []).push(f); return () => {}; };
  window.__TAURI__ = { core: { invoke }, event: { listen } };
  // ?view=settings opens Settings by itself (for scanners that only load the URL).
  if (q.get("view") === "settings") window.addEventListener("load", () => setTimeout(() => document.getElementById("settingsBtn")?.click(), 500));
  if (q.get("view") === "proxy") window.addEventListener("load", () => setTimeout(() => document.getElementById("proxyBtn")?.click(), 500));
  // ?view=project: the panel with the check done and the inspection open.
  if (q.get("view") === "project") window.addEventListener("load", () => setTimeout(async () => {
    document.getElementById("projectBtn")?.click();
    await new Promise((r) => setTimeout(r, 200));
    document.getElementById("projectDirInput").value = "D:\\\\Projects\\\\my-project";
    document.getElementById("projectCheckBtn").click();
    await new Promise((r) => setTimeout(r, 200));
    document.getElementById("projectConnectBtn")?.click();
  }, 500));
})();
`;

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://x");
  if (url.pathname === "/__stub.js") {
    res.writeHead(200, { "content-type": "text/javascript" });
    return res.end(STUB);
  }
  const rel = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
  const file = path.join(uiDir, rel);
  if (!file.startsWith(uiDir) || !fs.existsSync(file)) {
    res.writeHead(404);
    return res.end();
  }
  let body = fs.readFileSync(file);
  if (rel === "index.html") body = Buffer.from(body.toString("utf8").replace("<head>", '<head><script src="/__stub.js"></script>'));
  const type = { ".js": "text/javascript", ".css": "text/css", ".html": "text/html; charset=utf-8" }[path.extname(file)] ?? "application/octet-stream";
  res.writeHead(200, { "content-type": type });
  res.end(body);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

// ---- the shots ------------------------------------------------------------------------------
// screen: a snippet run in the page after load (open settings, a dialog, ...).
const SCREENS = {
  main: "",
  new: "document.getElementById('addBtn').click()",
  settings: "document.getElementById('settingsBtn').click()",
  dialog: "document.getElementById('deleteBtn').click()",
  launch: "document.getElementById('launchBtn').click()",
  // A disabled button under the pointer must stay readable (it was dark text on a dark hover fill).
  hoverlaunch: "hover:#launchBtn",
  hoverstop: "hover:#stopBtn",
  hoverenabled: "hover:#stopBtn",
  hoverprimary: "hover:#emptyAdd",
  hoverdanger: "hover:#deleteBtn",
  hoveravatar: "hover:#addBtn",
  hoverprovider: "hover:.provider-item",
  // The dim lines inside a hovered list item are the worst case, not the item's own text.
  hoverurl: "hover:.provider-item .url",
  hovermeta: "hover:.provider-item .meta",
  hoversettings: "hover:#settingsBtn",
  hovertheme: "hover:#themeBtn",
  hoverlang: "hover:#langBtn",
  hoverlogs: "hover:#logsBtn",
  // An unsaved edit, then the language switch in the title bar: it must ask before it drops the edit.
  dirtylang: "(() => { const n = document.getElementById('nameInput'); n.value = 'Edited name'; n.dispatchEvent(new Event('input', { bubbles: true })); document.getElementById('langBtn').click(); })()",
  // The scrolled-to-the-end card: head and action bar stay, only the middle moves.
  // (Settings fills its fields after the click: wait for that, or the edit is overwritten.)
  settingsbottom: "(async () => { document.getElementById('settingsBtn').click(); await new Promise((r) => setTimeout(r, 400)); const b = document.querySelector('#settingsPanel .card-body'); b.scrollTop = b.scrollHeight; })()",
  formbottom: "(async () => { const b = document.querySelector('#providerForm .card-body'); b.scrollTop = b.scrollHeight; })()",
  // An edited port: Save wakes up and the bar says there is something to save.
  settingsdirty: "(async () => { document.getElementById('settingsBtn').click(); await new Promise((r) => setTimeout(r, 400)); const p = document.getElementById('portInput'); p.value = '18000'; p.dispatchEvent(new Event('input', { bubbles: true })); })()",
  testconn: "[...document.querySelectorAll('button')].find((b) => /Проверить связь|Test connection/.test(b.textContent))?.click()",
  // A portable download in the middle: busy button, the other one off, progress in the hint.
  downloading: "(async () => { document.getElementById('settingsBtn').click(); await new Promise((r) => setTimeout(r, 400)); document.getElementById('desktopPortableBtn').click(); document.getElementById('codePortableBtn').click(); await new Promise((r) => setTimeout(r, 700)); })()",
  proxy: "document.getElementById('proxyBtn').click()",
  // The question before a whole-PC mode.
  proxyask: "(async () => { document.getElementById('proxyBtn').click(); await new Promise((r) => setTimeout(r, 400)); document.querySelector('input[name=proxyMode][value=tun]').click(); })()",
  // Connect a project: the check, then the inspection and the plan.
  projectempty: "document.getElementById('projectBtn').click()",
  project: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); })()",
  // Another folder typed after connecting: nothing of the old project may stay on the page.
  projectreset: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectConnectBtn').click(); await new Promise((r) => setTimeout(r, 1500)); const input = document.getElementById('projectDirInput'); input.value = 'D:\\\\Projects\\\\other'; input.dispatchEvent(new Event('input')); })()",
  // A connected project: Check, then Disconnect… — the plan with ticks and Back.
  projectundo: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectUndoBtn').click(); })()",
  // Check, Connect, then Back: the folder is checked again and offers Disconnect.
  projectback: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectConnectBtn').click(); await new Promise((r) => setTimeout(r, 400)); document.getElementById('projectBackBtn').click(); })()",
  // Disconnect the ticked: back to the folder, which can be connected again.
  projectundone: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectUndoBtn').click(); await new Promise((r) => setTimeout(r, 400)); document.getElementById('projectUndoApplyBtn').click(); })()",
  // The help window over the project page.
  help: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.querySelector('[data-help=project] .help-btn').click(); })()",
  projectopen: "(async () => { document.getElementById('projectBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectDirInput').value = 'D:\\\\Projects\\\\my-project'; document.getElementById('projectCheckBtn').click(); await new Promise((r) => setTimeout(r, 300)); document.getElementById('projectConnectBtn').click(); })()",
  // Installing over a regular Claude asks first.
  installask: "(async () => { document.getElementById('settingsBtn').click(); await new Promise((r) => setTimeout(r, 400)); document.getElementById('desktopInstallBtn').click(); })()",
};
const W = [[1920, 1080], [1280, 800], [900, 700], [1000, 680], [1100, 740]];
const shots = [];
const add = (scenario, screen, widths, schemes, lang = "ru", extra = {}) => {
  for (const wi of widths) for (const scheme of schemes) shots.push({ scenario, screen, size: W[wi], scheme, lang, ...extra });
};
add("normal", "main", [0, 1, 2], ["light", "dark"]);
add("empty", "main", [0, 1], ["light", "dark"]);
add("normal", "settings", [0, 1, 2], ["light", "dark"]);
add("normal", "settings", [1], ["light", "dark"], "en");
add("normal", "settingsbottom", [1, 2], ["light", "dark"]);
add("normal", "settingsdirty", [1], ["light", "dark"]);
add("normal", "formbottom", [2], ["dark"]);
add("long", "settingsbottom", [2], ["dark"]);
add("normal", "new", [1], ["dark"]);
add("normal", "dialog", [1], ["dark"]);
add("normal", "dirtylang", [1], ["dark"]);
add("empty", "hoverlaunch", [1], ["light", "dark"]);
add("empty", "hoverstop", [1], ["light", "dark"]);
// Enabled buttons under the pointer: every variant stays readable too.
add("empty", "hoverprimary", [1], ["light", "dark"]);
add("normal", "hoverenabled", [1], ["light", "dark"]);
add("normal", "hoverdanger", [1], ["light", "dark"]);
add("normal", "hoveravatar", [1], ["light", "dark"]);
for (const screen of ["hoverprovider", "hoverurl", "hovermeta", "hoversettings", "hovertheme", "hoverlang", "hoverlogs"]) add("normal", screen, [1], ["light", "dark"]);
add("long", "main", [1], ["light", "dark"]);
add("long", "settings", [1], ["dark"]);
add("error", "launch", [1], ["dark"]);
add("error", "testconn", [1], ["light"]);
add("slow", "main", [1], ["light"], "ru", { settleMs: 400 });
add("warn", "main", [1], ["light"]);
add("open", "main", [1], ["dark"]);
// The user's own theme wins over Windows: dark window on a light system and the other way round.
add("forceddark", "main", [1], ["light"]);
add("forcedlight", "settings", [1], ["dark"]);
add("stale", "main", [0, 1, 2], ["light", "dark"]);
// Downloads of Claude: portable ones present, a download running, the question before installing.
add("portable", "settings", [0, 1], ["light", "dark"]);
add("normal", "downloading", [1], ["light", "dark"], "ru", { settleMs: 900 });
add("normal", "installask", [1], ["dark"]);
add("empty", "settings", [1], ["dark"], "en");
// Proxy and VPN: off with no core, on with nodes, a policy error, long names, the question, the chip.
add("normal", "proxy", [1], ["light", "dark"]);
add("vpn", "proxy", [0, 1, 2], ["light", "dark"]);
add("vpn", "proxy", [1], ["dark"], "en");
add("vpnerror", "proxy", [1], ["light", "dark"]);
add("vpnlong", "proxy", [1, 2], ["dark"]);
add("vpn", "proxyask", [1], ["dark"]);
add("vpn", "main", [1], ["light", "dark"]);
add("vpnerror", "main", [1], ["dark"]);
// Connect a project: empty panel, ready, blockers, the inspection with a plan, an old copy,
// a long path, a failed install, English.
add("normal", "projectempty", [1], ["light", "dark"]);
add("normal", "project", [0, 1, 2], ["light", "dark"]);
add("kitnonode", "project", [1], ["light", "dark"]);
add("kitnorepo", "project", [1], ["dark"]);
add("kitsource", "project", [1], ["light", "dark"]);
add("kitupdate", "projectopen", [1], ["light", "dark"], "ru", { settleMs: 400 });
add("normal", "projectopen", [0, 1, 2], ["light", "dark"], "ru", { settleMs: 400 });
add("kitold", "projectopen", [1], ["dark"], "ru", { settleMs: 400 });
add("kitown", "projectopen", [1], ["light", "dark"], "ru", { settleMs: 400 });
add("normal", "projectreset", [1], ["dark"], "ru", { settleMs: 900 });
// Waiting: the answer is slow, the line shows the brand's work mark.
add("slow", "projectopen", [1], ["light", "dark"], "ru", { settleMs: 400 });
add("normal", "help", [1], ["light", "dark"], "ru", { settleMs: 1000 });
add("kitundo", "projectundo", [1], ["light", "dark"], "ru", { settleMs: 700 });
add("kitundo", "projectundo", [1], ["dark"], "en", { settleMs: 700 });
add("normal", "projectback", [1], ["dark"], "ru", { settleMs: 900 });
add("kitundo", "projectundone", [1], ["dark"], "ru", { settleMs: 1200 });
add("normal", "help", [1], ["dark"], "en", { settleMs: 1000 });
add("kitlong", "projectopen", [1, 2], ["dark"], "ru", { settleMs: 400 });
add("error", "projectopen", [1], ["dark"], "ru", { settleMs: 400 });
add("normal", "projectopen", [1], ["dark"], "en", { settleMs: 400 });
// The README screenshots (docs/images): neutral demo providers; Settings on a taller window so the
// whole page is in the frame. `node tools/ui-shots.mjs --only docs- --out state/docs-shots`.
W.push([1280, 1140]);
for (const lang of ["en", "ru"]) {
  add("docs", "main", [1], ["dark"], lang);
  add("docs", "new", [1], ["dark"], lang);
  add("docs", "settings", [5], ["dark"], lang);
  add("docs", "projectopen", [5], ["dark"], lang, { settleMs: 600 });
}
// Windows display scaling shrinks the window to fewer CSS pixels (1500 px at 150 % is 1000) and puts
// edges between device pixels: the widths of 1000 / 1100 px, and fractional device scales (x1.25, x1.5).
add("normal", "main", [3, 4], ["light", "dark"]);
add("normal", "settings", [3, 4], ["light", "dark"]);
add("normal", "settingsbottom", [3, 4], ["dark"]);
add("normal", "new", [3, 4], ["dark"]);
add("long", "settings", [3], ["dark"]);
add("normal", "settings", [1], ["light", "dark"], "ru", { scale: 1.25 });
add("normal", "settings", [3], ["light", "dark"], "ru", { scale: 1.5 });
add("normal", "new", [1], ["dark"], "ru", { scale: 1.25 });
add("normal", "new", [3], ["dark"], "ru", { scale: 1.5 });
add("normal", "main", [3], ["light"], "ru", { scale: 1.5 });

// ---- Chrome over the DevTools protocol -----------------------------------------------------
const profile = fs.mkdtempSync(path.join(os.tmpdir(), "ui-shots-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(CHROME, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run",
  "--disable-gpu", "--hide-scrollbars=false", "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let ws;
for (let i = 0; i < 50 && !ws; i++) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = list.find((t) => t.type === "page");
    if (page) ws = new WebSocket(page.webSocketDebuggerUrl);
  } catch { /* Chrome is starting */ }
  if (!ws) await sleep(200);
}
if (!ws) throw new Error("no DevTools page");
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  }
};
const cdp = (method, params = {}) => new Promise((resolve, reject) => {
  const n = ++id;
  pending.set(n, { resolve, reject });
  ws.send(JSON.stringify({ id: n, method, params }));
});
const evaluate = async (expression) => {
  const answer = await cdp("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  // A script error in the page would otherwise come back as a silent `undefined`.
  if (answer.exceptionDetails) throw new Error(answer.exceptionDetails.exception?.description ?? answer.exceptionDetails.text);
  return answer.result?.value;
};

const MEASURE = `(() => {
  const doc = document.documentElement;
  const clipped = [];
  for (const el of document.querySelectorAll('body *')) {
    const s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden' || el.closest('.hidden')) continue;
    if (el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 0 && !['INPUT','TEXTAREA','SELECT','SVG','svg'].includes(el.tagName)
        && s.overflowX !== 'auto' && s.overflowX !== 'scroll' && s.textOverflow !== 'ellipsis' && s.display !== 'inline') {
      clipped.push((el.id ? '#' + el.id : el.tagName.toLowerCase() + '.' + String(el.className).split(' ')[0]) + ' ' + el.scrollWidth + '>' + el.clientWidth);
    }
  }
  const panel = document.getElementById('settingsPanel');
  const scrollers = [...document.querySelectorAll('main, .card, .shell, #settingsPanel, .main, .content')].filter((el) => el.scrollHeight > el.clientHeight + 1 && getComputedStyle(el).overflowY !== 'visible')
    .map((el) => (el.id ? '#' + el.id : '.' + String(el.className).split(' ')[0]) + ' ' + el.scrollHeight + '>' + el.clientHeight);
  // The open card must sit whole inside the content area: its head and its action bar are never cut
  // (the bar used to be pushed past the bottom edge by a sticky offset).
  const card = [...document.querySelectorAll('.card')].find((c) => !c.classList.contains('hidden'));
  let cardClipped = false;
  if (card) {
    const host = card.parentElement.getBoundingClientRect();
    // The proxy card has no action bar: its own bottom edge is checked instead.
    const r = card.getBoundingClientRect(), bar = (card.querySelector('.form-actions') ?? card).getBoundingClientRect();
    cardClipped = r.top < host.top - 1 || r.bottom > host.bottom + 1 || bar.bottom > r.bottom + 1 || bar.bottom > innerHeight;
  }
  // Controls side by side in one form row (a .grid of fields with a button, a .key-row) end on the
  // same line: their bottom edges are measured, not eyeballed.
  const misaligned = [];
  for (const row of document.querySelectorAll('.grid.sub-form, .key-row, .node-grid')) {
    if (row.closest('.hidden')) continue;
    const controls = [...row.querySelectorAll(':scope > .field > input, :scope > .field > select, :scope > button, :scope > input')]
      .filter((c) => c.getBoundingClientRect().height > 0);
    const lines = {};
    for (const c of controls) { const r = c.getBoundingClientRect(); (lines[Math.round(r.top / 20)] ??= []).push(r); }
    for (const line of Object.values(lines)) {
      const bottoms = line.map((r) => Math.round(r.bottom)), heights = line.map((r) => Math.round(r.height));
      if (Math.max(...bottoms) - Math.min(...bottoms) > 1 || Math.max(...heights) - Math.min(...heights) > 1)
        misaligned.push((row.id || row.className) + ' ' + bottoms.join('/'));
    }
    // A grid row of a form: all its controls share one baseline whatever their top.
    if (row.classList.contains('sub-form')) {
      const bottoms = controls.map((c) => Math.round(c.getBoundingClientRect().bottom));
      const byRow = {}; for (const b of bottoms) (byRow[Math.round(b / 40)] ??= []).push(b);
      for (const group of Object.values(byRow)) if (Math.max(...group) - Math.min(...group) > 1) misaligned.push('sub-form bottoms ' + group.join('/'));
    }
  }
  // Two fields side by side in the models row (the textarea and the first select) start on one line.
  for (const row of document.querySelectorAll('.grid.row-models')) {
    const a = row.querySelector('textarea'), b = row.querySelector('select');
    if (row.closest('.hidden') || !a || !b) continue;
    const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
    if (rb.left > ra.right && Math.abs(ra.top - rb.top) > 1) misaligned.push('row-models tops ' + Math.round(ra.top) + '/' + Math.round(rb.top));
  }
  return { hscroll: doc.scrollWidth > innerWidth + 1, docH: doc.scrollHeight, winH: innerHeight, clipped: clipped.slice(0, 8), scrollers, cardClipped, misaligned,
           settingsVisible: !!panel && !panel.classList.contains('hidden') };
})()`;

// --audit: how consistent is the styling? Every visible button grouped by its variant classes with
// the distinct height / radius / font / padding found, plus the inventory of radii and font sizes.
const AUDIT = `(() => {
  const vis = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const buttons = {};
  for (const el of document.querySelectorAll('button')) {
    if (!vis(el)) continue;
    const s = getComputedStyle(el);
    const kind = [...el.classList].filter((c) => !['hidden','show'].includes(c)).sort().join('.') || '(plain)';
    const v = [Math.round(el.getBoundingClientRect().height), s.borderRadius, s.fontSize, s.fontWeight, s.padding].join(' | ');
    (buttons[kind] ??= {})[v] = (buttons[kind][v] ?? 0) + 1;
    const h = Math.round(el.getBoundingClientRect().height);
    if (!['win-btn','close.win-btn','icon-btn','provider-item','active.provider-item'].includes(kind) && h !== 34 && h !== 26)
      (window.__odd ??= []).push(kind + ' ' + h + ' "' + el.textContent.trim().slice(0, 30) + '"');
  }
  const radii = {}, sizes = {};
  for (const el of document.querySelectorAll('body *')) {
    if (!vis(el)) continue;
    const s = getComputedStyle(el);
    if (s.borderRadius !== '0px') radii[s.borderRadius] = (radii[s.borderRadius] ?? 0) + 1;
    if (el.childNodes.length && [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim()))
      sizes[s.fontSize] = (sizes[s.fontSize] ?? 0) + 1;
  }
  return { buttons, radii, sizes, odd: window.__odd ?? [] };
})()`;

fs.mkdirSync(outDir, { recursive: true });
await cdp("Page.enable");
if (args.includes("--audit")) {
  const merged = { buttons: {}, radii: {}, sizes: {} };
  const screens = [["normal", "main"], ["normal", "settings"], ["normal", "new"], ["normal", "dialog"], ["empty", "main"], ["stale", "main"],
    ["normal", "proxy"], ["vpn", "proxy"], ["vpnerror", "proxy"], ["vpn", "main"], ["portable", "settings"], ["normal", "projectopen"]];
  await cdp("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
  for (const [scenario, screen] of screens) {
    await cdp("Page.navigate", { url: `${base}/?s=${scenario}&lang=ru` });
    for (let i = 0; i < 40 && (await evaluate("document.body?.dataset?.ready")) !== "1"; i++) await sleep(100);
    await sleep(300);
    if (SCREENS[screen]) { await evaluate(SCREENS[screen]); await sleep(300); }
    const a = await evaluate(AUDIT);
    for (const o of a.odd) console.log(`  odd height [${scenario}/${screen}]: ${o}`);
    for (const [kind, variants] of Object.entries(a.buttons)) for (const [v, n] of Object.entries(variants)) ((merged.buttons[kind] ??= {})[v] = (merged.buttons[kind][v] ?? 0) + n);
    for (const key of ["radii", "sizes"]) for (const [v, n] of Object.entries(a[key])) merged[key][v] = (merged[key][v] ?? 0) + n;
  }
  console.log("BUTTONS (height | radius | font | weight | padding): count");
  for (const [kind, variants] of Object.entries(merged.buttons)) {
    console.log(`  ${kind}`);
    for (const [v, n] of Object.entries(variants)) console.log(`      ${v}: ${n}`);
  }
  console.log("RADII:", JSON.stringify(merged.radii));
  console.log("FONT SIZES:", JSON.stringify(merged.sizes));
  chrome.kill(); server.close(); await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
  process.exit(0);
}
// --detect: run the Impeccable detector (the installed plugin) over the live pages in both themes.
// Exit code 1 when it finds anything.
if (args.includes("--detect")) {
  chrome.kill();
  const pluginRoot = path.join(os.homedir(), ".claude", "plugins", "cache", "impeccable", "impeccable");
  const versions = fs.existsSync(pluginRoot) ? fs.readdirSync(pluginRoot).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })) : [];
  const bin = versions.length && path.join(pluginRoot, versions.at(-1), "skills", "impeccable", "scripts", "impeccable.cmd");
  if (!bin || !fs.existsSync(bin)) throw new Error("the Impeccable plugin is not installed: claude plugin enable impeccable@impeccable --scope local");
  const pages = ["normal", "normal&view=settings", "empty", "stale", "warn", "error", "long&view=settings", "forceddark", "forceddark&view=settings", "forcedlight", "normal&lang=en&view=settings",
    "normal&view=proxy", "vpn&view=proxy", "vpnerror&view=proxy", "vpnlong&view=proxy", "vpn&lang=en&view=proxy", "forceddark&view=proxy", "portable&view=settings", "normal&view=project", "kitnonode&view=project", "kitlong&view=project"];
  let failed = 0;
  for (const page of pages) {
    const url = `${base}/?s=${page}`;
    // cmd.exe needs the whole command line in quotes, or it cuts the URL at "&" and runs the rest
    // (`view=settings`) as a command of its own.
    const out = await new Promise((resolve) => {
      const p = spawn("cmd.exe", ["/d", "/s", "/c", `""${bin}" detect --no-advisory "${url}""`], { windowsVerbatimArguments: true });
      let text = "";
      p.stdout.on("data", (d) => (text += d));
      p.stderr.on("data", (d) => (text += d));
      p.on("close", (code) => resolve({ text, code }));
    });
    const found = /(\d+) anti-patterns? found/.exec(out.text);
    const count = found ? Number(found[1]) : 0;
    // The detector exits 0 when clean and 2 with findings; anything else is a broken run, never "clean".
    const clean = out.code === 0 && !count;
    if (count) failed += count;
    else if (!clean) failed += 1;
    console.log(page.padEnd(34), count ? `${count} finding(s)\n${out.text.trim()}` : clean ? "clean" : `DETECTOR FAILED (exit ${out.code})\n${out.text.trim()}`);
  }
  server.close();
  fs.rmSync(profile, { recursive: true, force: true });
  process.exit(failed ? 1 : 0);
}
// --serve: keep the stub server up so that `impeccable detect http://127.0.0.1:<port>/?s=normal` can scan it.
if (args.includes("--serve")) {
  console.log(`serving ${base}/?s=normal  (also ?s=empty, ?view=settings); Ctrl+C to stop`);
  chrome.kill();
  await new Promise(() => {});
}
const report = [];
let finding = false;
for (const shot of shots) {
  const name = `${shot.scenario}-${shot.screen}-${shot.size[0]}-${shot.scheme}-${shot.lang}${shot.scale ? "-x" + shot.scale : ""}`;
  if (only && !name.includes(only)) continue;
  await cdp("Emulation.setDeviceMetricsOverride", { width: shot.size[0], height: shot.size[1], deviceScaleFactor: shot.scale ?? 1, mobile: false });
  await cdp("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: shot.scheme }] });
  await cdp("Page.navigate", { url: `${base}/?s=${shot.scenario}&lang=${shot.lang}` });
  if (shot.settleMs) await sleep(shot.settleMs);
  else for (let i = 0; i < 40 && (await evaluate("document.body?.dataset?.ready")) !== "1"; i++) await sleep(100);
  await sleep(250);
  const step = SCREENS[shot.screen];
  let hoverNote = "";
  if (step?.startsWith("hover:")) {
    // The pointer over a control, for real (a :hover state cannot be faked from the page).
    const selector = step.slice(6);
    const at = await evaluate(`(() => { const b = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`);
    await cdp("Input.dispatchMouseEvent", { type: "mouseMoved", x: at[0], y: at[1] });
    await sleep(300);
    const ratio = await evaluate(`(() => {
      const el = document.querySelector(${JSON.stringify(selector)}), s = getComputedStyle(el);
      // Any CSS colour (rgb, color-mix, color(srgb ...)) as plain sRGB through a canvas pixel.
      const px = document.createElement('canvas'); px.width = px.height = 1;
      const g = px.getContext('2d', { willReadFrequently: true });
      const rgb = (c) => { g.clearRect(0, 0, 1, 1); g.fillStyle = '#000'; g.fillStyle = c; g.fillRect(0, 0, 1, 1); const d = g.getImageData(0, 0, 1, 1).data; return [d[0], d[1], d[2], d[3] / 255]; };
      const over = (top, under) => { const a = top[3] ?? 1; return [0, 1, 2].map((i) => top[i] * a + under[i] * (1 - a)); };
      let bg = [255, 255, 255];
      const chain = []; for (let n = el; n; n = n.parentElement) chain.unshift(getComputedStyle(n).backgroundColor);
      for (const c of chain) { const v = rgb(c); if ((v[3] ?? 1) > 0) bg = over(v, bg); }
      const lum = (v) => { const [r, g, b] = v.map((x) => { x /= 255; return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
      const fg = over(rgb(s.color), bg), l1 = lum(fg), l2 = lum(bg);
      return ((Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05)).toFixed(2);
    })()`);
    // 4.5:1 is the AA bar for the 12-13 px text of these controls.
    hoverNote = Number(ratio) < 4.5 ? `HOVER-CONTRAST ${ratio}:1` : `hover contrast ${ratio}:1`;
  } else if (step) {
    await evaluate(step);
    await sleep(shot.screen === "launch" || shot.screen === "testconn" ? 500 : 250);
  }
  const m = await evaluate(MEASURE);
  const png = await cdp("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(outDir, name + ".png"), Buffer.from(png.data, "base64"));
  report.push({ name, ...m });
  const flags = [hoverNote, m.cardClipped && "CARD-CLIPPED", m.hscroll && "HSCROLL", m.misaligned?.length && "MISALIGNED " + m.misaligned.join(", "), m.clipped.length && "CLIPPED " + m.clipped.join(", "), m.scrollers.length && "SCROLL " + m.scrollers.join(", ")].filter(Boolean);
  console.log(name.padEnd(42), flags.join(" | ") || "ok");
  // SCROLL — neutral: a panel that legitimately scrolls. It is still printed.
  if (hoverNote.startsWith("HOVER-CONTRAST") || m.cardClipped || m.hscroll || m.misaligned?.length || m.clipped.length) finding = true;
}
fs.writeFileSync(path.join(outDir, "report.json"), JSON.stringify(report, null, 1));

chrome.kill();
server.close();
await sleep(500);
fs.rmSync(profile, { recursive: true, force: true });
console.log(`\n${report.length} shots in ${outDir}${finding ? " - FINDINGS" : ""}`);
process.exit(finding ? 1 : 0);
