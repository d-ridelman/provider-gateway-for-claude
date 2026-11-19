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
    /// Папка, в которой открывается Claude Code CLI; `None` — домашняя.
    #[serde(default)]
    pub code_dir: Option<String>,
    /// Язык интерфейса: `ru`, `en` или `auto`/нет — язык Windows.
    #[serde(default)]
    pub language: Option<String>,
    /// Проверять обновления на GitHub при старте и раз в сутки.
    #[serde(default = "default_true")]
    pub auto_update: bool,
    /// Какие модели шлюза Claude Code считает «opus», «sonnet» и «haiku».
    #[serde(default)]
    pub model_roles: ModelRoles,
    /// Человек уже решал про советника Claude Code (галочка в настройках): после этого
    /// программа не включает его сама, даже если `advisorModel` в его настройках нет.
    #[serde(default)]
    pub advisor_decided: bool,
}

/// Модель шлюза: провайдер и ID модели у него (не псевдоним — тот меняется вместе с
/// провайдером и правилами Claude).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// Роли моделей в Claude Code. `sonnet` — основная (сессия и алиас `sonnet`), `opus` —
/// сильная (алиас `opus`: субагенты с `model: opus`), `haiku` — быстрая
/// (алиас `haiku` и фоновые мелочи). `None` — как раньше: первая модель шлюза.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoles {
    #[serde(default)]
    pub sonnet: Option<ModelRef>,
    #[serde(default)]
    pub opus: Option<ModelRef>,
    #[serde(default)]
    pub haiku: Option<ModelRef>,
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
            code_dir: None,
            language: None,
            auto_update: true,
            model_roles: ModelRoles::default(),
            advisor_decided: false,
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
    /// Есть ли файл перевода `claude-ru.json` в папке настроек.
    pub russian_file: bool,
    pub config_dir: String,
    pub code_dir: Option<String>,
    /// Язык окна сейчас: `ru` или `en`.
    pub language: String,
    /// Выбор пользователя: `auto`, `ru` или `en`.
    pub language_pref: String,
    pub auto_update: bool,
    /// Версия этой сборки (`Cargo.toml`).
    pub version: String,
    pub model_roles: ModelRoles,
    /// `advisorModel` из настроек Claude Code (`None` — советник выключен).
    pub advisor_model: Option<String>,
    pub advisor_decided: bool,
    /// Файл настроек Claude Code, куда пишется советник.
    pub claude_settings: String,
    /// Почему этот файл не годится для правки (битый JSON и т. п.); тогда галочки нет.
    pub claude_settings_error: Option<String>,
    /// Ответ на сохранение: что стало с советником (записан, выключен, не вышло).
    pub advisor_notice: Option<Notice>,
}

/// Сообщение для окна; `warning` — не вышло то, что просили.
#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub text: String,
    pub warning: bool,
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
    pub code_dir: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default)]
    pub model_roles: ModelRoles,
    /// Галочка советника, только если человек её трогал или она меняет файл; `None` —
    /// настройки Claude Code не трогать.
    #[serde(default)]
    pub advisor: Option<bool>,
}
