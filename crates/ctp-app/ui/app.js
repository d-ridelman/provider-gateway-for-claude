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
    "russianFileHint", "claudeWarn", "claudeCheckList", "startBtn", "stopBtn", "launchBtn", "modelCount",
    "logPanel", "logText", "logsBtn", "toast", "toastText", "presetField", "presetInput", "fetchModelsBtn",
    "testBtn", "pickerBox", "pickerFilter", "pickerCount", "pickerList", "dialog", "dialogTitle",
    "dialogText", "dialogOk", "dialogCancel", "dialogAlt", "dialogCheckRow", "dialogCheck", "dialogCheckText",
    "updateBtn", "versionText", "autoUpdateInput", "checkUpdateBtn", "roleSonnet", "roleOpus", "roleHaiku",
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
  await refresh();
  autostart();
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
  fillPresets();
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

async function pollStatus() {
  try {
    state.status = await invoke("status");
    renderStatus();
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
  const settingsBtn = $("settingsBtn");
  if (settings) settingsBtn.setAttribute("aria-current", "page");
  else settingsBtn.removeAttribute("aria-current");
  el.settingsPanel.classList.toggle("hidden", !settings);
  el.form.classList.toggle("hidden", settings || !hasSelection());
  el.empty.classList.toggle("hidden", settings || hasSelection());
  if (settings) {
    renderSettings();
    return;
  }
  if (hasSelection()) renderForm();
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
    Number(el.portInput.value) !== c.port ||
    el.claudePathInput.value.trim() !== (c.claudePath ?? "") ||
    el.codeDirInput.value.trim() !== (c.codeDir ?? "") ||
    el.russianInput.checked !== !!c.russianUi ||
    el.autoUpdateInput.checked !== (c.autoUpdate !== false) ||
    ROLES.some(([role, id]) => el[id].value !== roleKey(c.modelRoles?.[role]))
  );
}

// Model roles of Claude Code: role, select id, text of the "not chosen" option.
const ROLES = [
  ["sonnet", "roleSonnet", "settings.roleFirst"],
  ["opus", "roleOpus", "settings.roleAsMain"],
  ["haiku", "roleHaiku", "settings.roleAsMain"],
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
  el.portInput.value = state.config.port;
  el.claudePathInput.value = state.config.claudePath ?? "";
  el.codeDirInput.value = state.config.codeDir ?? "";
  for (const [role, id, emptyText] of ROLES) fillRole(el[id], state.config.modelRoles?.[role], emptyText);
  el.tokenInput.value = state.config.token;
  el.detectedClaude.textContent = state.config.detectedClaude
    ? t("settings.found", { path: state.config.detectedClaude })
    : t("settings.notFound");
  el.russianInput.checked = !!state.config.russianUi;
  // Whether the translation is fine is said by the self-check below; here only "not yet".
  el.russianFileHint.textContent = state.config.russianFile ? "" : t("settings.russianFileMissing");
  el.russianFileHint.classList.toggle("hidden", !!state.config.russianFile);
  el.versionText.textContent = t("update.current", { version: state.config.version });
  el.autoUpdateInput.checked = state.config.autoUpdate !== false;
}

// Self-check of the installed Claude: the list in Settings and a badge in the title bar.
function renderClaudeCheck() {
  const check = state.status?.claude;
  const items = check?.items ?? [];
  // An optional feature that is missing is listed, but does not light the title-bar warning.
  el.claudeWarn.classList.toggle("hidden", !items.some((item) => !item.ok && !item.optional));
  el.claudeCheckList.replaceChildren();
  if (!check) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = t("settings.checking");
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
  $("copyTokenBtn").addEventListener("click", () => copyText(el.tokenInput.value, t("toast.tokenCopied")));
  $("copyEnvBtn").addEventListener("click", copyEnv);
  $("openLogsFolderBtn").addEventListener("click", () => invoke("open_logs").catch((e) => toast(formatError(e), "error")));
  el.updateBtn.addEventListener("click", installUpdate);
  el.checkUpdateBtn.addEventListener("click", checkUpdate);
  $("browseClaudeBtn").addEventListener("click", () => browse(el.claudePathInput));
  $("browseCodeBtn").addEventListener("click", () => browse(el.codeDirInput));

  // Enter outside a button would submit the dialog with its first button ("exit and stop
  // the gateway" in the tray dialog). Only a focused button answers with Enter.
  el.dialog.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && event.target.tagName !== "BUTTON") event.preventDefault();
  });

  $("toastClose").addEventListener("click", hideToast);
  $("toastCopy").addEventListener("click", () => copyText(el.toastText.textContent, t("toast.copied")));

  document.addEventListener("keydown", (event) => {
    if (el.dialog.open) return; // the dialog handles its own keys
    if (event.key === "Escape") {
      if (el.toast.classList.contains("show")) hideToast();
      else if (state.logsOpen) setLogs(false);
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

async function busy(button, label, work) {
  const text = button.textContent;
  button.disabled = true;
  button.setAttribute("aria-busy", "true");
  button.textContent = label;
  try {
    await work();
  } finally {
    button.disabled = false;
    button.removeAttribute("aria-busy");
    button.textContent = text;
  }
}

// Saving a provider stops the gateway (routes change); if it was running, start it again.
async function restartIfWas(wasRunning, done) {
  state.status = await invoke("status");
  if (!wasRunning) {
    toast(done, "good");
    return;
  }
  const started = await startGateway({ quiet: true });
  if (started) toast(t("toast.restarted", { done }), "good");
  else if (started === null) toast(done, "good"); // a later command overrode the restart
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
        toast(result.message, result.warning ? "error" : "good");
      } catch (exc) {
        toast(formatError(exc), "error");
      }
    });
  } finally {
    launching = false;
    renderStatus();
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
  // Only a new port stops the gateway; then bring it back if it was running.
  const portChanged = port !== state.config?.port;
  const wasRunning = portChanged && (starting > 0 || !!state.status?.running);
  try {
    state.config = await invoke("save_settings", {
      input: {
        port,
        claudePath: el.claudePathInput.value.trim() || null,
        codeDir: el.codeDirInput.value.trim() || null,
        modelRoles: Object.fromEntries(ROLES.map(([role, id]) => [role, readRole(el[id])])),
        russianUi: el.russianInput.checked,
        language: el.languageInput.value,
        autoUpdate: el.autoUpdateInput.checked,
      },
    });
    if (state.config.language !== LANG) applyLanguage(state.config.language);
    render();
    await restartIfWas(wasRunning, t("settings.saved"));
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

// The same variables as the Claude Code button, built in one place (Rust).
async function copyEnv() {
  if (!(state.status?.models ?? []).length) {
    toast(t("toast.needModels"), "error");
    return;
  }
  try {
    copyText(await invoke("code_command"), t("toast.envCopied"));
  } catch (exc) {
    toast(formatError(exc), "error");
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
