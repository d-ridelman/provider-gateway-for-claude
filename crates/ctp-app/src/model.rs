use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_PORT: u16 = 17899;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub providers: Vec<ProviderRow>,
    #[serde(default)]
    pub claude_path: Option<String>,
    /// Запускать русскую копию Claude (см. `ruui`); по умолчанию — оригинал.
    #[serde(default)]
    pub russian_ui: bool,
    /// Включить встроенный браузер Claude (`builtinBrowserEnabled` в 3p-конфиге):
    /// в третьестороннем режиме он по умолчанию выключен.
    #[serde(default)]
    pub builtin_browser: bool,
    /// Папка, в которой открывается Claude Code CLI; `None` — домашняя.
    #[serde(default)]
    pub code_dir: Option<String>,
    /// Язык интерфейса: `ru`, `en` или `auto`/нет — язык Windows.
    #[serde(default)]
    pub language: Option<String>,
    /// Тема окна: `light` или `dark`; нет — как в Windows.
    #[serde(default)]
    pub theme: Option<String>,
    /// Проверять обновления на GitHub при старте и раз в сутки.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Какую модель шлюза Claude Code считает основной («sonnet»).
    #[serde(default)]
    pub model_roles: ModelRoles,
    /// Кнопка «Claude Code» открывает портативный Claude Code из папки приложения
    /// (`installs::portable_code`) с его собственной папкой настроек.
    #[serde(default)]
    pub code_portable: bool,
    /// Прокси (ядро mihomo) и подписки.
    #[serde(default)]
    pub proxy: ProxySettings,
}

/// Режимы прокси: что идёт через ядро mihomo.
pub const PROXY_MODES: &[&str] = &["off", "app", "system", "tun"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    /// `off` (или пусто) — без прокси; `app` — шлюз, Claude из PGC и Claude Code;
    /// `system` — плюс системный прокси Windows; `tun` — весь трафик (нужен админ).
    #[serde(default)]
    pub mode: String,
    /// mixed-порт ядра на 127.0.0.1; `0` — выбрать свободный при первом запуске.
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

impl ProxySettings {
    pub fn is_on(&self) -> bool {
        PROXY_MODES[1..].contains(&self.mode.as_str())
    }
}

/// Подписка VPN. Адрес — секрет и лежит в Credential Manager (`store::sub_key`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub name: String,
    /// Как часто ядро обновляет подписку само.
    #[serde(default = "default_interval")]
    pub interval_hours: u32,
}

fn default_interval() -> u32 {
    12
}

/// Модель шлюза: провайдер и ID модели у него (не псевдоним — тот меняется вместе с
/// провайдером и правилами Claude).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// Роль модели в Claude Code: `sonnet` — основная (сессия и алиас `sonnet`); алиасы
/// `opus`/`haiku` шлюз тоже отдаёт ей. `None` — первая модель шлюза. Старые `opus` и
/// `haiku` из config.json (сильная и быстрая роли до 0.2.2) читаются и отбрасываются.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoles {
    #[serde(default)]
    pub sonnet: Option<ModelRef>,
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            token: String::new(),
            providers: Vec::new(),
            claude_path: None,
            russian_ui: false,
            builtin_browser: false,
            code_dir: None,
            language: None,
            theme: None,
            auto_update: true,
            model_roles: ModelRoles::default(),
            code_portable: false,
            proxy: ProxySettings::default(),
        }
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Потолок уровня рассуждения (`low…max`); `None` — уровни выключены.
    #[serde(default)]
    pub max_effort: Option<String>,
    /// Уровень, если Claude его не прислал.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Имя модели для меню Claude (ID без `[1m]` → «DeepSeek V4 Pro»), из каталога OpenCode.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

fn default_kind() -> String {
    "openai".to_string()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    pub port: u16,
    pub token: String,
    pub claude_path: Option<String>,
    pub detected_claude: Option<String>,
    pub providers: Vec<UiProvider>,
    pub russian_ui: bool,
    /// Включён ли встроенный браузер в 3p-профиле Claude.
    pub builtin_browser: bool,
    /// Есть ли файл перевода `claude-ru.json` в папке настроек.
    pub russian_file: bool,
    pub config_dir: String,
    pub code_dir: Option<String>,
    /// Язык окна сейчас: `ru` или `en`.
    pub language: String,
    /// Выбор пользователя: `auto`, `ru` или `en`.
    pub language_pref: String,
    /// Выбор темы: `auto`, `light` или `dark`.
    pub theme_pref: String,
    pub auto_update: bool,
    /// Версия этой сборки (`Cargo.toml`).
    pub version: String,
    pub model_roles: ModelRoles,
    pub code_portable: bool,
    /// Что скачано в папку приложения (портативные Claude Code и Claude Desktop).
    pub installed: crate::installs::Installed,
    /// Установленный Claude Code (PATH или `~/.local/bin`), если есть.
    pub detected_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiProvider {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub has_key: bool,
    pub needs_key: bool,
    pub aliases: Vec<AliasRow>,
    pub max_effort: Option<String>,
    pub default_effort: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasRow {
    pub model: String,
    pub alias: String,
    pub supports_1m: bool,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStatus {
    pub running: bool,
    pub port: u16,
    pub token: String,
    pub models: Vec<ModelInfo>,
    pub log_path: String,
    /// Самопроверка установленного Claude; `None` — ещё идёт.
    pub claude: Option<crate::upkeep::Check>,
    /// Доступная новая версия программы.
    pub update: Option<crate::updates::UpdateInfo>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub alias: String,
    pub model: String,
    pub provider: String,
    pub supports_1m: bool,
    /// Потолок уровня рассуждения провайдера; `None` — выбора уровня в Claude нет.
    pub max_effort: Option<String>,
    /// OpenAI-провайдер (иначе Anthropic: модель и так из семейства Claude).
    pub openai: bool,
    /// Имя для меню Claude; `None` — ID модели.
    pub label: Option<String>,
}

/// Итог «Открыть Claude»: текст и признак, что вышло не то, что просили.
#[derive(Clone, Debug, Serialize)]
pub struct LaunchResult {
    pub message: String,
    pub warning: bool,
}

/// Ответ «Загрузить модели»: ID от провайдера и подсказки каталога OpenCode.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedModels {
    pub ids: Vec<String>,
    pub catalog: crate::catalog::Hints,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderInput {
    pub provider: ProviderRow,
    pub original_id: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_key: bool,
}

/// Несохранённая форма провайдера для «Загрузить модели» и «Проверить».
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInput {
    pub provider: ProviderRow,
    pub original_id: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub port: u16,
    pub claude_path: Option<String>,
    #[serde(default)]
    pub russian_ui: bool,
    #[serde(default)]
    pub builtin_browser: bool,
    #[serde(default)]
    pub code_dir: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default)]
    pub model_roles: ModelRoles,
    #[serde(default)]
    pub code_portable: bool,
}
