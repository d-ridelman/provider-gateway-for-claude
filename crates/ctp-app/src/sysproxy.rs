//! Системный прокси Windows (режим «Системный прокси»): браузеры и программы, которые
//! берут прокси из настроек Windows, идут через ядро mihomo.
//!
//! Через WinINet (`INTERNET_OPTION_PER_CONNECTION_OPTION`), а не прямой записью в реестр:
//! на доменных ПК обычно стоит PAC-сценарий или автообнаружение, они важнее ручного
//! прокси, и одна запись `ProxyEnable` тихо не сработала бы. Прежние флаги, сервер, обход и
//! адрес PAC сохраняются в файл до включения и возвращаются при выключении, выходе и
//! следующем старте (если PGC упал, браузеры до этого смотрят в неработающий порт).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// Что показать в окне.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Системный прокси сейчас наш.
    pub on: bool,
    /// Почему не включился или почему перестал быть нашим (политика, ошибка).
    pub error: Option<String>,
}

/// Настройки прокси Windows до нас.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub flags: u32,
    pub server: Option<String>,
    pub bypass: Option<String>,
    pub autoconfig: Option<String>,
}

static STATE: Mutex<Option<(u16, Status)>> = Mutex::new(None);

fn backup() -> Option<PathBuf> {
    Some(crate::mihomo::dir()?.join("sysproxy-backup.json"))
}

/// Обход: локальные имена, сам компьютер, частные сети и прежний список человека.
pub fn bypass_list(old: Option<&str>) -> String {
    let mut parts: Vec<String> = [
        "<local>",
        "localhost",
        "127.*",
        "10.*",
        "192.168.*",
        "169.254.*",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    parts.extend((16..=31).map(|n| format!("172.{n}.*")));
    for item in old.unwrap_or_default().split(';') {
        let item = item.trim();
        if !item.is_empty() && !parts.iter().any(|p| p.eq_ignore_ascii_case(item)) {
            parts.push(item.to_string());
        }
    }
    parts.join(";")
}

/// Запрещает ли групповая политика менять прокси этому пользователю.
fn policy_block() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
        let machine = LOCAL_MACHINE
            .open(r"SOFTWARE\Policies\Microsoft\Windows\CurrentVersion\Internet Settings")
            .and_then(|k| k.get_u32("ProxySettingsPerUser"))
            .is_ok_and(|v| v == 0);
        let user = CURRENT_USER
            .open(r"Software\Policies\Microsoft\Internet Explorer\Control Panel")
            .and_then(|k| k.get_u32("Proxy"))
            .is_ok_and(|v| v == 1);
        if machine || user {
            return Some(crate::tr!(
                "настройки прокси Windows закреплены групповой политикой — системный прокси здесь не включить; режим «Только Claude и провайдеры» работает",
                "Windows proxy settings are locked by a group policy, so the system proxy cannot be set here; the Claude and providers only mode works"
            ));
        }
    }
    None
}

/// Включает системный прокси на `127.0.0.1:port`.
pub fn enable(port: u16) -> Result<()> {
    let result = (|| {
        if let Some(why) = policy_block() {
            bail!("{why}");
        }
        let file = backup().context("no AppData\\Local")?;
        // Снимок — только если его ещё нет: иначе после сбоя мы сохранили бы свой же прокси.
        if !file.is_file() {
            let saved = win::query()?;
            std::fs::create_dir_all(file.parent().context("backup folder")?)?;
            std::fs::write(&file, serde_json::to_vec_pretty(&saved)?)?;
        }
        let saved: Saved = serde_json::from_slice(&std::fs::read(&file)?)?;
        let ours = format!("127.0.0.1:{port}");
        // Что поставили мы: возврат при выключении — только если прокси всё ещё наш.
        std::fs::write(file.with_file_name("sysproxy-ours.txt"), &ours)?;
        win::set(&Saved {
            flags: win::DIRECT | win::PROXY,
            server: Some(ours),
            bypass: Some(bypass_list(saved.bypass.as_deref())),
            autoconfig: None,
        })
    })();
    let status = Status {
        on: result.is_ok(),
        error: result.as_ref().err().map(|e| format!("{e:#}")),
    };
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some((port, status));
    result
}

/// Возвращать ли снимок. Прокси с тех пор сменили (человек, политика, другой VPN) — их выбор
/// не затираем. Не знаем, что стоит сейчас (запрос не прошёл, файла «нашего» нет — его писала
/// только эта версия), — возвращаем: потерять снимок человека хуже.
fn should_restore(ours: &str, now: Option<&Saved>) -> bool {
    let ours = ours.trim();
    ours.is_empty()
        || now.is_none_or(|now| now.flags & win::PROXY != 0 && now.server.as_deref() == Some(ours))
}

/// Возвращает настройки, что были до нас (если мы их меняли).
pub fn disable() -> Result<()> {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let Some(file) = backup().filter(|f| f.is_file()) else {
        return Ok(());
    };
    let ours_file = file.with_file_name("sysproxy-ours.txt");
    let ours = std::fs::read_to_string(&ours_file).unwrap_or_default();
    if should_restore(&ours, win::query().ok().as_ref()) {
        let saved: Saved = serde_json::from_slice(&std::fs::read(&file)?)?;
        win::set(&saved)?;
        tracing::info!("system proxy restored");
    } else {
        tracing::info!("system proxy changed since; left as it is");
    }
    std::fs::remove_file(&file)?;
    let _ = std::fs::remove_file(&ours_file);
    Ok(())
}

/// Для окна; заодно ловит, что политика или другая программа сменила прокси за нас.
pub fn status() -> Option<Status> {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (port, status) = guard.as_mut()?;
    if status.on {
        let ours = format!("127.0.0.1:{port}");
        let now = win::query().ok();
        let still =
            now.is_some_and(|s| s.flags & win::PROXY != 0 && s.server.as_deref() == Some(&ours));
        if !still {
            status.on = false;
            status.error = Some(crate::tr!(
                "системный прокси сменили извне (групповая политика или другая программа)",
                "the system proxy was changed from outside (a group policy or another program)"
            ));
        }
    }
    Some(status.clone())
}

#[cfg(windows)]
mod win {
    use super::Saved;
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Networking::WinInet::{
        INTERNET_OPTION_PER_CONNECTION_OPTION, INTERNET_OPTION_REFRESH,
        INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_PER_CONN_AUTOCONFIG_URL,
        INTERNET_PER_CONN_OPTION_LISTW, INTERNET_PER_CONN_OPTIONW, INTERNET_PER_CONN_PROXY_BYPASS,
        INTERNET_PER_CONN_PROXY_SERVER, InternetQueryOptionW, InternetSetOptionW,
    };
    // Флаги в том виде, как их видит панель «Прокси» (с галочкой автоопределения): простой
    // INTERNET_PER_CONN_FLAGS её не отдаёт, и после возврата она пропадала бы.
    use windows_sys::Win32::Networking::WinInet::INTERNET_PER_CONN_FLAGS_UI as INTERNET_PER_CONN_FLAGS;
    pub use windows_sys::Win32::Networking::WinInet::{
        PROXY_TYPE_DIRECT as DIRECT, PROXY_TYPE_PROXY as PROXY,
    };

    const OPTIONS: [u32; 4] = [
        INTERNET_PER_CONN_FLAGS,
        INTERNET_PER_CONN_PROXY_SERVER,
        INTERNET_PER_CONN_PROXY_BYPASS,
        INTERNET_PER_CONN_AUTOCONFIG_URL,
    ];

    fn list(options: &mut [INTERNET_PER_CONN_OPTIONW]) -> INTERNET_PER_CONN_OPTION_LISTW {
        INTERNET_PER_CONN_OPTION_LISTW {
            dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
            pszConnection: std::ptr::null_mut(), // подключение по локальной сети
            dwOptionCount: options.len() as u32,
            dwOptionError: 0,
            pOptions: options.as_mut_ptr(),
        }
    }

    pub fn query() -> Result<Saved> {
        let mut options: Vec<INTERNET_PER_CONN_OPTIONW> = OPTIONS
            .iter()
            .map(|&o| INTERNET_PER_CONN_OPTIONW {
                dwOption: o,
                ..Default::default()
            })
            .collect();
        let mut request = list(&mut options);
        let mut size = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
        // SAFETY: список и его массив живы до конца функции; строки, что вернёт WinINet,
        // читаются и освобождаются GlobalFree ниже.
        unsafe {
            if InternetQueryOptionW(
                std::ptr::null(),
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                (&raw mut request).cast(),
                &mut size,
            ) == 0
            {
                bail!("InternetQueryOption: {}", std::io::Error::last_os_error());
            }
            let take = |o: &INTERNET_PER_CONN_OPTIONW| {
                let ptr = o.Value.pszValue;
                if ptr.is_null() {
                    return None;
                }
                let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                GlobalFree(ptr.cast());
                (!text.is_empty()).then_some(text)
            };
            Ok(Saved {
                flags: options[0].Value.dwValue,
                server: take(&options[1]),
                bypass: take(&options[2]),
                autoconfig: take(&options[3]),
            })
        }
    }

    fn wide(text: &Option<String>) -> Vec<u16> {
        text.as_deref()
            .unwrap_or_default()
            .encode_utf16()
            .chain([0])
            .collect()
    }

    pub fn set(saved: &Saved) -> Result<()> {
        let (mut server, mut bypass, mut auto) = (
            wide(&saved.server),
            wide(&saved.bypass),
            wide(&saved.autoconfig),
        );
        let mut options = [
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_FLAGS,
                ..Default::default()
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_SERVER,
                ..Default::default()
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_BYPASS,
                ..Default::default()
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_AUTOCONFIG_URL,
                ..Default::default()
            },
        ];
        options[0].Value.dwValue = saved.flags;
        options[1].Value.pszValue = server.as_mut_ptr();
        options[2].Value.pszValue = bypass.as_mut_ptr();
        options[3].Value.pszValue = auto.as_mut_ptr();
        let request = list(&mut options);
        // SAFETY: строки и список живут до конца вызова; WinINet их копирует.
        unsafe {
            if InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                (&raw const request).cast(),
                size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
            ) == 0
            {
                bail!("InternetSetOption: {}", std::io::Error::last_os_error());
            }
            // Программам сказать, что настройки изменились.
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null(),
                0,
            );
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod win {
    use super::Saved;
    use anyhow::{Result, bail};
    pub const DIRECT: u32 = 1;
    pub const PROXY: u32 = 2;
    pub fn query() -> Result<Saved> {
        bail!("Windows only")
    }
    pub fn set(_: &Saved) -> Result<()> {
        bail!("Windows only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_keeps_the_old_list_and_private_networks() {
        let list = bypass_list(Some("*.corp.example; 10.*;intranet"));
        assert!(list.starts_with("<local>;localhost;127.*;10.*;192.168.*"));
        assert!(
            list.contains("172.16.*") && list.contains("172.31.*") && !list.contains("172.32.*")
        );
        assert!(list.ends_with(";*.corp.example;intranet"), "{list}");
        assert_eq!(list.matches("10.*").count(), 1);
        assert!(!bypass_list(None).contains(";;"));
    }

    #[test]
    fn the_snapshot_comes_back_unless_someone_else_set_a_proxy() {
        let ours = "127.0.0.1:17990";
        let set = |flags, server: &str| Saved {
            flags,
            server: Some(server.into()),
            ..Default::default()
        };
        let proxy = win::DIRECT | win::PROXY;
        assert!(should_restore(ours, Some(&set(proxy, ours))), "still ours");
        assert!(
            !should_restore(ours, Some(&set(proxy, "10.0.0.1:3128"))),
            "someone else's"
        );
        assert!(
            !should_restore(ours, Some(&set(win::DIRECT, ours))),
            "turned off since"
        );
        assert!(should_restore(ours, None), "unreadable: restore");
        assert!(
            should_restore("", Some(&set(proxy, "10.0.0.1:3128"))),
            "no marker: restore"
        );
    }

    #[test]
    fn saved_settings_round_trip() {
        let saved = Saved {
            flags: 5,
            server: None,
            bypass: Some("<local>".into()),
            autoconfig: Some("http://wpad.corp/proxy.pac".into()),
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), saved);
    }

    /// Живая проверка ТОЛЬКО в Windows Sandbox (`PGC_SANDBOX=1`): меняет прокси Windows.
    /// Исходное состояние — как на доменном ПК: PAC и автоопределение. Включение должно стать
    /// действующим (реестр, запрос программы с системным прокси приходит к нам), выключение —
    /// вернуть всё как было.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn live_system_proxy_in_sandbox() {
        use std::io::{BufRead, BufReader};
        use windows_registry::CURRENT_USER;
        assert_eq!(
            std::env::var("PGC_SANDBOX").as_deref(),
            Ok("1"),
            "sandbox only"
        );
        let home = std::env::var("PGC_LIVE_HOME").expect("set PGC_LIVE_HOME");
        // SAFETY: до потоков, читающих окружение.
        unsafe { std::env::set_var("CLAUDE_GATEWAY_HOME", &home) };
        let original = win::query().unwrap();
        // Исходное возвращается и если тест упадёт посередине.
        struct Back(Saved);
        impl Drop for Back {
            fn drop(&mut self) {
                let _ = disable();
                let _ = win::set(&self.0);
            }
        }
        let _back = Back(original.clone());
        let domain = Saved {
            flags: win::DIRECT | 4 | 8, // + PAC + автоопределение
            server: None,
            bypass: Some("<local>;*.corp.example".into()),
            autoconfig: Some("http://wpad.corp.example/proxy.pac".into()),
        };
        win::set(&domain).unwrap();
        let before = win::query().unwrap();
        println!("before: {before:?}");
        assert_eq!(
            before.flags & 8,
            8,
            "auto-detect must be readable (FLAGS_UI)"
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut line);
                log.lock().unwrap().push(line.trim().to_string());
            }
        });
        enable(port).unwrap();
        let on = win::query().unwrap();
        println!("on: {on:?}");
        let key = CURRENT_USER
            .open(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .unwrap();
        let enabled = key.get_u32("ProxyEnable").unwrap_or(0);
        let server = key.get_string("ProxyServer").unwrap_or_default();
        println!("registry: ProxyEnable={enabled} ProxyServer={server}");
        assert_eq!(enabled, 1);
        assert_eq!(server, format!("127.0.0.1:{port}"));
        assert!(status().unwrap().on);
        // Программа с системным прокси (reqwest читает его из реестра) идёт к нам.
        let _ = reqwest::Client::new()
            .get("http://example.test/through-the-system-proxy")
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
        let lines = seen.lock().unwrap().clone();
        println!("listener saw: {lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("example.test/through-the-system-proxy"))
        );

        disable().unwrap();
        let after = win::query().unwrap();
        println!("after: {after:?}");
        assert_eq!(after, before, "the domain setup is back");
    }

    /// Чтение настроек Windows ничего не меняет и должно работать на любой машине.
    #[cfg(windows)]
    #[test]
    fn current_settings_can_be_read() {
        let saved = win::query().unwrap();
        assert!(saved.flags & win::DIRECT != 0 || saved.flags != 0);
    }
}
