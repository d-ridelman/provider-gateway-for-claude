use crate::model::Config;
use crate::runtime::GatewayRuntime;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, AtomicU64};

pub struct AppShared {
    pub config_dir: PathBuf,
    pub config: Mutex<Config>,
    pub runtime: Mutex<Option<GatewayRuntime>>,
    /// Растёт при каждой остановке: запуск, который за время bind «перебили»
    /// остановкой или другим запуском, не регистрирует свой сервер.
    pub epoch: AtomicU64,
    /// Порт, который шлюз взял вместо занятого из настроек (0 — не брал): после перезапуска
    /// он пробуется первым, чтобы адрес в уже открытом Claude не менялся.
    pub fallback_port: AtomicU16,
    /// Последняя самопроверка установленного Claude (`upkeep::check`).
    pub claude: Mutex<Option<crate::upkeep::Check>>,
    /// Найденная более новая версия (`updates::spawn_auto_check` или «Проверить»).
    pub update: Mutex<Option<crate::updates::UpdateInfo>>,
}

impl AppShared {
    pub fn new(config_dir: PathBuf, config: Config) -> Self {
        Self {
            config_dir,
            config: Mutex::new(config),
            runtime: Mutex::new(None),
            epoch: AtomicU64::new(0),
            fallback_port: AtomicU16::new(0),
            claude: Mutex::new(None),
            update: Mutex::new(None),
        }
    }
}
