const invoke =
  window.__TAURI__?.core?.invoke ??
  window.__TAURI__?.invoke;

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

const draft = {
  id: "openai",
  name: "OpenAI",
  kind: "openai",
  baseUrl: "https://api.openai.com/v1",
  models: [],
  hasKey: false,
  needsKey: true,
  aliases: [],
};

// Пресеты из списка must-have провайдеров v1 (docs/rewrite-rust-tauri.md).
const PRESETS = [
  { label: "OpenAI", id: "openai", name: "OpenAI", kind: "openai", baseUrl: "https://api.openai.com/v1" },
  { label: "OpenRouter", id: "openrouter", name: "OpenRouter", kind: "openai", baseUrl: "https://openrouter.ai/api/v1" },
  { label: "OpenCode Go (подписка)", id: "opencode-go", name: "OpenCode Go", kind: "openai", baseUrl: "https://opencode.ai/inference/go/openai/v1" },
  { label: "OpenCode Go — Claude, Qwen, MiniMax (Anthropic API)", id: "opencode-go-anthropic", name: "OpenCode Go (Anthropic)", kind: "anthropic", baseUrl: "https://opencode.ai/inference/go/anthropic" },
  { label: "OpenCode Zen (оплата по факту)", id: "opencode", name: "OpenCode Zen", kind: "openai", baseUrl: "https://opencode.ai/inference/openai/v1" },
  { label: "DeepSeek", id: "deepseek", name: "DeepSeek", kind: "openai", baseUrl: "https://api.deepseek.com/v1" },
  { label: "Google Gemini", id: "gemini", name: "Gemini", kind: "openai", baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai" },
  { label: "Anthropic API", id: "anthropic", name: "Anthropic", kind: "anthropic", baseUrl: "https://api.anthropic.com" },
  { label: "Ollama (этот компьютер)", id: "ollama", name: "Ollama", kind: "openai", baseUrl: "http://127.0.0.1:11434/v1" },
  { label: "llama.cpp server (этот компьютер)", id: "llamacpp", name: "llama.cpp", kind: "openai", baseUrl: "http://127.0.0.1:8080/v1" },
  { label: "LM Studio (этот компьютер)", id: "lmstudio", name: "LM Studio", kind: "openai", baseUrl: "http://127.0.0.1:1234/v1" },
];

const $ = (id) => document.getElementById(id);
const el = {
  statusDot: $("statusDot"),
  statusText: $("statusText"),
  portLabel: $("portLabel"),
  providerList: $("providerList"),
  empty: $("empty"),
  form: $("providerForm"),
  formTitle: $("formTitle"),
  formSubtitle: $("formSubtitle"),
  formKindBadge: $("formKindBadge"),
  formKeyBadge: $("formKeyBadge"),
  idInput: $("idInput"),
  nameInput: $("nameInput"),
  kindInput: $("kindInput"),
  baseInput: $("baseInput"),
  keyInput: $("keyInput"),
  keyHint: $("keyHint"),
  clearKeyBtn: $("clearKeyBtn"),
  modelsInput: $("modelsInput"),
  maxEffortInput: $("maxEffortInput"),
  defaultEffortInput: $("defaultEffortInput"),
  aliasBox: $("aliasBox"),
  aliasList: $("aliasList"),
  deleteBtn: $("deleteBtn"),
  formNotice: $("formNotice"),
  settingsPanel: $("settingsPanel"),
  portInput: $("portInput"),
  claudePathInput: $("claudePathInput"),
  codeDirInput: $("codeDirInput"),
  launchCodeBtn: $("launchCodeBtn"),
  detectedClaude: $("detectedClaude"),
  tokenInput: $("tokenInput"),
  russianInput: $("russianInput"),
  russianFileHint: $("russianFileHint"),
  claudeWarn: $("claudeWarn"),
  claudeCheckList: $("claudeCheckList"),
  startBtn: $("startBtn"),
  stopBtn: $("stopBtn"),
  launchBtn: $("launchBtn"),
  modelCount: $("modelCount"),
  logPanel: $("logPanel"),
  logText: $("logText"),
  toast: $("toast"),
  presetField: $("presetField"),
  presetInput: $("presetInput"),
  fetchModelsBtn: $("fetchModelsBtn"),
  testBtn: $("testBtn"),
  pickerBox: $("pickerBox"),
  pickerFilter: $("pickerFilter"),
  pickerCount: $("pickerCount"),
  pickerList: $("pickerList"),
};

// Список моделей, полученный с провайдера; живёт, пока открыт тот же провайдер.
let picker = [];
// Подсказки каталога OpenCode к последнему списку моделей: { source, models: { id: hint } }.
let catalog = { source: null, models: {} };
// Имена моделей для меню Claude (ID → «DeepSeek V4 Pro») в открытой форме.
let labels = {};
// Растёт при смене открытого провайдера: поздний ответ «Загрузить» не попадёт в чужую форму.
let formEpoch = 0;

function boot() {
  if (!invoke) {
    toast("Tauri API не найден. Запустите приложение через ctp-app.exe.", "error");
    return;
  }
  for (const [index, preset] of PRESETS.entries()) {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = preset.label;
    el.presetInput.append(option);
  }
  bind();
  refresh().then(autostart);
  setInterval(pollStatus, 3000);
  setInterval(() => {
    if (state.logsOpen) refreshLogs();
  }, 2500);
}

async function refresh() {
  try {
    const [config, status] = await Promise.all([
      invoke("get_config"),
      invoke("status"),
    ]);
    state.config = config;
    state.status = status;
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

// Окно открывают, чтобы пользоваться шлюзом: поднимаем его сразу, если есть что раздавать.
async function autostart() {
  if (state.config?.providers?.length && !state.status?.running) {
    await startGateway({ quiet: true });
  }
}

async function pollStatus() {
  try {
    const status = await invoke("status");
    state.status = status;
    renderStatus();
  } catch {
    /* приложение закрывается — молчим */
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
  el.statusText.textContent = running ? "Работает" : "Остановлен";
  el.portLabel.textContent = `127.0.0.1:${status?.port ?? state.config?.port ?? "—"}`;
  el.startBtn.textContent = running ? "Перезапустить" : "Запустить шлюз";
  el.stopBtn.disabled = !running;
  el.launchBtn.disabled = launching || !running || !status?.models?.length;
  el.launchCodeBtn.disabled = !running || !status?.models?.length;
  el.modelCount.textContent = modelWord(status?.models?.length ?? 0);
  renderClaudeCheck();
}

function renderProviders() {
  el.providerList.replaceChildren();
  const providers = state.config?.providers ?? [];
  if (!providers.length) {
    const empty = document.createElement("div");
    empty.className = "muted small";
    empty.style.padding = "8px 6px";
    empty.textContent = "Нет провайдеров";
    el.providerList.append(empty);
    return;
  }
  for (const provider of providers) {
    const item = document.createElement("div");
    item.className = "provider-item";
    if (!state.isNew && provider.id === state.selectedId) item.classList.add("active");
    item.addEventListener("click", () => selectProvider(provider.id));

    const row = document.createElement("div");
    row.className = "row";
    const name = document.createElement("b");
    name.textContent = provider.name || provider.id;
    const kind = document.createElement("span");
    kind.className = "badge";
    kind.textContent = provider.kind;
    row.append(name, kind);

    const url = document.createElement("div");
    url.className = "url";
    url.textContent = provider.baseUrl;

    const meta = document.createElement("div");
    meta.className = "meta";
    const key = provider.hasKey ? "ключ есть" : provider.needsKey ? "ключа нет" : "без ключа";
    meta.textContent = `${modelWord(provider.models.length)} · ${key}`;

    item.append(row, url, meta);
    el.providerList.append(item);
  }
}

function renderMain() {
  const settings = state.view === "settings";
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

function selectProvider(id) {
  state.view = "provider";
  state.selectedId = id;
  state.isNew = false;
  state.originalId = id;
  state.clearKey = false;
  picker = [];
  formEpoch += 1;
  render();
}

function newProvider() {
  state.view = "provider";
  state.selectedId = "__new__";
  state.isNew = true;
  state.originalId = null;
  state.clearKey = false;
  picker = [];
  formEpoch += 1;
  render();
}

function renderForm() {
  const provider = currentProvider();
  if (!provider) return;
  el.formTitle.textContent = state.isNew ? "Новый провайдер" : provider.name || provider.id;
  el.formSubtitle.textContent = state.isNew
    ? "Ключ хранится в Windows Credential Manager, в JSON не пишется"
    : provider.baseUrl;
  el.formKindBadge.textContent = provider.kind;
  el.formKindBadge.className = `badge ${provider.kind === "anthropic" ? "warn" : ""}`;
  el.formKeyBadge.textContent = provider.hasKey ? "ключ сохранён" : provider.needsKey ? "ключ не задан" : "ключ не нужен";
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
  el.keyInput.placeholder = provider.hasKey ? "•••••••• (сохранён, оставьте пустым)" : "вставьте ключ";
  el.clearKeyBtn.classList.toggle("hidden", !provider.hasKey || state.isNew);
  el.deleteBtn.classList.toggle("hidden", state.isNew);
  el.keyHint.textContent = provider.needsKey
    ? provider.hasKey
      ? "Ключ сохранён. Чтобы заменить — введите новый."
      : "Ключ не задан: провайдер может ответить 401."
    : "Для localhost ключ можно не заполнять.";
  if (state.clearKey) el.keyHint.textContent = "Ключ будет удалён при сохранении.";

  el.aliasBox.classList.toggle("hidden", !provider.aliases?.length);
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
  el.formNotice.textContent = provider.aliases?.length
    ? `${modelWord(provider.aliases.length)} · псевдонимы готовы`
    : "Псевдонимы появятся после сохранения";
}

function renderSettings() {
  if (!state.config) return;
  el.portInput.value = state.config.port;
  el.claudePathInput.value = state.config.claudePath ?? "";
  el.codeDirInput.value = state.config.codeDir ?? "";
  el.tokenInput.value = state.config.token;
  el.detectedClaude.textContent = state.config.detectedClaude
    ? `найден: ${state.config.detectedClaude}`
    : "Claude Desktop не найден — укажите путь вручную";
  el.russianInput.checked = !!state.config.russianUi;
  el.russianFileHint.textContent = state.config.russianFile
    ? "Файл перевода claude-ru.json найден; при обновлении Claude он скачивается заново."
    : "Перевод claude.ai скачается сам при следующем старте или «Открыть Claude» (нужен интернет).";
}

// Самопроверка установленного Claude: список в настройках и пометка в шапке.
function renderClaudeCheck() {
  const check = state.status?.claude;
  const items = check?.items ?? [];
  el.claudeWarn.classList.toggle("hidden", !items.some((item) => !item.ok));
  el.claudeCheckList.replaceChildren();
  if (!check) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = "Проверяется…";
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
    li.className = item.ok ? "ok" : "bad";
    li.textContent = `${item.ok ? "✓" : "✕"} ${item.text}`;
    el.claudeCheckList.append(li);
  }
}

function bind() {
  $("winMin").addEventListener("click", () => invoke("win_minimize").catch(() => {}));
  $("winMax").addEventListener("click", () => invoke("win_toggle_maximize").catch(() => {}));
  $("winClose").addEventListener("click", () => invoke("win_close").catch(() => {}));

  el.claudeWarn.addEventListener("click", () => {
    state.view = "settings";
    render();
  });
  $("addBtn").addEventListener("click", newProvider);
  $("emptyAdd").addEventListener("click", newProvider);
  $("settingsBtn").addEventListener("click", () => {
    state.view = "settings";
    render();
  });
  $("closeSettings").addEventListener("click", () => {
    state.view = "provider";
    render();
  });

  el.form.addEventListener("submit", saveProvider);
  el.presetInput.addEventListener("change", applyPreset);
  el.maxEffortInput.addEventListener("change", syncEffort);
  el.fetchModelsBtn.addEventListener("click", fetchModels);
  el.testBtn.addEventListener("click", testProvider);
  el.pickerFilter.addEventListener("input", renderPicker);
  el.modelsInput.addEventListener("input", renderPicker);
  // Адрес или тип поменяли во время «Загрузить модели» — ответ уже про другое.
  for (const input of [el.baseInput, el.kindInput]) {
    input.addEventListener("input", () => {
      formEpoch += 1;
    });
  }
  $("pickerClose").addEventListener("click", () => {
    picker = [];
    renderPicker();
  });
  $("deleteBtn").addEventListener("click", deleteProvider);
  el.clearKeyBtn.addEventListener("click", () => {
    state.clearKey = true;
    el.keyInput.value = "";
    el.keyHint.textContent = "Ключ будет удалён при сохранении.";
    el.clearKeyBtn.classList.add("hidden");
  });
  el.keyInput.addEventListener("input", () => {
    if (el.keyInput.value.trim()) state.clearKey = false;
  });

  el.startBtn.addEventListener("click", startGateway);
  el.stopBtn.addEventListener("click", stopGateway);
  el.launchBtn.addEventListener("click", launchClaude);
  el.launchCodeBtn.addEventListener("click", async () => {
    await busy(el.launchCodeBtn, "Открываю…", async () => {
      try {
        toast(await invoke("launch_claude_code"), "good");
      } catch (exc) {
        toast(formatError(exc), "error");
      }
    });
    renderStatus(); // busy включает кнопку обратно — вернуть ей состояние шлюза
  });

  $("logsBtn").addEventListener("click", toggleLogs);
  $("refreshLogsBtn").addEventListener("click", refreshLogs);
  $("closeLogsBtn").addEventListener("click", () => {
    state.logsOpen = false;
    el.logPanel.classList.add("hidden");
  });

  $("saveSettingsBtn").addEventListener("click", saveSettings);
  $("copyTokenBtn").addEventListener("click", () => copyText(el.tokenInput.value, "Токен скопирован"));
  $("copyEnvBtn").addEventListener("click", copyEnv);
  $("openLogsFolderBtn").addEventListener("click", () => invoke("open_logs").catch((e) => toast(formatError(e), "error")));
}

// «По умолчанию» без потолка шлюз игнорирует — не даём выбрать его впустую.
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

// Ключ для проверок: из поля; null — сохранённый; "" — без ключа («Удалить ключ»).
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
  formEpoch += 1; // ответ «Загрузить модели» для прежнего адреса уже не нужен
  renderPicker();
}

// Подсказка каталога к ID; hasOwn — чтобы модель «constructor» не нашла Object.
function hintFor(id) {
  return Object.hasOwn(catalog.models ?? {}, id) ? catalog.models[id] : undefined;
}

async function fetchModels() {
  const form = formEpoch;
  await busy(el.fetchModelsBtn, "Загружаю…", async () => {
    try {
      const fetched = await invoke("fetch_models", { input: probeInput() });
      if (form !== formEpoch) return; // пока ждали, открыли другого провайдера
      picker = fetched.ids;
      catalog = fetched.catalog ?? { source: null, models: {} };
      // Уже выбранным моделям без имени — имя из каталога (сохранится вместе с формой).
      for (const id of formModels().map(bareId)) {
        const name = hintFor(id)?.name;
        if (name && !Object.hasOwn(labels, id)) labels[id] = name;
      }
      el.pickerFilter.value = "";
      renderPicker();
      if (picker.length) el.pickerBox.scrollIntoView({ block: "nearest", behavior: "smooth" });
      if (!picker.length) toast("Провайдер вернул пустой список моделей", "error");
    } catch (exc) {
      toast(`Список моделей: ${formatError(exc)}`, "error");
    }
  });
}

function renderPicker() {
  el.pickerBox.classList.toggle("hidden", !picker.length);
  if (!picker.length) return;
  const chosen = new Set(formModels().map(bareId));
  const filter = el.pickerFilter.value.trim().toLowerCase();
  const shown = picker.filter(
    (id) => !filter || `${id} ${hintFor(id)?.name ?? ""}`.toLowerCase().includes(filter),
  );
  const source = catalog.source ? ` · каталог OpenCode: ${catalog.source}` : "";
  el.pickerCount.textContent = `${shown.length} из ${picker.length} · выбрано ${chosen.size}${source}`;
  el.pickerList.replaceChildren();
  for (const id of shown) {
    const hint = hintFor(id);
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = `chip ${chosen.has(id) ? "on" : ""}`;
    chip.textContent = hint?.context >= 1_000_000 ? `${id} · 1M` : id;
    chip.title = [
      hint?.name,
      hint?.context ? `контекст ${Math.round(hint.context / 1000)}K` : "",
      hint?.efforts?.length ? `уровни: ${hint.efforts.join(", ")}` : "",
      chosen.has(id) ? "Убрать из списка" : "Добавить в список",
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
    // Из каталога: 1M контекста, имя для меню Claude и уровни, если они ещё не заданы.
    models.push(hint?.context >= 1_000_000 ? `${id}[1m]` : id);
    if (hint?.name) labels[id] = hint.name;
    if (hint?.efforts?.length && !el.maxEffortInput.value) {
      el.maxEffortInput.value = hint.efforts[hint.efforts.length - 1];
      syncEffort();
      el.defaultEffortInput.value = hint.defaultEffort ?? "";
      toast(`Уровни рассуждения из каталога: до ${el.maxEffortInput.value} (для всего провайдера)`);
    }
  }
  el.modelsInput.value = models.join("\n");
  renderPicker();
}

async function testProvider() {
  await busy(el.testBtn, "Проверяю…", async () => {
    try {
      toast(await invoke("test_provider", { input: probeInput() }), "good");
    } catch (exc) {
      toast(`Проверка: ${formatError(exc)}`, "error");
    }
  });
}

async function busy(button, label, work) {
  const text = button.textContent;
  button.disabled = true;
  button.textContent = label;
  try {
    await work();
  } finally {
    button.disabled = false;
    button.textContent = text;
  }
}

// Сохранение останавливает шлюз (меняются маршруты); если он работал — поднимаем снова.
async function restartIfWas(wasRunning, done) {
  state.status = await invoke("status");
  if (!wasRunning) {
    toast(done, "good");
    return;
  }
  const started = await startGateway({ quiet: true });
  if (started) {
    toast(`${done}. Шлюз перезапущен; уже открытый Claude увидит новые модели после перезапуска.`, "good");
  } else if (started === null) {
    toast(done, "good"); // перезапуск перебила более поздняя команда — её итог виден в статусе
  }
}

let saving = false;

async function saveProvider(event) {
  event.preventDefault();
  if (saving) return; // двойной клик ушёл бы со старым originalId
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
  const input = {
    provider,
    originalId: state.originalId,
    apiKey: apiKey || null,
    clearKey: state.clearKey,
  };
  const wasRunning = (starting > 0 || !!state.status?.running);
  try {
    state.config = await invoke("save_provider", { input });
    state.selectedId = provider.id.toLowerCase();
    state.originalId = state.selectedId;
    state.isNew = false;
    state.clearKey = false;
    picker = [];
    render();
    await restartIfWas(wasRunning, "Провайдер сохранён");
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

async function deleteProvider() {
  const provider = currentProvider();
  if (!provider || state.isNew) return;
  if (!confirm(`Удалить провайдера «${provider.name || provider.id}»?`)) return;
  const wasRunning = (starting > 0 || !!state.status?.running) && state.config.providers.length > 1;
  try {
    state.config = await invoke("delete_provider", { id: provider.id });
    state.selectedId = state.config.providers[0]?.id ?? null;
    state.originalId = state.selectedId;
    state.isNew = false;
    state.clearKey = false; // иначе «удалить ключ» удалённого провайдера сработает на соседе
    picker = [];
    formEpoch += 1;
    render();
    await restartIfWas(wasRunning, "Провайдер удалён");
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// Сколько запусков идёт: сохранение в это время тоже должно поднять шлюз после себя.
let starting = 0;

// true — работает; false — ошибка (показана); null — запуск перебила более поздняя команда.
async function startGateway({ quiet = false } = {}) {
  starting += 1;
  try {
    state.status = await invoke("start_gateway");
    renderStatus();
    if (!state.status.running) {
      if (!quiet) toast("Запуск отменён более поздней командой");
      return null;
    }
    if (!quiet) toast(`Шлюз слушает 127.0.0.1:${state.status.port}`, "good");
    return true;
  } catch (exc) {
    state.status = await invoke("status").catch(() => state.status);
    renderStatus();
    toast(`Шлюз не запущен: ${formatError(exc)}`, "error");
    return false;
  } finally {
    starting -= 1;
  }
}

async function stopGateway() {
  try {
    state.status = await invoke("stop_gateway");
    renderStatus();
    toast("Шлюз остановлен");
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

// Идёт запуск Claude: опрос статуса не должен снова включать кнопку (второй клик
// начал бы вторую сборку русской копии).
let launching = false;

async function launchClaude() {
  if (launching) return;
  launching = true;
  // Первая сборка русской копии занимает время — кнопка показывает, что работа идёт.
  const label = state.config?.russianUi ? "Готовлю копию…" : "Запускаю…";
  try {
    await busy(el.launchBtn, label, async () => {
      try {
        const message = await invoke("launch_claude");
        toast(message, message.includes("не применён") ? "error" : "good");
      } catch (exc) {
        toast(formatError(exc), "error");
      }
    });
  } finally {
    launching = false;
    renderStatus();
  }
}

async function saveSettings() {
  const port = Number(el.portInput.value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    toast("Порт должен быть числом от 1 до 65535", "error");
    return;
  }
  const wasRunning = (starting > 0 || !!state.status?.running);
  try {
    state.config = await invoke("save_settings", {
      input: {
        port,
        claudePath: el.claudePathInput.value.trim() || null,
        codeDir: el.codeDirInput.value.trim() || null,
        russianUi: el.russianInput.checked,
      },
    });
    renderSettings();
    await restartIfWas(wasRunning, "Настройки сохранены");
    renderStatus();
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

function toggleLogs() {
  state.logsOpen = !state.logsOpen;
  el.logPanel.classList.toggle("hidden", !state.logsOpen);
  if (state.logsOpen) refreshLogs();
}

async function refreshLogs() {
  try {
    const text = await invoke("tail_log");
    const atBottom = el.logText.scrollTop + el.logText.clientHeight >= el.logText.scrollHeight - 20;
    el.logText.textContent = text || "Логов пока нет";
    if (atBottom) el.logText.scrollTop = el.logText.scrollHeight;
  } catch {
    /* журнал может ещё не создаться */
  }
}

function copyEnv() {
  const models = state.status?.models ?? [];
  if (!models.length) {
    toast("Сначала запустите шлюз с моделями", "error");
    return;
  }
  const port = state.status?.port ?? state.config?.port;
  const token = state.status?.token ?? state.config?.token;
  const alias = models[0].alias;
  const text = [
    `$env:ANTHROPIC_BASE_URL="http://127.0.0.1:${port}"`,
    `$env:ANTHROPIC_AUTH_TOKEN="${token}"`,
    `$env:ANTHROPIC_MODEL="${alias}"`,
    `$env:ANTHROPIC_DEFAULT_HAIKU_MODEL="${alias}"`,
    `$env:ANTHROPIC_SMALL_FAST_MODEL="${alias}"`,
    "claude",
  ].join("\n");
  copyText(text, "Команда для Claude Code скопирована");
}

async function copyText(text, message) {
  try {
    await invoke("copy_text", { text });
    toast(message, "good");
  } catch (exc) {
    toast(formatError(exc), "error");
  }
}

let toastTimer = null;
function toast(message, kind = "") {
  el.toast.textContent = message;
  el.toast.className = `toast show ${kind}`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    el.toast.className = "toast";
  }, 4800);
}

function formatError(exc) {
  if (typeof exc === "string") return exc;
  if (exc?.message) return exc.message;
  return String(exc);
}

function modelWord(count) {
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return `${count} модель`;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return `${count} модели`;
  return `${count} моделей`;
}

document.addEventListener("DOMContentLoaded", boot);
