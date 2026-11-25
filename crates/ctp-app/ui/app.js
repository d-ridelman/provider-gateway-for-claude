"use strict";

const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI__?.invoke;

const state = {
  config: null,
  status: null,
  selectedId: null,
  isNew: false,
  originalId: null,
  clearKey: false,
  view: "provider",
  logsOpen: false,
};

// A new provider starts empty: examples live in placeholders, not in the values.
const draft = {
  id: "",
  name: "",
  kind: "openai",
  baseUrl: "",
  models: [],
  hasKey: false,
  needsKey: true,
  aliases: [],
};

// Templates: id, name, type and URL; the label is shown in the interface language.
const PRESETS = [
  { label: { ru: "OpenAI", en: "OpenAI" }, id: "openai", name: "OpenAI", kind: "openai", baseUrl: "https://api.openai.com/v1" },
  { label: { ru: "OpenRouter", en: "OpenRouter" }, id: "openrouter", name: "OpenRouter", kind: "openai", baseUrl: "https://openrouter.ai/api/v1" },
  { label: { ru: "OpenCode Go (подписка)", en: "OpenCode Go (subscription)" }, id: "opencode-go", name: "OpenCode Go", kind: "openai", baseUrl: "https://opencode.ai/inference/go/openai/v1" },
  { label: { ru: "OpenCode Go — Claude, Qwen, MiniMax (Anthropic API)", en: "OpenCode Go — Claude, Qwen, MiniMax (Anthropic API)" }, id: "opencode-go-anthropic", name: "OpenCode Go (Anthropic)", kind: "anthropic", baseUrl: "https://opencode.ai/inference/go/anthropic" },
  { label: { ru: "OpenCode Zen (оплата по факту)", en: "OpenCode Zen (pay as you go)" }, id: "opencode", name: "OpenCode Zen", kind: "openai", baseUrl: "https://opencode.ai/inference/openai/v1" },
  { label: { ru: "DeepSeek", en: "DeepSeek" }, id: "deepseek", name: "DeepSeek", kind: "openai", baseUrl: "https://api.deepseek.com/v1" },
  { label: { ru: "Google Gemini", en: "Google Gemini" }, id: "gemini", name: "Gemini", kind: "openai", baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai" },
  { label: { ru: "Anthropic API", en: "Anthropic API" }, id: "anthropic", name: "Anthropic", kind: "anthropic", baseUrl: "https://api.anthropic.com" },
  { label: { ru: "Ollama (этот компьютер)", en: "Ollama (this computer)" }, id: "ollama", name: "Ollama", kind: "openai", baseUrl: "http://127.0.0.1:11434/v1" },
  { label: { ru: "llama.cpp server (этот компьютер)", en: "llama.cpp server (this computer)" }, id: "llamacpp", name: "llama.cpp", kind: "openai", baseUrl: "http://127.0.0.1:8080/v1" },
  { label: { ru: "LM Studio (этот компьютер)", en: "LM Studio (this computer)" }, id: "lmstudio", name: "LM Studio", kind: "openai", baseUrl: "http://127.0.0.1:1234/v1" },
];

const $ = (id) => document.getElementById(id);
const el = Object.fromEntries(
  [
    "statusDot", "statusText", "portLabel", "providerList", "empty", "providerForm", "formTitle",
    "formSubtitle", "formKindBadge", "formKeyBadge", "idInput", "nameInput", "kindInput", "baseInput",
    "keyInput", "keyHint", "clearKeyBtn", "modelsInput", "maxEffortInput", "defaultEffortInput",
    "aliasBox", "aliasList", "deleteBtn", "formNotice", "settingsPanel", "portInput", "languageInput",
    "claudePathInput", "codeDirInput", "launchCodeBtn", "detectedClaude", "tokenInput", "russianInput",
    "builtinBrowserInput", "claudeState", "claudeStateText", "langBtn", "themeBtn", "themeInput",
    "russianFileHint", "claudeWarn", "claudeCheckList", "startBtn", "stopBtn", "launchBtn", "modelCount",
    "logPanel", "logText", "logsBtn", "toast", "toastText", "presetField", "presetInput", "fetchModelsBtn",
    "testBtn", "pickerBox", "pickerFilter", "pickerCount", "pickerList", "dialog", "dialogTitle",
    "dialogText", "dialogOk", "dialogCancel", "dialogAlt", "dialogCheckRow", "dialogCheck", "dialogCheckText",
    "updateBtn", "versionText", "autoUpdateInput", "checkUpdateBtn", "roleSonnet", "roleCritic",
    "desktopPortableBtn", "desktopInstallBtn", "desktopGetHint", "codeFound",
    "codePortableInput", "codePortableBtn", "codeInstallBtn", "codeGetHint",
    "proxyBtn", "proxyChip", "proxyChipText", "proxyPanel", "proxyError", "proxyCoreLine", "proxyCoreBtn",
    "proxyCoreHint", "subList", "subForm", "subNameInput", "subUrlInput", "subIntervalInput", "subAddBtn",
    "measureBtn", "nodeGrid", "nodesEmpty",
  ].map((id) => [id, $(id)]),
);
el.form = el.providerForm;

// Models loaded from the provider; live while the same provider is open.
let picker = [];
// OpenCode catalog hints for the last model list: { source, models: { id: hint } }.
let catalog = { source: null, models: {} };
// Model names for Claude's menu (ID → "DeepSeek V4 Pro") in the open form.
let labels = {};
// Grows when another provider opens: a late "Load models" answer won't reach a foreign form.
let formEpoch = 0;
// The form as rendered: compared with the current one to spot unsaved changes.
let formSnapshot = "";

async function boot() {
  if (!invoke) {
    applyI18n();
    toast(t("toast.noTauri"), "error");
    return;
  }
  bind();
  addHelpButtons();
  await refresh();
  autostart();
  pollStatus();
  setInterval(pollStatus, 3000);
  setInterval(() => {
    if (state.logsOpen) refreshLogs();
  }, 2500);
}

function fillPresets() {
  el.presetInput.replaceChildren(el.presetInput.options[0]);
  for (const [index, preset] of PRESETS.entries()) {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = preset.label[LANG] ?? preset.label.en;
    el.presetInput.append(option);
  }
}

// Language from the app: re-label everything that is static.
function applyLanguage(lang) {
  setLanguage(lang);
  applyI18n();
  addHelpButtons();
  fillPresets();
  renderUiSwitches();
}

// Theme: "auto" follows Windows, "light" / "dark" are the user's own choice. The page is
// painted by theme.js before the first frame; this keeps it in step with the saved setting.
const lightQuery = matchMedia("(prefers-color-scheme: light)");
function effectiveTheme(pref) {
  return pref === "light" || (pref !== "dark" && lightQuery.matches) ? "light" : "dark";
}
function applyTheme(pref) {
  state.themePref = pref || "auto";
  document.documentElement.dataset.theme = effectiveTheme(state.themePref);
  try {
    localStorage.setItem("pgc-theme", state.themePref);
  } catch {
    /* the next start follows the system */
  }
  renderUiSwitches();
}
lightQuery.addEventListener("change", () => {
  if ((state.themePref ?? "auto") === "auto") applyTheme("auto");
});

// The two quick switches in the title bar: language shows the current one, the theme button
// shows what a click gives (sun in the dark theme, moon in the light one).
function renderUiSwitches() {
  const dark = document.documentElement.dataset.theme !== "light";
  el.langBtn.textContent = LANG.toUpperCase();
  el.langBtn.title = el.langBtn.ariaLabel = t(LANG === "ru" ? "ui.langToEn" : "ui.langToRu");
  el.themeBtn.title = el.themeBtn.ariaLabel = t(dark ? "ui.themeToLight" : "ui.themeToDark");
}

async function setUi(patch) {
  // A language switch re-labels and re-renders the whole window: unsaved edits would be lost,
  // so it asks first, like leaving the form does. The theme switch changes colours only.
  if (patch.language && !(await leaveForm())) return;
  try {
    const config = await invoke("set_ui", patch);
    if (patch.language) {
      state.config = config;
      applyLanguage(config.language);
      applyTheme(config.themePref);
      render();
    } else {
      state.config.themePref = config.themePref;
      el.themeInput.value = config.themePref;
      applyTheme(config.themePref);
      syncSettingsSave();
    }
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

async function refresh() {
  try {
    const [config, status] = await Promise.all([invoke("get_config"), invoke("status")]);
    state.config = config;
    state.status = status;
    if (LANG !== config.language || !document.body.dataset.ready) {
      applyLanguage(config.language);
      document.body.dataset.ready = "1";
    }
    applyTheme(config.themePref);
    if (state.selectedId && !state.isNew) {
      if (!config.providers.some((p) => p.id === state.selectedId)) {
        state.selectedId = config.providers[0]?.id ?? null;
        state.originalId = state.selectedId;
      }
    } else if (!state.selectedId && config.providers.length) {
      state.selectedId = config.providers[0].id;
      state.originalId = state.selectedId;
    }
    render();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// The window is opened to use the gateway: start it right away if there is anything to serve.
async function autostart() {
  if (state.config?.providers?.length && !state.status?.running) {
    await startGateway({ quiet: true });
  }
}

// Strip in the footer: is Claude open in the gateway's profile, and does it still run with the
// current settings. Stays on screen (unlike the toast after "Open Claude").
function renderClaudeState() {
  const run = state.claudeRun;
  // A failed launch stays in the strip (the toast is gone in seconds) until Claude opens.
  const failed = !run?.running && !!state.launchError;
  el.claudeState.classList.toggle("hidden", !run?.running && !failed);
  if (!run?.running && !failed) return;
  el.claudeState.classList.toggle("error", failed);
  el.claudeState.classList.toggle("stale", !failed && !!run.stale);
  if (failed) {
    el.claudeStateText.textContent = t("claudeState.failed");
    el.claudeState.title = state.launchError;
    return;
  }
  el.claudeStateText.textContent = t(run.stale ? "claudeState.stale" : "claudeState.open");
  el.claudeState.title = t(run.stale ? "claudeState.staleTitle" : "claudeState.openTitle");
}

async function pollStatus() {
  try {
    state.status = await invoke("status");
    renderStatus();
    state.claudeRun = await invoke("claude_state").catch(() => null);
    renderClaudeState();
    // The proxy is asked only while it matters: its page is open or a mode is on.
    // A hidden window (in the tray) asks nothing: each answer reads Credential Manager and the core.
    if (!document.hidden && (!state.proxy || state.view === "proxy" || state.proxy.mode !== "off")) await refreshProxy();
  } catch {
    /* the app is closing */
  }
}

function render() {
  renderStatus();
  renderProviders();
  renderMain();
}

function renderStatus() {
  const status = state.status;
  const running = !!status?.running;
  el.statusDot.classList.toggle("on", running);
  el.statusText.textContent = t(running ? "status.running" : "status.stopped");
  el.statusText.classList.toggle("bad", !running);
  el.portLabel.textContent = `127.0.0.1:${status?.port ?? state.config?.port ?? "—"}`;
  el.startBtn.textContent = t(running ? "actions.restart" : "actions.start");
  el.stopBtn.disabled = !running;
  const ready = running && !!status?.models?.length;
  el.launchBtn.disabled = launching || !ready;
  if (!launchingCode) el.launchCodeBtn.disabled = !ready;
  el.modelCount.textContent = t("models.total", { count: status?.models?.length ?? 0 });
  const update = status?.update;
  el.updateBtn.classList.toggle("hidden", !update);
  if (update && !installingUpdate) el.updateBtn.textContent = t("update.button", { version: update.version });
  renderClaudeCheck();
}

function renderProviders() {
  el.providerList.replaceChildren();
  const providers = state.config?.providers ?? [];
  // No providers: the main area already says so and offers "Add provider".
  if (!providers.length) return;
  for (const provider of providers) {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "provider-item";
    const active = state.view === "provider" && !state.isNew && provider.id === state.selectedId;
    item.classList.toggle("active", active);
    if (active) item.setAttribute("aria-current", "true");
    item.addEventListener("click", () => selectProvider(provider.id));

    const row = document.createElement("span");
    row.className = "row";
    const name = document.createElement("b");
    name.textContent = provider.name || provider.id;
    const kind = document.createElement("span");
    kind.className = "badge";
    kind.textContent = provider.kind;
    row.append(name, kind);

    const url = document.createElement("span");
    url.className = "url";
    url.textContent = provider.baseUrl;

    const meta = document.createElement("span");
    meta.className = "meta";
    const key = t(provider.hasKey ? "provider.keySet" : provider.needsKey ? "provider.keyMissing" : "provider.keyless");
    meta.textContent = `${modelWord(provider.models.length)} · ${key}`;

    item.append(row, url, meta);
    el.providerList.append(item);
  }
}

function renderMain() {
  const settings = state.view === "settings";
  const proxy = state.view === "proxy";
  const project = state.view === "project";
  const page = settings || proxy || project;
  for (const [button, on] of [[$("settingsBtn"), settings], [el.proxyBtn, proxy], [$("projectBtn"), project]]) {
    if (on) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  }
  el.settingsPanel.classList.toggle("hidden", !settings);
  el.proxyPanel.classList.toggle("hidden", !proxy);
  $("projectPanel").classList.toggle("hidden", !project);
  el.form.classList.toggle("hidden", page || !hasSelection());
  el.empty.classList.toggle("hidden", page || hasSelection());
  if (settings) {
    renderSettings();
    return;
  }
  if (proxy) {
    renderProxy();
    return;
  }
  if (project) {
    renderProject();
    return;
  }
  if (hasSelection()) renderForm();
}

// ---- connect a project (agent-kit, ADR-014) -------------------------------------------
// Check (nothing changes) → Connect (the plugin, local to this machine unless "for the team")
// → the inspection and the plan with ticks → Apply the ticked ones. Every step is the
// backend's answer drawn as it is; agent-kit's own lines come in its language.

async function openProject() {
  if (state.view === "project") return;
  if (!(await leaveForm())) return;
  state.view = "project";
  render();
}

function closeProject() {
  state.view = "provider";
  render();
}

function checkItem(kind, text, title) {
  const li = document.createElement("li");
  li.className = kind;
  if (kind === "busy") {
    // Work under way: the brand's work mark instead of a sign.
    li.append(workMark(), text);
    if (title) li.title = title;
    return li;
  }
  li.textContent = `${{ ok: "✓", off: "–", bad: "✕", human: "!" }[kind] ?? "·"} ${text}`;
  if (title) li.title = title;
  return li;
}

function renderProject() {
  const p = state.project ?? {};
  const list = $("projectCheckList");
  list.replaceChildren();
  if (p.busy === "check") list.append(checkItem("busy", t("project.checking")));
  if (p.error) list.append(checkItem("bad", p.error));
  const check = p.check;
  if (check && !p.busy) {
    // What stops the connection is the person's to fix, not a failure: amber, not red.
    for (const item of check.blockers) list.append(checkItem("human", item.text));
    for (const item of check.notes) list.append(checkItem("off", item.text));
    // "Ready to connect" is for before connecting: once the inspection is shown it is stale.
    if (!check.blockers.length && !p.data) list.append(checkItem("ok", t(check.connected ? "project.alreadyOn" : "project.ready", { root: check.root })));
  }
  // The only blocker the page itself can lift: where agent-kit comes from.
  $("projectSourceBox").classList.toggle("hidden", !check?.blockers.some((b) => b.id === "source"));
  const canConnect = !!check && !check.blockers.length && !p.data;
  $("projectConnectBox").classList.toggle("hidden", !canConnect);
  $("projectConnectBtn").textContent = t(check?.connected ? "project.open" : "project.connect");
  // While a step runs the folder cannot change under it: the answer is for the folder asked.
  for (const id of ["projectConnectBtn", "projectCheckBtn", "projectApplyBtn", "projectDirInput", "browseProjectBtn", "projectSourceBtn", "projectTeamInput"]) {
    $(id).disabled = !!p.busy;
  }
  if (p.busy === "connect") list.append(checkItem("busy", t("project.connecting")));
  if (p.busy === "update") list.append(checkItem("busy", t("project.updating")));

  const data = p.data;
  $("projectStateGroup").classList.toggle("hidden", !data);
  $("projectPlanGroup").classList.toggle("hidden", !data);
  $("projectChecksGroup").classList.toggle("hidden", !data?.checks);
  if (!data) return;
  const findings = data.inspect?.findings ?? [];
  const count = (status) => findings.filter((f) => f.status === status).length;
  $("projectStateLine").textContent = t("project.stateLine", { root: data.root, ok: count("ok"), missing: count("missing"), problem: count("problem") });
  $("projectFresh").classList.toggle("hidden", !p.fresh);
  // A newer agent-kit inside PGC: offered, never applied by itself.
  const update = p.check?.update;
  $("projectUpdateBtn").classList.toggle("hidden", !update || !!p.updated);
  if (update) $("projectUpdateBtn").textContent = t("project.update", { from: update.from, to: update.to });
  $("projectUpdateBtn").disabled = !!p.busy;
  const shown = $("projectFindings");
  shown.replaceChildren();
  // Only what is not fine, and advice (a slow step that never failed): the rest is in the count above.
  for (const f of findings.filter((f) => f.status !== "ok" || f.advice)) {
    const li = checkItem(f.status === "problem" ? "human" : "off", f.what, f.fix);
    // The fix was only a hover tooltip: under "?" a keyboard user reads it too.
    if (f.about || f.fix) li.append(helpButton(() => openHelp(f.what, [f.about, f.fix].filter(Boolean).join("\n\n")), f.what));
    shown.append(li);
  }
  const plan = $("projectPlan");
  plan.replaceChildren();
  const note = $("projectPlanNote");
  if (data.planError) {
    note.textContent = t("project.planOld");
    note.title = data.planError;
  } else {
    note.textContent = data.plan.length ? t("project.planHint") : t("project.planEmpty");
    note.title = "";
  }
  // Blocks by what a person gets (agent-kit's group), advice for the person last; an old
  // agent-kit gives no groups and its items stay in one block, as before.
  const order = ["agent", "checks", "safety", "docs", "", "advice"];
  const groupOf = (item) => (item.action === "keep" ? "advice" : order.includes(item.group) ? item.group : "");
  const items = [...(data.plan ?? [])].sort((a, b) => order.indexOf(groupOf(a)) - order.indexOf(groupOf(b)));
  // Items with one plain title in one block (four step templates, three rule files) are one
  // row with one tick for all of them: a person decides on "templates for big tasks", not on
  // each file. Items without a title (an old agent-kit) stay one row each.
  const rows = [];
  for (const item of items) {
    const last = rows.at(-1);
    const same = last && item.title && last[0].title === item.title && groupOf(last[0]) === groupOf(item)
      && last[0].action === item.action && (last[0].default === false) === (item.default === false);
    if (same) last.push(item);
    else rows.push([item]);
  }
  let shownGroup = null;
  for (const row of rows) {
    const item = row[0];
    const group = groupOf(item);
    if (group !== shownGroup && (group || shownGroup)) {
      const head = document.createElement("li");
      head.className = "plan-group";
      head.textContent = t(`project.planGroup.${group || "other"}`);
      plan.append(head);
    }
    shownGroup = group;
    const li = document.createElement("li");
    // Full text (with long permission lists) goes under "?": the row stays one plain line.
    const help = [item.about, item.title ? row.map((i) => i.what).join("\n") : ""].filter(Boolean).join("\n\n");
    const helpTitle = item.title || item.path;
    const verb = t(item.action === "add" ? "project.add" : "project.create");
    if (item.action === "keep") {
      li.className = "off";
      li.textContent = item.title ? `– ${item.title}` : `– ${item.path}: ${item.what}`;
      if (help) li.append(helpButton(() => openHelp(helpTitle, help), helpTitle));
    } else {
      const label = document.createElement("label");
      label.className = "check";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.value = item.id;
      box.checked = row.every((i) => planTicked(p, i));
      box.addEventListener("change", () => {
        // What the person changed against each item's default, so a re-render keeps it.
        for (const i of row) {
          const key = i.default === false ? "planOn" : "unticked";
          const set = new Set(state.project[key] ?? []);
          if (box.checked === (i.default !== false)) set.delete(i.id);
          else set.add(i.id);
          state.project[key] = [...set];
        }
      });
      const span = document.createElement("span");
      if (item.title) {
        // A plain name with its "?" right after it, and the files in small type under it.
        // In a label a click on "?" would tick the box: helpButton cancels that.
        const where = document.createElement("small");
        where.className = "plan-path";
        where.textContent = `${verb} ${row.map((i) => i.path).join(", ")}`;
        span.append(item.title);
        if (help) span.append(helpButton(() => openHelp(helpTitle, help), helpTitle));
        span.append(where);
      } else {
        // agent-kit's own text may already name the file ("Create docs/x.md"): no second copy of it.
        span.textContent = item.what.includes(item.path) ? item.what : `${verb} ${item.path} — ${item.what}`;
        if (help) span.append(helpButton(() => openHelp(helpTitle, help), helpTitle));
      }
      label.append(box, span);
      li.append(label);
      // A draft (critic.md rules) is shown in full: the person reads what would be written.
      if (item.rules?.length) {
        // Each rule with where it was taken from, so a person can check it against the source.
        const list = document.createElement("ul");
        list.className = "draft-rules";
        item.rules.forEach((rule, n) => {
          const row = document.createElement("li");
          row.append(Object.assign(document.createElement("span"), { className: "mono", textContent: rule }));
          const src = item.from?.[n];
          if (src?.file) {
            row.append(Object.assign(document.createElement("small"), {
              textContent: t(src.line ? "project.ruleFrom" : "project.ruleFromFile", { file: src.file, line: src.line }),
            }));
          }
          list.append(row);
        });
        li.append(list);
      }
    }
    plan.append(li);
  }
  renderChecks(p, data);
  const applying = p.busy === "apply";
  $("projectApplyBtn").replaceChildren(...(applying ? [workMark(), t("project.applying")] : [t("project.apply")]));
  $("projectApplyBtn").toggleAttribute("aria-busy", applying);
  $("projectApplyBtn").classList.toggle("hidden", !(data.plan ?? []).some((item) => item.action !== "keep") && !checksOf(data).length);
}

// A plan item is ticked by default unless agent-kit says otherwise (default: false — a draft).
function planTicked(p, item) {
  return item.default === false ? !!p.planOn?.includes(item.id) : !p.unticked?.includes(item.id);
}

// Checks the project writes down (ADR-016): the code's pick ticked, the ones its name rule set aside unticked.
function checksOf(data) {
  const c = data.checks;
  // One already run by the project's own verify.json is not offered again: unticked, with a note.
  return c ? [...c.checks.map((x) => ({ ...x, on: !x.present })), ...c.struck.map((x) => ({ ...x, on: false }))] : [];
}

function checkTicked(p, check) {
  return (check.on && !p.checksOff?.includes(check.id)) || (!check.on && !!p.checksOn?.includes(check.id));
}

function renderChecks(p, data) {
  const all = checksOf(data);
  const list = $("projectChecks");
  list.replaceChildren();
  if (!data.checks) return;
  const own = !(data.plan ?? []).some((item) => item.id === ".claude/verify.json");
  $("projectChecksNote").textContent = !all.length ? t("project.checksEmpty") : t(own ? "project.checksOwn" : "project.checksHint");
  // After applying with an own verify.json: the steps to add by hand (agent-kit never edits that file).
  const byHand = data.applied?.skipped?.find((s) => s.id === "verify-checks");
  if (byHand) {
    const li = checkItem("human", t("project.checksByHand"));
    const pre = document.createElement("pre");
    // The steps follow "by hand: " as JSON objects; a command may hold braces (${{ matrix.n }}), so
    // the rest is shown as is, never picked apart. `steps` — when agent-kit gives them as a list.
    const steps = byHand.steps ?? null;
    const rest = byHand.why.split("by hand: ")[1];
    pre.textContent = steps ? steps.map((s) => JSON.stringify(s)).join(",\n") : rest ? rest.replaceAll('"}, {"', '"},\n{"') : byHand.why;
    li.append(pre);
    list.append(li);
  }
  for (const check of all) {
    const li = document.createElement("li");
    const label = document.createElement("label");
    label.className = "check";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = checkTicked(p, check);
    box.addEventListener("change", () => {
      // What the person changed against the default, so a re-render keeps it.
      const key = check.on ? "checksOff" : "checksOn";
      const set = new Set(state.project[key] ?? []);
      if (box.checked === check.on) set.delete(check.id);
      else set.add(check.id);
      state.project[key] = [...set];
    });
    const span = document.createElement("span");
    const cmd = document.createElement("span");
    cmd.className = "mono";
    cmd.textContent = check.cmd;
    const meta = document.createElement("span");
    meta.className = "muted";
    const where = check.source ? `${check.source.file}:${check.source.line}` : "";
    const kind = [check.kind === "other" ? "" : check.kind, check.caught ? t("project.checkCaught", { count: check.caught }) : ""].filter(Boolean).join(" · ");
    const why = check.present ? t("project.checkPresent")
      : check.on ? "" : t("project.checkStruck", { why: check.ruled ? t("project.checkRuled", { word: check.ruled }) : check.why });
    meta.textContent = ` — ${[[where, kind].filter(Boolean).join(" · "), why].filter(Boolean).join(" — ")}`;
    span.append(cmd, meta);
    label.append(box, span);
    li.append(label);
    list.append(li);
  }
}

async function projectStep(busy, work) {
  state.project = { ...(state.project ?? {}), busy, error: null };
  renderProject();
  try {
    await work();
  } catch (exc) {
    state.project.error = formatError(exc);
  } finally {
    state.project.busy = null;
    renderProject();
  }
}

// A step's answer is kept only if the page still shows the folder it was asked for: a folder
// typed or picked meanwhile starts over (the input resets state.project).
function projectStillOn(dir) {
  return state.project?.dir === dir;
}

function checkProject() {
  if (state.project?.busy) return;
  const dir = $("projectDirInput").value.trim();
  if (!dir) {
    $("projectDirInput").focus();
    return;
  }
  state.project = { dir };
  projectStep("check", async () => {
    const check = await invoke("connect_check", { dir });
    if (projectStillOn(dir)) state.project.check = check;
  });
}

function resetProject() {
  if (state.project?.busy) return;
  state.project = null;
  $("projectTeamInput").checked = false;
  renderProject();
}

function connectProject() {
  const { dir, busy } = state.project ?? {};
  if (!dir || busy) return;
  projectStep("connect", async () => {
    const data = await invoke("connect_open", { dir, team: $("projectTeamInput").checked });
    if (!projectStillOn(dir)) return;
    state.project.data = data;
    state.project.fresh = data.fresh;
  });
}

function updateProject() {
  const { dir, busy } = state.project ?? {};
  if (!dir || busy) return;
  projectStep("update", async () => {
    const data = await invoke("connect_update", { dir });
    if (!projectStillOn(dir)) return;
    state.project.data = data;
    state.project.fresh = data.fresh;
    state.project.updated = true;
    // A new agent-kit may give a different draft: a tick given to the old one does not carry over.
    state.project.planOn = [];
  });
}

async function saveProjectSource() {
  const source = $("projectSourceInput").value.trim();
  if (!source) {
    $("projectSourceInput").focus();
    return;
  }
  try {
    await invoke("connect_set_source", { source });
    checkProject();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

function applyProject() {
  const data = state.project?.data;
  if (!data || state.project.busy) return;
  const dir = state.project.dir;
  const chosen = (data.plan ?? []).filter((item) => item.action !== "keep" && planTicked(state.project, item)).map((item) => item.id);
  const checks = checksOf(data).filter((check) => checkTicked(state.project, check)).map((check) => check.id);
  projectStep("apply", async () => {
    const next = await invoke("connect_apply", { root: data.root, chosen, checks });
    if (!projectStillOn(dir)) return;
    state.project.data = next;
    state.project.unticked = [];
    state.project.planOn = [];
    state.project.checksOff = [];
    state.project.checksOn = [];
    const applied = next.applied?.applied?.length ?? 0;
    // The steps to add by hand are shown under the checks, not counted as a stale item.
    const skipped = next.applied?.skipped?.filter((s) => s.id !== "verify-checks").length ?? 0;
    toast(t(skipped ? "project.appliedSkipped" : "project.applied", { count: applied, skipped }), skipped ? "" : "good");
  });
}

function hasSelection() {
  return state.isNew || !!currentProvider();
}

function currentProvider() {
  if (state.isNew) return draft;
  return state.config?.providers.find((p) => p.id === state.selectedId) ?? null;
}

// Leaving the form with unsaved changes asks first.
async function leaveForm() {
  const dirty = state.view === "settings" ? settingsDirty() : hasSelection() && isDirty();
  if (!dirty) return true;
  const answer = await ask({
    title: t("dialog.discardTitle"),
    text: t("dialog.discardText"),
    ok: t("dialog.discard"),
    cancel: t("dialog.keep"),
    danger: true,
  });
  return answer.value === "ok";
}

async function selectProvider(id) {
  if (state.view === "provider" && !state.isNew && id === state.selectedId) return;
  if (!(await leaveForm())) return;
  state.view = "provider";
  state.selectedId = id;
  state.isNew = false;
  state.originalId = id;
  state.clearKey = false;
  picker = [];
  formEpoch += 1;
  render();
}

async function newProvider() {
  if (!(await leaveForm())) return;
  state.view = "provider";
  state.selectedId = "__new__";
  state.isNew = true;
  state.originalId = null;
  state.clearKey = false;
  picker = [];
  formEpoch += 1;
  render();
  el.presetInput.focus();
}

async function openSettings() {
  if (state.view === "settings") return;
  if (!(await leaveForm())) return;
  state.view = "settings";
  render();
}

async function closeSettings() {
  if (!(await leaveForm())) return;
  state.view = "provider";
  render();
}

async function openProxy() {
  if (state.view === "proxy") return;
  if (!(await leaveForm())) return;
  state.view = "proxy";
  render();
  refreshProxy();
}

function closeProxy() {
  state.view = "provider";
  render();
}

// ---- proxy (mihomo) --------------------------------------------------------------------
// The backend answers every proxy command with the whole state; the view only draws it.

async function refreshProxy() {
  let fresh;
  try {
    fresh = await invoke("proxy_state");
  } catch {
    return;
  }
  // Nothing changed: no redraw, so keyboard focus on a node or a button stays put.
  if (JSON.stringify(fresh) === JSON.stringify(state.proxy)) return;
  state.proxy = fresh;
  renderChip();
  if (state.view === "proxy") renderProxy();
}

// Runs a proxy command; on failure the error goes to a toast and the state is read again.
async function proxyCall(cmd, args = {}, button = null, label = "") {
  const work = async () => {
    try {
      state.proxy = await invoke(cmd, args);
      return true;
    } catch (exc) {
      toast(formatError(exc), "error");
      await refreshProxy();
      return false;
    }
  };
  const ok = button ? await busyValue(button, label, work) : await work();
  renderChip();
  if (state.view === "proxy") renderProxy();
  return ok;
}

async function busyValue(button, label, work) {
  let value;
  await busy(button, label, async () => {
    value = await work();
  });
  return value;
}

function nodeLabel(name) {
  const p = state.proxy;
  return p?.nodes?.find((n) => n.name === name)?.label ?? name;
}

function delayText(delay) {
  if (delay === null || delay === undefined) return { text: t("proxy.untested"), kind: "" };
  if (delay === 0) return { text: t("proxy.timeout"), kind: "fail" };
  return { text: t("proxy.ms", { ms: delay }), kind: delay < 300 ? "good" : delay < 800 ? "mid" : "bad" };
}

// The title-bar chip: hidden while the proxy is off; red when the core is not working.
function renderChip() {
  const p = state.proxy;
  const on = !!p && p.mode !== "off";
  el.proxyChip.classList.toggle("hidden", !on);
  if (!on) return;
  const bad = !p.running || !!p.error || p.system?.error;
  el.proxyChip.classList.toggle("bad", !!bad);
  el.proxyChip.querySelector(".dot").classList.toggle("on", !bad);
  if (bad) {
    el.proxyChipText.textContent = t("proxy.chipBad");
  } else {
    const chosen = p.selected === "AUTO" ? p.autoNow : p.selected;
    const node = p.nodes.find((n) => n.name === chosen);
    const delay = node?.delay ? ` · ${t("proxy.ms", { ms: node.delay })}` : "";
    el.proxyChipText.textContent = t("proxy.chip", { name: chosen ? nodeLabel(chosen) : "DIRECT" }) + delay;
  }
  el.proxyChip.title = [t("proxy.chipTitle"), p.error, p.system?.error].filter(Boolean).join("\n");
}

function renderProxy() {
  const p = state.proxy;
  if (!p) return;
  for (const radio of document.querySelectorAll('input[name="proxyMode"]')) radio.checked = radio.value === p.mode;
  const errors = [p.error, p.system?.error].filter(Boolean);
  el.proxyError.textContent = errors.join("\n");
  $("proxyErrorBox").classList.toggle("hidden", !errors.length);
  // The system proxy failed (a policy, say): one click to the mode that needs no Windows setting.
  $("proxyToAppBtn").classList.toggle("hidden", !(errors.length && p.mode === "system"));
  el.proxyCoreLine.textContent = !p.coreVersion
    ? t("proxy.coreMissing", { version: p.pinnedVersion })
    : p.running
      ? t("proxy.coreRunning", { version: p.coreVersion, port: p.port })
      : t("proxy.coreStopped", { version: p.coreVersion });
  if (!downloading.core) {
    el.proxyCoreBtn.textContent = t(p.coreVersion ? "proxy.coreRedownload" : "proxy.coreDownload");
    el.proxyCoreHint.textContent = p.system?.on ? t("proxy.systemOn") : "";
  }
  el.proxyCoreBtn.classList.toggle("primary", !p.coreVersion);
  el.proxyCoreBtn.classList.toggle("ghost", !!p.coreVersion);
  renderSubs(p);
  renderNodes(p);
}

function renderSubs(p) {
  el.subList.replaceChildren();
  for (const sub of p.subs) {
    const li = document.createElement("li");
    li.className = "sub-row";
    const text = document.createElement("div");
    const name = document.createElement("b");
    name.textContent = sub.name;
    const host = document.createElement("span");
    host.className = "url";
    host.textContent = sub.host;
    const meta = document.createElement("span");
    meta.className = "meta";
    const parts = [];
    if (sub.updatedAt) {
      const time = new Date(sub.updatedAt).toLocaleString(LANG, { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
      parts.push(t("proxy.subMeta", { nodes: nodeWord(sub.count), time }));
    }
    if (sub.total) {
      const gb = (bytes) => (bytes / 1073741824).toLocaleString(LANG, { maximumFractionDigits: 1 });
      parts.push(t("proxy.subTraffic", { used: gb((sub.upload ?? 0) + (sub.download ?? 0)), total: gb(sub.total) }));
    }
    if (sub.expire) parts.push(t("proxy.subExpire", { date: new Date(sub.expire * 1000).toLocaleDateString(LANG) }));
    meta.textContent = parts.join(" · ");
    text.append(name, host, meta);
    const buttons = document.createElement("div");
    buttons.className = "button-row";
    const update = document.createElement("button");
    update.type = "button";
    update.className = "ghost small";
    update.textContent = t("proxy.subUpdate");
    update.disabled = !p.running;
    update.addEventListener("click", () => proxyCall("proxy_update_sub", { id: sub.id }, update, t("proxy.coreBusy")));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "ghost small danger";
    remove.textContent = t("proxy.subRemove");
    remove.addEventListener("click", () => removeSub(sub));
    buttons.append(update, remove);
    li.append(text, buttons);
    el.subList.append(li);
  }
}

// `meta` is shown under the name (Auto: what it chose); `hint` only on hover (the protocol).
// A plain 34 px ghost button: the name on the left, the latency on the right; the protocol
// only on hover.
function nodeButton(name, label, delay, pressed, hint = "") {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "ghost node";
  button.setAttribute("aria-pressed", String(pressed));
  button.title = [label, hint].filter(Boolean).join(" · ");
  const title = document.createElement("span");
  title.className = "node-name";
  title.textContent = label;
  const ms = document.createElement("span");
  const d = delayText(delay);
  ms.className = `delay ${d.kind}`;
  ms.textContent = d.text;
  button.append(title, ms);
  button.addEventListener("click", () => {
    if (!pressed) proxyCall("proxy_select", { name });
  });
  return button;
}

function renderNodes(p) {
  el.nodeGrid.replaceChildren();
  el.measureBtn.disabled = !p.running || !p.nodes.length;
  const empty = !p.subs.length
    ? "proxy.nodesNoSubs"
    : !p.running
      ? p.mode === "off" ? "proxy.nodesOff" : "proxy.nodesStopped"
      : !p.nodes.length ? "proxy.nodesLoading" : null;
  el.nodesEmpty.textContent = empty ? t(empty) : "";
  el.nodesEmpty.classList.toggle("hidden", !empty);
  if (empty) return;
  const auto = p.nodes.find((n) => n.name === p.autoNow);
  el.nodeGrid.append(
    nodeButton("AUTO", auto ? t("proxy.autoNow", { name: auto.label }) : t("proxy.auto"), auto?.delay, p.selected === "AUTO", t("proxy.auto")),
  );
  const several = p.subs.length > 1;
  for (const sub of p.subs) {
    const nodes = p.nodes.filter((n) => n.sub === sub.id);
    if (several && nodes.length) {
      const head = document.createElement("h4");
      head.className = "node-group";
      head.textContent = sub.name;
      el.nodeGrid.append(head);
    }
    for (const node of nodes) el.nodeGrid.append(nodeButton(node.name, node.label, node.delay, p.selected === node.name, node.kind));
  }
}

// The whole-PC modes ask first: they change the route of every program, the regular Claude too.
async function setProxyMode(mode) {
  const p = state.proxy;
  if (mode === p?.mode) return;
  const ask_ = { system: "proxy.askSystem", tun: "proxy.askTun" }[mode];
  if (ask_) {
    // The new mode is lit only after the answer: until then the current one stays marked.
    renderProxy();
    const answer = await ask({ title: t(`${ask_}Title`), text: t(`${ask_}Text`), ok: t("proxy.askOk"), cancel: t("dialog.cancel") });
    if (answer.value !== "ok") {
      renderProxy();
      return;
    }
  }
  // One change at a time: TUN may wait for the UAC answer, and arrows would apply every mode
  // they pass.
  const radios = [...document.querySelectorAll('input[name="proxyMode"]'), $("proxyToAppBtn")];
  for (const radio of radios) radio.disabled = true;
  let ok;
  try {
    ok = await proxyCall("proxy_set_mode", { mode });
  } finally {
    for (const radio of radios) radio.disabled = false;
    renderProxy();
  }
  if (ok && state.claudeRun?.running) toast(t("proxy.reopenClaude"), "");
}

async function downloadCore() {
  downloading.core = true;
  try {
    if (await proxyCall("proxy_download_core", {}, el.proxyCoreBtn, t("proxy.coreBusy"))) toast(t("proxy.coreReady"), "good");
  } finally {
    downloading.core = false;
    renderProxy();
  }
}

async function addSub(event) {
  event.preventDefault();
  const url = el.subUrlInput.value.trim();
  if (!url) {
    el.subUrlInput.focus();
    return;
  }
  const ok = await proxyCall(
    "proxy_add_sub",
    { name: el.subNameInput.value, url, hours: Number(el.subIntervalInput.value) },
    el.subAddBtn,
    t("proxy.coreBusy"),
  );
  if (!ok) return;
  el.subNameInput.value = "";
  el.subUrlInput.value = "";
  toast(t("proxy.added"), "good");
  // The core fetches the subscription in the background: look again shortly.
  setTimeout(refreshProxy, 1500);
}

async function removeSub(sub) {
  const answer = await ask({
    title: t("proxy.subRemoveTitle", { name: sub.name }),
    text: t("proxy.subRemoveText"),
    ok: t("proxy.subRemove"),
    cancel: t("dialog.cancel"),
    danger: true,
  });
  if (answer.value === "ok") await proxyCall("proxy_remove_sub", { id: sub.id });
}

function renderForm() {
  const provider = currentProvider();
  if (!provider) return;
  el.formTitle.textContent = state.isNew ? t("form.newTitle") : provider.name || provider.id;
  // The URL and the type are in the fields right below; the subtitle only explains a new form.
  el.formSubtitle.textContent = state.isNew ? t("form.newSubtitle") : "";
  el.formSubtitle.classList.toggle("hidden", !state.isNew);
  el.formKindBadge.classList.add("hidden");
  el.formKeyBadge.textContent = t(provider.hasKey ? "form.keySaved" : provider.needsKey ? "form.keyNotSet" : "form.keyNotNeeded");
  el.formKeyBadge.className = `badge soft ${provider.hasKey ? "good" : ""}`;

  el.presetField.classList.toggle("hidden", !state.isNew);
  el.presetInput.value = "";
  renderPicker();

  el.idInput.value = provider.id;
  el.nameInput.value = provider.name;
  el.kindInput.value = provider.kind;
  el.baseInput.value = provider.baseUrl;
  el.modelsInput.value = (provider.models || []).join("\n");
  // Tall enough for the whole list (4 to 10 lines): no half-cut last line.
  el.modelsInput.rows = Math.min(10, Math.max(4, (provider.models || []).length + 1));
  labels = { ...(provider.labels ?? {}) };
  el.maxEffortInput.value = provider.maxEffort ?? "";
  el.defaultEffortInput.value = provider.defaultEffort ?? "";
  syncEffort();

  el.keyInput.value = "";
  el.keyInput.placeholder = t(provider.hasKey ? "form.keyPlaceholderSaved" : "form.keyPlaceholder");
  el.clearKeyBtn.classList.toggle("hidden", !provider.hasKey || state.isNew);
  el.deleteBtn.classList.toggle("hidden", state.isNew);
  renderKeyHint(provider);

  el.aliasBox.classList.toggle("hidden", !provider.aliases?.length);
  $("aliasSummary").textContent = t("form.aliases", { count: provider.aliases?.length ?? 0 });
  el.aliasList.replaceChildren();
  for (const alias of provider.aliases ?? []) {
    const row = document.createElement("div");
    row.className = "alias-row";
    const from = document.createElement("span");
    from.className = "from";
    from.textContent = alias.label ? `${alias.model} (${alias.label})` : alias.model;
    const arrow = document.createElement("span");
    arrow.className = "arrow";
    arrow.textContent = "→";
    const to = document.createElement("span");
    to.className = "to";
    to.textContent = alias.alias;
    row.append(from, arrow, to);
    el.aliasList.append(row);
  }
  formSnapshot = snapshot();
  renderNotice();
}

function renderKeyHint(provider = currentProvider()) {
  if (!provider) return;
  el.keyHint.textContent = state.clearKey
    ? t("form.keyHintClear")
    : t(provider.needsKey ? (provider.hasKey ? "form.keyHintSaved" : "form.keyHintMissing") : "form.keyHintLocal");
}

function snapshot() {
  return JSON.stringify([formProvider(), el.keyInput.value.trim(), state.clearKey]);
}

function isDirty() {
  return !el.form.classList.contains("hidden") && snapshot() !== formSnapshot;
}

// Settings fields differ from what is saved (same normalisation as saveSettings).
function settingsDirty() {
  const c = state.config;
  if (!c) return false;
  return (
    el.languageInput.value !== (c.languagePref ?? "auto") ||
    el.themeInput.value !== (c.themePref ?? "auto") ||
    Number(el.portInput.value) !== c.port ||
    el.claudePathInput.value.trim() !== (c.claudePath ?? "") ||
    el.codeDirInput.value.trim() !== (c.codeDir ?? "") ||
    el.russianInput.checked !== !!c.russianUi ||
    el.builtinBrowserInput.checked !== !!c.builtinBrowser ||
    el.autoUpdateInput.checked !== (c.autoUpdate !== false) ||
    el.codePortableInput.checked !== !!c.codePortable ||
    ROLES.some(([role, id]) => el[id].value !== roleKey(c.modelRoles?.[role]))
  );
}

// Model roles: role, select id, text of the "not chosen" option. The main one (the strong and
// fast roles are gone, every role goes to it) and the critic for project scripts (ADR-012).
const ROLES = [
  ["sonnet", "roleSonnet", "settings.roleFirst"],
  ["critic", "roleCritic", "settings.roleCriticNone"],
];

// A role is stored as provider + model id (no [1m]): the alias changes with the provider.
function roleKey(ref) {
  return ref ? JSON.stringify([ref.provider, bareId(ref.model)]) : "";
}

function readRole(select) {
  if (!select.value) return null;
  const [provider, model] = JSON.parse(select.value);
  return { provider, model };
}

function fillRole(select, ref, emptyText) {
  const add = (value, text) => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = text;
    select.append(option);
  };
  select.replaceChildren();
  add("", t(emptyText));
  // The provider name only tells models apart when there are several providers.
  const several = state.config.providers.length > 1;
  for (const provider of state.config.providers) {
    for (const row of provider.aliases ?? []) {
      const id = bareId(row.model);
      const name = row.label ?? id;
      add(JSON.stringify([provider.id, id]), several ? `${name} · ${provider.name || provider.id}` : name);
    }
  }
  // A saved choice whose model is gone stays visible (and unchanged) until replaced.
  const key = roleKey(ref);
  if (key && ![...select.options].some((option) => option.value === key)) {
    add(key, t("settings.roleMissing", { model: bareId(ref.model) }));
  }
  select.value = key;
}

function renderNotice() {
  const provider = currentProvider();
  if (!provider) return;
  const dirty = isDirty();
  el.formNotice.classList.toggle("dirty", dirty);
  el.formNotice.textContent = dirty
    ? t("form.unsaved")
    : provider.aliases?.length
      ? t("form.aliasesReady", { models: modelWord(provider.aliases.length) })
      : t("form.aliasesLater");
}

function renderSettings() {
  if (!state.config) return;
  el.languageInput.value = state.config.languagePref ?? "auto";
  el.themeInput.value = state.config.themePref ?? "auto";
  el.portInput.value = state.config.port;
  el.claudePathInput.value = state.config.claudePath ?? "";
  el.codeDirInput.value = state.config.codeDir ?? "";
  for (const [role, id, emptyText] of ROLES) fillRole(el[id], state.config.modelRoles?.[role], emptyText);
  el.tokenInput.value = state.config.token;
  el.detectedClaude.textContent = state.config.detectedClaude
    ? t("settings.found", { path: state.config.detectedClaude })
    : t("settings.notFound");
  // One line with an ellipsis (a long user name would wrap it); the full path on hover.
  el.detectedClaude.title = state.config.detectedClaude ?? "";
  el.russianInput.checked = !!state.config.russianUi;
  el.builtinBrowserInput.checked = !!state.config.builtinBrowser;
  // Whether the translation is fine is said by the self-check below; here only "not yet".
  el.russianFileHint.textContent = state.config.russianFile ? "" : t("settings.russianFileMissing");
  el.russianFileHint.classList.toggle("hidden", !!state.config.russianFile);
  el.versionText.textContent = t("update.current", { version: state.config.version });
  el.autoUpdateInput.checked = state.config.autoUpdate !== false;
  renderDownloads();
  syncSettingsSave();
}

// Download rows of Claude Desktop and Claude Code: what is there and what the buttons do.
// A running download writes its progress into the hint instead (onDownload).
const downloading = {};
function renderDownloads() {
  const c = state.config;
  const got = c.installed ?? {};
  el.desktopPortableBtn.textContent = t(got.desktopPortable ? "settings.unpackUpdate" : "settings.unpack");
  // Over an existing regular install the button updates it (and asks first).
  el.desktopInstallBtn.textContent = t(got.desktopInstalled ? "settings.installUpdate" : "settings.install");
  el.codeInstallBtn.textContent = t(c.detectedCode ? "settings.installUpdate" : "settings.install");
  // With the app's VPN on, downloads go through it: say so under both rows.
  const vpn = state.proxy && state.proxy.mode !== "off" ? " " + t("settings.viaVpn") : "";
  if (!downloading.desktop) {
    el.desktopGetHint.textContent =
      (got.desktopPortable
        ? t("settings.desktopPortableHint", { version: got.desktopPortable })
        : t("settings.desktopGetHint")) + vpn;
  }
  el.codeFound.textContent = c.detectedCode
    ? t("settings.codeFound", { path: c.detectedCode })
    : t("settings.codeNotFound");
  el.codeFound.title = c.detectedCode ?? "";
  el.codePortableInput.checked = !!c.codePortable;
  el.codePortableInput.disabled = !got.codePortable && !c.codePortable;
  el.codePortableBtn.textContent = t(got.codePortable ? "settings.codePortableUpdate" : "settings.codePortableGet");
  if (!downloading.code) {
    el.codeGetHint.textContent =
      (got.codePortable
        ? t("settings.codePortableHint", { version: got.codePortable })
        : t("settings.codeGetHint")) + vpn;
  }
}

function onDownload({ payload }) {
  const hint = { desktop: el.desktopGetHint, code: el.codeGetHint, core: el.proxyCoreHint }[payload?.id];
  if (!hint || !downloading[payload.id]) return;
  const mb = (bytes) => (bytes / 1048576).toFixed(0);
  hint.textContent =
    payload.total && payload.done >= payload.total
      ? t("download.checking")
      : payload.total
        ? t("download.progress", { done: mb(payload.done), total: mb(payload.total) })
        : t("download.progressUnknown", { done: mb(payload.done) });
}

// Installing over a regular Claude touches the person's own copy: ask first.
async function getClaude(kind, portable) {
  const c = state.config;
  const existing = kind === "desktop" ? c.installed?.desktopInstalled : c.detectedCode;
  if (!portable && existing) {
    const prefix = kind === "desktop" ? "dialog.installDesktop" : "dialog.installCode";
    const answer = await ask({
      title: t(`${prefix}Title`),
      text: t(`${prefix}Text`, { path: existing }),
      ok: t("dialog.install"),
      cancel: t("dialog.cancel"),
    });
    if (answer.value !== "ok") return;
  }
  const buttons = kind === "desktop" ? [el.desktopPortableBtn, el.desktopInstallBtn] : [el.codePortableBtn, el.codeInstallBtn];
  const button = portable ? buttons[0] : buttons[1];
  const other = portable ? buttons[1] : buttons[0];
  downloading[kind] = true;
  other.disabled = true;
  try {
    await busy(button, t("download.busy"), async () => {
      const result = await invoke(kind === "desktop" ? "download_desktop" : "download_code", { portable });
      state.config = result.config;
      toast(result.message, "good");
    });
  } catch (exc) {
    toast(formatError(exc), "error");
  } finally {
    downloading[kind] = false;
    other.disabled = false;
    // Unsaved edits in the settings stay; then only the download rows are redrawn.
    if (settingsDirty()) renderDownloads();
    else renderSettings();
  }
}

// "Save settings" is active only when something differs from what is saved.
function syncSettingsSave() {
  const dirty = settingsDirty();
  $("saveSettingsBtn").disabled = !dirty;
  // The mark in front of the hint says there is something to save; the hint itself stays.
  $("settingsDirty").classList.toggle("hidden", !dirty);
}

// Self-check of the installed Claude: the list in Settings and a badge in the title bar.
function renderClaudeCheck() {
  const check = state.status?.claude;
  const items = check?.items ?? [];
  // An optional feature that is missing is listed, but does not light the title-bar warning.
  el.claudeWarn.classList.toggle("hidden", !items.some((item) => !item.ok && !item.optional));
  el.claudeCheckList.replaceChildren();
  if (!check) {
    el.claudeCheckList.append(checkItem("busy", t("settings.checking")));
    return;
  }
  // Everything fine: one line (the list is in the tooltip); a problem opens the whole list.
  if (items.length && items.every((item) => item.ok)) {
    const li = document.createElement("li");
    li.className = "ok";
    li.textContent = `✓ ${t("settings.checkOk", { version: check.version })}`;
    li.title = items.map((item) => item.text).join("\n");
    el.claudeCheckList.append(li);
    return;
  }
  if (check.version) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = `Claude ${check.version}`;
    el.claudeCheckList.append(li);
  }
  for (const item of items) {
    const li = document.createElement("li");
    // A missing optional feature is neutral, not an error.
    const kind = item.ok ? "ok" : item.optional ? "off" : "bad";
    li.className = kind;
    li.textContent = `${{ ok: "✓", off: "–", bad: "✕" }[kind]} ${item.text}`;
    el.claudeCheckList.append(li);
  }
}

function bind() {
  $("winMin").addEventListener("click", () => invoke("win_minimize").catch(() => {}));
  $("winMax").addEventListener("click", () => invoke("win_toggle_maximize").catch(() => {}));
  $("winClose").addEventListener("click", closeToTray);

  el.claudeWarn.addEventListener("click", openSettings);
  el.langBtn.addEventListener("click", () => setUi({ language: LANG === "ru" ? "en" : "ru" }));
  el.themeBtn.addEventListener("click", () => {
    setUi({ theme: document.documentElement.dataset.theme === "light" ? "dark" : "light" });
  });
  $("addBtn").addEventListener("click", newProvider);
  $("emptyAdd").addEventListener("click", newProvider);
  $("settingsBtn").addEventListener("click", openSettings);
  $("closeSettings").addEventListener("click", closeSettings);

  el.form.addEventListener("submit", saveProvider);
  el.form.addEventListener("input", renderNotice);
  el.form.addEventListener("change", renderNotice);
  el.presetInput.addEventListener("change", applyPreset);
  el.maxEffortInput.addEventListener("change", syncEffort);
  el.fetchModelsBtn.addEventListener("click", fetchModels);
  el.testBtn.addEventListener("click", testProvider);
  el.pickerFilter.addEventListener("input", renderPicker);
  el.modelsInput.addEventListener("input", renderPicker);
  // Address or type changed during "Load models": that answer is about something else now.
  for (const input of [el.baseInput, el.kindInput]) {
    input.addEventListener("input", () => {
      formEpoch += 1;
    });
  }
  $("pickerClose").addEventListener("click", () => {
    picker = [];
    renderPicker();
  });
  el.deleteBtn.addEventListener("click", deleteProvider);
  el.clearKeyBtn.addEventListener("click", () => {
    state.clearKey = true;
    el.keyInput.value = "";
    renderKeyHint();
    el.clearKeyBtn.classList.add("hidden");
    renderNotice();
  });
  el.keyInput.addEventListener("input", () => {
    if (el.keyInput.value.trim()) state.clearKey = false;
  });

  el.startBtn.addEventListener("click", () => startGateway());
  el.stopBtn.addEventListener("click", stopGateway);
  el.launchBtn.addEventListener("click", launchClaude);
  el.launchCodeBtn.addEventListener("click", launchClaudeCode);

  el.logsBtn.addEventListener("click", toggleLogs);
  $("refreshLogsBtn").addEventListener("click", refreshLogs);
  $("closeLogsBtn").addEventListener("click", () => setLogs(false));

  $("saveSettingsBtn").addEventListener("click", saveSettings);
  el.settingsPanel.addEventListener("input", syncSettingsSave);
  el.settingsPanel.addEventListener("change", syncSettingsSave);
  $("copyTokenBtn").addEventListener("click", () => copyText(el.tokenInput.value, t("toast.tokenCopied")));
  $("openLogsFolderBtn").addEventListener("click", () => invoke("open_logs").catch((e) => toast(formatError(e), "error")));
  el.updateBtn.addEventListener("click", installUpdate);
  el.checkUpdateBtn.addEventListener("click", checkUpdate);
  $("browseClaudeBtn").addEventListener("click", () => browse(el.claudePathInput));
  $("browseCodeBtn").addEventListener("click", () => browse(el.codeDirInput));
  $("projectBtn").addEventListener("click", openProject);
  $("closeProject").addEventListener("click", closeProject);
  $("browseProjectBtn").addEventListener("click", async () => {
    if (state.project?.busy) return;
    await browse($("projectDirInput"));
    if ($("projectDirInput").value.trim() !== (state.project?.dir ?? "")) resetProject();
  });
  $("projectCheckBtn").addEventListener("click", checkProject);
  $("projectDirInput").addEventListener("keydown", (event) => event.key === "Enter" && checkProject());
  $("projectDirInput").addEventListener("input", resetProject);
  $("projectSourceBtn").addEventListener("click", saveProjectSource);
  $("projectUpdateBtn").addEventListener("click", updateProject);
  $("projectConnectBtn").addEventListener("click", connectProject);
  $("projectApplyBtn").addEventListener("click", applyProject);
  el.proxyBtn.addEventListener("click", openProxy);
  el.proxyChip.addEventListener("click", openProxy);
  $("closeProxy").addEventListener("click", closeProxy);
  for (const radio of document.querySelectorAll('input[name="proxyMode"]')) {
    radio.addEventListener("change", () => radio.checked && setProxyMode(radio.value));
  }
  el.proxyCoreBtn.addEventListener("click", downloadCore);
  $("proxyToAppBtn").addEventListener("click", () => setProxyMode("app"));
  el.subForm.addEventListener("submit", addSub);
  el.measureBtn.addEventListener("click", () => proxyCall("proxy_measure", {}, el.measureBtn, t("proxy.measuring")));
  el.desktopPortableBtn.addEventListener("click", () => getClaude("desktop", true));
  el.desktopInstallBtn.addEventListener("click", () => getClaude("desktop", false));
  el.codePortableBtn.addEventListener("click", () => getClaude("code", true));
  el.codeInstallBtn.addEventListener("click", () => getClaude("code", false));
  window.__TAURI__?.event?.listen("download", onDownload);

  // Enter outside a button would submit the dialog with its first button ("exit and stop
  // the gateway" in the tray dialog). Only a focused button answers with Enter.
  el.dialog.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && event.target.tagName !== "BUTTON") event.preventDefault();
  });

  $("toastClose").addEventListener("click", hideToast);
  $("toastCopy").addEventListener("click", () => copyText(el.toastText.textContent, t("toast.copied")));

  document.addEventListener("keydown", (event) => {
    if (el.dialog.open || $("helpDialog").open) return; // a dialog handles its own keys
    if (event.key === "Escape") {
      if (el.toast.classList.contains("show")) hideToast();
      else if (state.logsOpen) setLogs(false);
      else if (state.view === "proxy") closeProxy();
      else if (state.view === "project") closeProject();
      else if (state.view === "settings") {
        // Without this the same Esc would at once close the "discard changes?" dialog.
        event.preventDefault();
        closeSettings();
      }
    }
    // key: Ctrl+S in any Latin layout; code: the same physical key in the Russian one.
    if ((event.ctrlKey || event.metaKey) && (event.key.toLowerCase() === "s" || event.code === "KeyS")) {
      event.preventDefault();
      if (state.view === "settings") saveSettings();
      else if (!el.form.classList.contains("hidden")) el.form.requestSubmit();
    }
  });
}

// "Default" without a ceiling is ignored by the gateway — don't let it be chosen in vain.
function syncEffort() {
  const off = !el.maxEffortInput.value;
  el.defaultEffortInput.disabled = off;
  if (off) el.defaultEffortInput.value = "";
}

function formModels() {
  return el.modelsInput.value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function bareId(model) {
  return model.replace(/\[1m\]$/i, "").trim();
}

function formProvider() {
  const models = formModels();
  const ids = new Set(models.map(bareId));
  return {
    id: el.idInput.value.trim(),
    name: el.nameInput.value.trim(),
    kind: el.kindInput.value,
    baseUrl: el.baseInput.value.trim(),
    models,
    maxEffort: el.maxEffortInput.value || null,
    defaultEffort: el.defaultEffortInput.value || null,
    labels: Object.fromEntries(Object.entries(labels).filter(([id]) => ids.has(id))),
  };
}

// Key for probes: from the field; null — the saved one; "" — no key ("Remove key").
function probeInput() {
  const apiKey = el.keyInput.value.trim();
  return {
    provider: formProvider(),
    originalId: state.originalId,
    apiKey: apiKey || (state.clearKey ? "" : null),
  };
}

function applyPreset() {
  const preset = PRESETS[Number(el.presetInput.value)];
  if (!preset || el.presetInput.value === "") return;
  const taken = new Set((state.config?.providers ?? []).map((p) => p.id));
  let id = preset.id;
  for (let n = 2; taken.has(id); n += 1) id = `${preset.id}-${n}`;
  el.idInput.value = id;
  el.nameInput.value = preset.name;
  el.kindInput.value = preset.kind;
  el.baseInput.value = preset.baseUrl;
  el.formKindBadge.textContent = preset.kind;
  picker = [];
  formEpoch += 1; // a "Load models" answer for the old address is no longer wanted
  renderPicker();
  renderNotice();
  el.keyInput.focus();
}

// Catalog hint for an ID; hasOwn so a model named "constructor" finds no Object.
function hintFor(id) {
  return Object.hasOwn(catalog.models ?? {}, id) ? catalog.models[id] : undefined;
}

async function fetchModels() {
  const form = formEpoch;
  await busy(el.fetchModelsBtn, t("form.fetching"), async () => {
    try {
      const fetched = await invoke("fetch_models", { input: probeInput() });
      if (form !== formEpoch) return; // another provider opened meanwhile
      picker = fetched.ids;
      catalog = fetched.catalog ?? { source: null, models: {} };
      // Already chosen models without a name get the catalog's (saved with the form).
      for (const id of formModels().map(bareId)) {
        const name = hintFor(id)?.name;
        if (name && !Object.hasOwn(labels, id)) labels[id] = name;
      }
      el.pickerFilter.value = "";
      renderPicker();
      renderNotice();
      if (picker.length) el.pickerBox.scrollIntoView({ block: "nearest", behavior: "smooth" });
      else toast(t("toast.fetchEmpty"), "error");
    } catch (exc) {
      toast(t("toast.fetchError", { error: formatError(exc) }), "error");
    }
  });
}

function renderPicker() {
  el.pickerBox.classList.toggle("hidden", !picker.length);
  if (!picker.length) return;
  const chosen = new Set(formModels().map(bareId));
  const filter = el.pickerFilter.value.trim().toLowerCase();
  const shown = picker.filter((id) => !filter || `${id} ${hintFor(id)?.name ?? ""}`.toLowerCase().includes(filter));
  const source = catalog.source ? t("picker.catalog", { source: catalog.source }) : "";
  el.pickerCount.textContent = t("picker.count", { shown: shown.length, total: picker.length, chosen: chosen.size }) + source;
  el.pickerList.replaceChildren();
  for (const id of shown) {
    const hint = hintFor(id);
    const on = chosen.has(id);
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = `chip ${on ? "on" : ""}`;
    chip.setAttribute("aria-pressed", String(on));
    chip.textContent = hint?.context >= 1_000_000 ? `${id} · 1M` : id;
    chip.title = [
      hint?.name,
      hint?.context ? t("picker.context", { k: Math.round(hint.context / 1000) }) : "",
      hint?.efforts?.length ? t("picker.levels", { levels: hint.efforts.join(", ") }) : "",
      t(on ? "picker.remove" : "picker.add"),
    ]
      .filter(Boolean)
      .join(" · ");
    chip.addEventListener("click", () => toggleModel(id));
    el.pickerList.append(chip);
  }
}

function toggleModel(id) {
  const models = formModels();
  const index = models.findIndex((m) => bareId(m) === id);
  const hint = hintFor(id);
  if (index >= 0) {
    models.splice(index, 1);
  } else {
    // From the catalog: 1M context, a name for Claude's menu and levels if none are set yet.
    models.push(hint?.context >= 1_000_000 ? `${id}[1m]` : id);
    if (hint?.name) labels[id] = hint.name;
    if (hint?.efforts?.length && !el.maxEffortInput.value) {
      el.maxEffortInput.value = hint.efforts[hint.efforts.length - 1];
      syncEffort();
      el.defaultEffortInput.value = hint.defaultEffort ?? "";
      toast(t("toast.catalogLevels", { level: el.maxEffortInput.value }));
    }
  }
  el.modelsInput.value = models.join("\n");
  renderPicker();
  renderNotice();
}

async function testProvider() {
  await busy(el.testBtn, t("form.testing"), async () => {
    try {
      toast(await invoke("test_provider", { input: probeInput() }), "good");
    } catch (exc) {
      toast(t("toast.testError", { error: formatError(exc) }), "error");
    }
  });
}

// The brand book's work mark (see .work-mark in styles.css).
function workMark() {
  return Object.assign(document.createElement("span"), { className: "work-mark" });
}

async function busy(button, label, work) {
  const text = button.textContent;
  button.disabled = true;
  button.setAttribute("aria-busy", "true");
  button.replaceChildren(workMark(), label);
  try {
    await work();
  } finally {
    button.disabled = false;
    button.removeAttribute("aria-busy");
    button.textContent = text;
  }
}

// Saving a provider stops the gateway (routes change); if it was running, start it again.
// `kind` "error": the save went through, but part of it did not (one toast says both).
async function restartIfWas(wasRunning, done, kind = "good") {
  state.status = await invoke("status");
  if (!wasRunning) {
    toast(done, kind);
    return;
  }
  const started = await startGateway({ quiet: true });
  if (started) toast(t("toast.restarted", { done }), kind);
  else if (started === null) toast(done, kind); // a later command overrode the restart
}

let saving = false;

async function saveProvider(event) {
  event.preventDefault();
  if (saving) return; // a double click would go out with the old originalId
  saving = true;
  try {
    await saveProviderOnce();
  } finally {
    saving = false;
  }
}

async function saveProviderOnce() {
  const provider = formProvider();
  const apiKey = el.keyInput.value.trim();
  const input = { provider, originalId: state.originalId, apiKey: apiKey || null, clearKey: state.clearKey };
  const wasRunning = starting > 0 || !!state.status?.running;
  const first = !state.config?.providers?.length;
  try {
    state.config = await invoke("save_provider", { input });
    state.selectedId = provider.id.toLowerCase();
    state.originalId = state.selectedId;
    state.isNew = false;
    state.clearKey = false;
    picker = [];
    render();
    await restartIfWas(wasRunning, t("toast.providerSaved"));
    if (first) await autostart(); // the first provider: there is something to serve now
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

async function deleteProvider() {
  const provider = currentProvider();
  if (!provider || state.isNew) return;
  const answer = await ask({
    title: t("dialog.deleteTitle", { name: provider.name || provider.id }),
    text: t("dialog.deleteText"),
    ok: t("dialog.delete"),
    cancel: t("dialog.cancel"),
    danger: true,
  });
  if (answer.value !== "ok") return;
  const wasRunning = (starting > 0 || !!state.status?.running) && state.config.providers.length > 1;
  try {
    state.config = await invoke("delete_provider", { id: provider.id });
    state.selectedId = state.config.providers[0]?.id ?? null;
    state.originalId = state.selectedId;
    state.isNew = false;
    state.clearKey = false; // otherwise "remove key" of the deleted provider would hit a neighbour
    picker = [];
    formEpoch += 1;
    render();
    await restartIfWas(wasRunning, t("toast.providerDeleted"));
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// How many starts are in flight: a save meanwhile must also bring the gateway up after itself.
let starting = 0;

// true — running; false — error (shown); null — a later command overrode the start.
async function startGateway({ quiet = false } = {}) {
  starting += 1;
  try {
    state.status = await invoke("start_gateway");
    renderStatus();
    if (!state.status.running) {
      if (!quiet) toast(t("toast.startCancelled"));
      return null;
    }
    if (!quiet) toast(t("toast.listening", { port: state.status.port }), "good");
    return true;
  } catch (exc) {
    state.status = await invoke("status").catch(() => state.status);
    renderStatus();
    toast(t("toast.notStarted", { error: formatError(exc) }), "error");
    return false;
  } finally {
    starting -= 1;
  }
}

async function stopGateway() {
  try {
    state.status = await invoke("stop_gateway");
    renderStatus();
    toast(t("toast.stopped"));
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// Claude is launching: the status poll must not re-enable the button (a second click would
// start a second build of the Russian copy).
let launching = false;
let launchingCode = false;

async function launchClaude() {
  if (launching) return;
  launching = true;
  // The first build of the Russian copy takes a while — the button shows work in progress.
  const label = t(state.config?.russianUi ? "actions.preparingCopy" : "actions.launching");
  try {
    await busy(el.launchBtn, label, async () => {
      try {
        const result = await invoke("launch_claude");
        state.launchError = null;
        toast(result.message, result.warning ? "error" : "good");
      } catch (exc) {
        state.launchError = formatError(exc);
        toast(state.launchError, "error");
      }
    });
  } finally {
    launching = false;
    renderStatus();
    // Claude needs a few seconds to take the profile lock: ask again now and a bit later.
    pollStatus();
    setTimeout(pollStatus, 4000);
  }
}

async function launchClaudeCode() {
  if (launchingCode) return;
  launchingCode = true;
  try {
    await busy(el.launchCodeBtn, t("actions.opening"), async () => {
      try {
        toast(await invoke("launch_claude_code"), "good");
      } catch (exc) {
        toast(formatError(exc), "error");
      }
    });
  } finally {
    launchingCode = false;
    renderStatus(); // busy re-enables the button — give it back the gateway's state
  }
}

async function checkUpdate() {
  await busy(el.checkUpdateBtn, t("update.checking"), async () => {
    try {
      const found = await invoke("check_update");
      state.status = { ...state.status, update: found };
      renderStatus();
      toast(found ? t("update.found", { version: found.version }) : t("update.none"), "good");
    } catch (exc) {
      toast(formatError(exc), "error");
    }
  });
}

let installingUpdate = false;

async function installUpdate() {
  const update = state.status?.update;
  if (!update || installingUpdate) return;
  const answer = await ask({
    title: t("update.confirmTitle", { version: update.version }),
    text: update.notes ? `${t("update.confirmText")}\n\n${update.notes}` : t("update.confirmText"),
    ok: t("update.install"),
    cancel: t("dialog.cancel"),
  });
  if (answer.value !== "ok") return;
  installingUpdate = true;
  el.updateBtn.disabled = true;
  el.updateBtn.textContent = t("update.installing");
  try {
    await invoke("install_update"); // on success the app restarts and this never returns
  } catch (exc) {
    toast(formatError(exc), "error");
  } finally {
    installingUpdate = false;
    el.updateBtn.disabled = false;
    renderStatus();
  }
}

async function browse(input) {
  const picked = await invoke("pick_folder", { start: input.value.trim() || null }).catch(() => null);
  if (picked) input.value = picked;
}

async function saveSettings() {
  const port = Number(el.portInput.value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    toast(t("settings.portInvalid"), "error");
    el.portInput.focus();
    return;
  }
  // A new port or a new main model (the gateway's default) stop the gateway; then bring it
  // back if it was running. The critic is for scripts only and needs no restart.
  const portChanged = port !== state.config?.port;
  const rolesChanged = el.roleSonnet.value !== roleKey(state.config?.modelRoles?.sonnet);
  const wasRunning = (portChanged || rolesChanged) && (starting > 0 || !!state.status?.running);
  try {
    state.config = await invoke("save_settings", {
      input: {
        port,
        claudePath: el.claudePathInput.value.trim() || null,
        codeDir: el.codeDirInput.value.trim() || null,
        modelRoles: Object.fromEntries(ROLES.map(([role, id]) => [role, readRole(el[id])])),
        russianUi: el.russianInput.checked,
        builtinBrowser: el.builtinBrowserInput.checked,
        language: el.languageInput.value,
        theme: el.themeInput.value,
        autoUpdate: el.autoUpdateInput.checked,
        codePortable: el.codePortableInput.checked,
      },
    });
    if (state.config.language !== LANG) applyLanguage(state.config.language);
    applyTheme(state.config.themePref);
    render();
    await restartIfWas(wasRunning, t(rolesChanged ? "settings.savedRoles" : "settings.saved"), "good");
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

function toggleLogs() {
  setLogs(!state.logsOpen);
}

function setLogs(open) {
  state.logsOpen = open;
  el.logPanel.classList.toggle("hidden", !open);
  el.logsBtn.setAttribute("aria-expanded", String(open));
  if (open) refreshLogs();
}

async function refreshLogs() {
  try {
    const text = await invoke("tail_log");
    const atBottom = el.logText.scrollTop + el.logText.clientHeight >= el.logText.scrollHeight - 20;
    el.logText.textContent = text || t("logs.empty");
    if (atBottom) el.logText.scrollTop = el.logText.scrollHeight;
  } catch {
    /* the log may not exist yet */
  }
}

async function copyText(text, message) {
  try {
    await invoke("copy_text", { text });
    toast(message, "good");
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// Close button: the first time, explain the tray and offer a real exit.
async function closeToTray() {
  if (localStorage.getItem("trayHintDone") !== "1") {
    const answer = await ask({
      title: t("dialog.trayTitle"),
      text: t("dialog.trayText"),
      ok: t("dialog.trayOk"),
      cancel: t("dialog.trayCancel"),
      alt: t("dialog.exit"),
      check: t("dialog.trayDontAsk"),
    });
    if (answer.checked) localStorage.setItem("trayHintDone", "1");
    if (answer.value === "alt") {
      invoke("app_exit").catch(() => {});
      return;
    }
    if (answer.value !== "ok") return;
  }
  invoke("win_close").catch(() => {});
}

// A modal question. Resolves with { value: "ok" | "cancel" | "alt", checked }.
// "?" next to a heading or a field (data-help="<topic>"): a plain-words explanation in a window.
// The texts live in i18n.js as help.<topic>.title / help.<topic>.body (paragraphs split by a blank line).
// `name` — what the "?" explains: a screen reader hears "<name>: what is this and why?", not
// a row of identical buttons. A language switch rebuilds the buttons (addHelpButtons, renders).
function helpButton(open, name = "") {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "help-btn";
  button.textContent = "?";
  const label = name ? `${name}: ${t("help.what")}` : t("help.what");
  button.setAttribute("aria-label", label);
  button.title = t("help.what");
  button.addEventListener("click", (event) => {
    // Inside a label or a summary the click would also toggle what it belongs to.
    event.preventDefault();
    event.stopPropagation();
    open();
  });
  return button;
}

function addHelpButtons(root = document) {
  for (const node of root.querySelectorAll("[data-help]")) {
    if (node.querySelector(":scope > .help-btn")) continue;
    // A language switch rewrites the heading's text and drops the button: applyLanguage adds it back.
    const topic = node.dataset.help;
    node.append(helpButton(() => openHelp(t(`help.${topic}.title`), t(`help.${topic}.body`)), t(`help.${topic}.title`)));
  }
}

function openHelp(title, body) {
  const dialog = $("helpDialog");
  $("helpTitle").textContent = title;
  const box = $("helpBody");
  box.replaceChildren(...body.split(/\n\s*\n/).map((part) => Object.assign(document.createElement("p"), { textContent: part.trim() })));
  dialog.showModal();
}

function ask({ title, text, ok, cancel, alt = null, danger = false, check = null }) {
  el.dialogTitle.textContent = title;
  el.dialogText.textContent = text;
  el.dialogOk.textContent = ok;
  el.dialogOk.className = danger ? "danger-solid" : "primary";
  el.dialogCancel.textContent = cancel;
  el.dialogAlt.textContent = alt ?? "";
  el.dialogAlt.classList.toggle("hidden", !alt);
  el.dialogCheckRow.classList.toggle("hidden", !check);
  el.dialogCheck.checked = false;
  el.dialogCheckText.textContent = check ?? "";
  return new Promise((resolve) => {
    el.dialog.addEventListener(
      "close",
      () => resolve({ value: el.dialog.returnValue || "cancel", checked: el.dialogCheck.checked }),
      { once: true },
    );
    el.dialog.returnValue = "";
    el.dialog.showModal();
    // Destructive answers are not the default focus.
    (danger ? el.dialogCancel : el.dialogOk).focus();
  });
}

let toastTimer = null;
function toast(message, kind = "") {
  el.toastText.textContent = message;
  el.toast.className = `toast show ${kind}`;
  el.toast.setAttribute("role", kind === "error" ? "alert" : "status");
  clearTimeout(toastTimer);
  // Errors stay until closed: they are often long and worth copying.
  if (kind !== "error") toastTimer = setTimeout(hideToast, 5000);
}

function hideToast() {
  clearTimeout(toastTimer);
  el.toast.className = "toast";
}

function formatError(exc) {
  if (typeof exc === "string") return exc;
  if (exc?.message) return exc.message;
  return String(exc);
}

document.addEventListener("DOMContentLoaded", boot);
