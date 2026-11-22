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
  const lang = q.get("lang") || "ru";
  const long = scenario === "long";
  const alias = (p, m, i) => ({ model: m, alias: "claude-" + p + "-" + (1000 + i), supports1m: m.includes("[1m]"), label: m.replace("[1m]", "") });
  const mk = (id, name, base, models, key = true) => ({
    id, name, kind: "openai", baseUrl: base, models, hasKey: key, needsKey: true,
    aliases: models.map((m, i) => alias(id, m, i)), maxEffort: "max", defaultEffort: null, labels: {},
  });
  let providers = scenario === "empty" ? [] : long ? [
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
    russianUi: false, builtinBrowser: false, russianFile: true, configDir: "C:\\Users\\user\\AppData\\Roaming\\app.provider-gateway-for-claude",
    codeDir: null, language: lang, languagePref: "auto", autoUpdate: true, version: "0.2.2",
    modelRoles: providers.length ? { sonnet: { provider: providers[0].id, model: providers[0].models[0] }, opus: null, haiku: null } : { sonnet: null, opus: null, haiku: null },
    advisorModel: "opus", advisorDecided: true, claudeSettings: "C:\\Users\\user\\.claude\\settings.json", claudeSettingsError: null, advisorNotice: null,
  });
  let running = scenario !== "empty";
  const status = () => ({ running, port: 17899, token: cfg().token, models: running ? models() : [], logPath: "C:\\logs\\gateway.log",
    claude: { version: "app-6.28899.6", items }, update: null });
  const delay = scenario === "slow" ? 1500 : 0;
  const fail = (msg) => Promise.reject(msg);
  const handlers = {
    get_config: () => cfg(), status: () => status(),
    claude_state: () => ({ running: scenario === "open" || scenario === "stale", stale: scenario === "stale" }),
    save_provider: () => scenario === "error" ? fail("HTTP 500: Internal Server Error (stubbed)") : cfg(),
    delete_provider: () => cfg(), fetch_models: () => ({ ids: ["m1", "m2"], catalog: {} }),
    test_provider: () => scenario === "error" ? fail("HTTP 500: Internal Server Error (stubbed)") : "OK",
    save_settings: () => cfg(), start_gateway: () => { running = true; return status(); }, stop_gateway: () => { running = false; return status(); },
    launch_claude: () => scenario === "error" ? fail("Could not start claude.exe: access denied (stubbed)") : ({ message: "Claude started (PID 4242)", warning: false }),
    launch_claude_code: () => "Claude Code opened", tail_log: () => "2026-11-22T10:00:00Z INFO gateway started port=17899\n".repeat(30),
    open_logs: () => null, copy_text: () => null, pick_folder: () => null, app_exit: () => null, check_update: () => ({ available: false }),
    install_update: () => null, win_minimize: () => null, win_toggle_maximize: () => null, win_close: () => null, code_command: () => "$env:X=1\nclaude",
  };
  const invoke = async (cmd, a) => {
    if (delay) await new Promise((r) => setTimeout(r, delay));
    const h = handlers[cmd];
    if (!h) return fail("unknown command " + cmd);
    return h(a);
  };
  window.__TAURI__ = { core: { invoke } };
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
  testconn: "[...document.querySelectorAll('button')].find((b) => /Проверить связь|Test connection/.test(b.textContent))?.click()",
};
const W = [[1920, 1080], [1280, 800], [900, 700]];
const shots = [];
const add = (scenario, screen, widths, schemes, lang = "ru", extra = {}) => {
  for (const wi of widths) for (const scheme of schemes) shots.push({ scenario, screen, size: W[wi], scheme, lang, ...extra });
};
add("normal", "main", [0, 1, 2], ["light", "dark"]);
add("empty", "main", [0, 1], ["light", "dark"]);
add("normal", "settings", [0, 1, 2], ["light", "dark"]);
add("normal", "settings", [1], ["light", "dark"], "en");
add("normal", "new", [1], ["dark"]);
add("normal", "dialog", [1], ["dark"]);
add("long", "main", [1], ["light", "dark"]);
add("long", "settings", [1], ["dark"]);
add("error", "launch", [1], ["dark"]);
add("error", "testconn", [1], ["light"]);
add("slow", "main", [1], ["light"], "ru", { settleMs: 400 });
add("warn", "main", [1], ["light"]);
add("open", "main", [1], ["dark"]);
add("stale", "main", [0, 1, 2], ["light", "dark"]);

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
const evaluate = async (expression) => (await cdp("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.value;

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
  return { hscroll: doc.scrollWidth > innerWidth + 1, docH: doc.scrollHeight, winH: innerHeight, clipped: clipped.slice(0, 8), scrollers,
           settingsVisible: !!panel && !panel.classList.contains('hidden') };
})()`;

fs.mkdirSync(outDir, { recursive: true });
await cdp("Page.enable");
const report = [];
for (const shot of shots) {
  const name = `${shot.scenario}-${shot.screen}-${shot.size[0]}-${shot.scheme}-${shot.lang}`;
  if (only && !name.includes(only)) continue;
  await cdp("Emulation.setDeviceMetricsOverride", { width: shot.size[0], height: shot.size[1], deviceScaleFactor: 1, mobile: false });
  await cdp("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: shot.scheme }] });
  await cdp("Page.navigate", { url: `${base}/?s=${shot.scenario}&lang=${shot.lang}` });
  if (shot.settleMs) await sleep(shot.settleMs);
  else for (let i = 0; i < 40 && (await evaluate("document.body?.dataset?.ready")) !== "1"; i++) await sleep(100);
  await sleep(250);
  const step = SCREENS[shot.screen];
  if (step) {
    await evaluate(step);
    await sleep(shot.screen === "launch" || shot.screen === "testconn" ? 500 : 250);
  }
  const m = await evaluate(MEASURE);
  const png = await cdp("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(outDir, name + ".png"), Buffer.from(png.data, "base64"));
  report.push({ name, ...m });
  const flags = [m.hscroll && "HSCROLL", m.clipped.length && "CLIPPED " + m.clipped.join(", "), m.scrollers.length && "SCROLL " + m.scrollers.join(", ")].filter(Boolean);
  console.log(name.padEnd(42), flags.join(" | ") || "ok");
}
fs.writeFileSync(path.join(outDir, "report.json"), JSON.stringify(report, null, 1));

chrome.kill();
server.close();
await sleep(500);
fs.rmSync(profile, { recursive: true, force: true });
console.log(`\n${report.length} shots in ${outDir}`);
process.exit(0);
