//! Установленный MSIX Claude: пакет, служба `CoworkVMService` и запуск ради VM.
//!
//! Служба VM принимает клиента только из каталога самого пакета
//! (`C:\Program Files\WindowsApps\Claude_…`): идентичности пакета у переведённой копии
//! мало, служба отвечает «client … was not started from the package directory»
//! (проверено в Windows Sandbox, ADR-018). Поэтому VM-режим запускает настоящий
//! `claude.exe` пакета, но с профилем и конфигом PGC.
//!
//! `Invoke-CommandInDesktopPackage` не передаёт процессу переменные окружения вызывающего,
//! поэтому запуск идёт через `cmd.exe` (`set` и сам exe), а `-PreventBreakaway` оставляет
//! потомку идентичность пакета.

use crate::tr;
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;
use std::process::Command;

/// Найденный пакет Claude.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub family: String,
    pub app_id: String,
    pub version: String,
    /// `claude.exe` внутри каталога пакета.
    pub exe: PathBuf,
}

const DETECT: &str = r#"
$pkgs = Get-AppxPackage | Where-Object { $_.Name -eq 'Claude' -or $_.Name -eq 'AnthropicPBC.Claude' } | Sort-Object Version -Descending
$p = $pkgs | Select-Object -First 1
if (-not $p) { exit 1 }
$m = Get-AppxPackageManifest $p
$app = $m.Package.Applications.Application | Select-Object -First 1
$exe = Join-Path $p.InstallLocation 'app\claude.exe'
if (-not (Test-Path $exe)) {
    $found = Get-ChildItem $p.InstallLocation -Recurse -Filter claude.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
    if ($found) { $exe = $found }
}
[pscustomobject]@{ family = $p.PackageFamilyName; id = $app.Id; version = [string]$p.Version; exe = $exe } | ConvertTo-Json -Compress
"#;

/// `claude.exe` из пакета текущего пользователя — без PowerShell: раздел AppModel хранит
/// корень установки. `detect` зовут часто, а `Get-AppxPackage` стоит 1,3 с.
#[cfg(windows)]
pub fn installed_exe() -> Option<PathBuf> {
    use windows_registry::CURRENT_USER;
    const REPO: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    let repo = CURRENT_USER.open(REPO).ok()?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for name in repo.keys().ok()? {
        let Some(version) = name
            .strip_prefix("Claude_")
            .or_else(|| name.strip_prefix("AnthropicPBC.Claude_"))
            .and_then(|rest| rest.split('_').next())
        else {
            continue;
        };
        if !version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        {
            continue;
        }
        let key: Vec<u64> = version
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect();
        let Ok(root) = repo
            .open(&name)
            .and_then(|package| package.get_string("PackageRootFolder"))
        else {
            continue;
        };
        let exe = PathBuf::from(root).join("app").join("claude.exe");
        if !exe.is_file() {
            continue;
        }
        if best.as_ref().is_none_or(|(have, _)| key > *have) {
            best = Some((key, exe));
        }
    }
    best.map(|(_, exe)| exe)
}

#[cfg(not(windows))]
pub fn installed_exe() -> Option<PathBuf> {
    None
}

/// PowerShell без окна консоли: окно мигало бы при каждом запуске Claude из пакета.
fn powershell(script: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Пакет Claude, установленный для текущего пользователя (Store, провижининг или MSIX).
pub fn detect() -> Option<Installed> {
    let output = powershell(DETECT).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_detect(&String::from_utf8_lossy(&output.stdout))
}

fn parse_detect(text: &str) -> Option<Installed> {
    let line = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))?;
    #[derive(serde::Deserialize)]
    struct Raw {
        family: String,
        id: String,
        version: String,
        exe: String,
    }
    let raw: Raw = serde_json::from_str(line.trim()).ok()?;
    if raw.family.is_empty() || raw.id.is_empty() || raw.exe.is_empty() {
        return None;
    }
    Some(Installed {
        family: raw.family,
        app_id: raw.id,
        version: raw.version,
        exe: PathBuf::from(raw.exe),
    })
}

/// Служба есть только у полной MSIX-установки; у per-user установки без прав
/// администратора Claude ставится без Cowork.
#[cfg(windows)]
pub fn service_installed() -> bool {
    windows_registry::LOCAL_MACHINE
        .open(r"SYSTEM\CurrentControlSet\Services\CoworkVMService")
        .is_ok()
}

#[cfg(not(windows))]
pub fn service_installed() -> bool {
    false
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Символы, которые `cmd.exe` истолкует сам (или которых не должно быть в `set "…"`).
const CMD_UNSAFE: &[char] = &['"', '%', '&', '|', '<', '>', '^', '\r', '\n'];

fn cmd_token(value: &str) -> Result<String> {
    ensure!(
        !value.contains(CMD_UNSAFE),
        "{}",
        tr!(
            "значение «{value}» нельзя передать через cmd.exe",
            "the value \"{value}\" cannot be passed through cmd.exe"
        )
    );
    Ok(if value.contains(' ') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    })
}

/// Командная строка для `cmd.exe /c`: `set` переменных, затем exe пакета с аргументами.
fn cmd_line(exe: &str, args: &[String], envs: &[(String, String)]) -> Result<String> {
    ensure!(
        !exe.contains(CMD_UNSAFE),
        "{}",
        tr!("путь к exe не подходит", "the exe path is not usable")
    );
    let mut line = String::from("/c ");
    for (name, value) in envs {
        ensure!(
            !name.is_empty() && !name.contains(['=', ' ']) && !name.contains(CMD_UNSAFE),
            "{}",
            tr!(
                "имя переменной «{name}» нельзя передать через cmd.exe",
                "the variable name \"{name}\" cannot be passed through cmd.exe"
            )
        );
        ensure!(
            !value.contains(CMD_UNSAFE),
            "{}",
            tr!(
                "значение переменной {name} нельзя передать через cmd.exe",
                "the value of {name} cannot be passed through cmd.exe"
            )
        );
        line.push_str(&format!("set \"{name}={value}\" && "));
    }
    line.push_str(&format!("\"{exe}\""));
    for arg in args {
        line.push(' ');
        line.push_str(&cmd_token(arg)?);
    }
    Ok(line)
}

/// Запустить настоящий `claude.exe` пакета с профилем PGC: служба VM его примет, а профиль,
/// конфиг шлюза и аргументы — наши. Перевода здесь нет: переведённая копия вне каталога
/// пакета службе не подходит.
pub fn launch(package: &Installed, args: &[String], envs: &[(String, String)]) -> Result<()> {
    let line = cmd_line(&package.exe.display().to_string(), args, envs)?;
    let script = format!(
        "Invoke-CommandInDesktopPackage -PackageFamilyName {} -AppId {} -Command {} -Args {} -PreventBreakaway -ErrorAction Stop",
        quote(&package.family),
        quote(&package.app_id),
        quote(r"C:\Windows\System32\cmd.exe"),
        quote(&line),
    );
    let output = powershell(&script).output().with_context(|| {
        tr!(
            "не удалось запустить PowerShell",
            "could not start PowerShell"
        )
    })?;
    let err = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "{}",
        tr!(
            "MSIX-запуск не сработал: {err}",
            "MSIX launch failed: {err}"
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_quotes_single_quotes() {
        assert_eq!(quote(r"C:\a'b\claude.exe"), r"'C:\a''b\claude.exe'");
    }

    #[test]
    fn registry_finds_the_installed_package() {
        // На машине без пакета проверять нечего (CI); где он есть — путь должен существовать.
        if let Some(exe) = installed_exe() {
            assert!(exe.is_file(), "{}", exe.display());
            assert!(exe.ends_with("app\\claude.exe") || exe.ends_with("app/claude.exe"));
        }
    }

    #[test]
    fn detect_reads_the_last_json_line() {
        let text = "warning\n{\"family\":\"Claude_pzs8sxrjxfjjc\",\"id\":\"Claude\",\"version\":\"6.28899.6\",\"exe\":\"C:\\\\Program Files\\\\WindowsApps\\\\Claude_6.28899.6.0_x64__pzs8sxrjxfjjc\\\\app\\\\claude.exe\"}\n";
        let parsed = parse_detect(text).unwrap();
        assert_eq!(parsed.family, "Claude_pzs8sxrjxfjjc");
        assert_eq!(parsed.app_id, "Claude");
        assert!(parsed.exe.ends_with("app\\claude.exe") || parsed.exe.ends_with("app/claude.exe"));
        assert!(parse_detect("no package").is_none());
        assert!(parse_detect(r#"{"family":"","id":"Claude","version":"1","exe":"x"}"#).is_none());
        assert!(parse_detect(r#"{"family":"F","id":"Claude","version":"1","exe":""}"#).is_none());
    }

    #[test]
    fn the_command_line_sets_variables_then_starts_the_package_exe() {
        let line = cmd_line(
            r"C:\Program Files\WindowsApps\Claude_1\app\claude.exe",
            &[r"--user-data-dir=C:\Users\A B\Claude-3p".to_string()],
            &[
                ("LOCALAPPDATA".into(), r"C:\Users\A B\PGC".into()),
                (
                    "CLAUDE_USER_DATA_DIR".into(),
                    r"C:\Users\A B\PGC\Claude-3p".into(),
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            line,
            r#"/c set "LOCALAPPDATA=C:\Users\A B\PGC" && set "CLAUDE_USER_DATA_DIR=C:\Users\A B\PGC\Claude-3p" && "C:\Program Files\WindowsApps\Claude_1\app\claude.exe" "--user-data-dir=C:\Users\A B\Claude-3p""#
        );
    }

    #[test]
    fn values_that_cmd_would_reinterpret_are_refused() {
        for bad in ["a&b", "a|b", "a%PATH%", "a\"b", "a^b", "a<b", "a>b", "a\nb"] {
            let envs = vec![("X".to_string(), bad.to_string())];
            assert!(cmd_line("claude.exe", &[], &envs).is_err(), "{bad}");
            assert!(
                cmd_line("claude.exe", &[bad.to_string()], &[]).is_err(),
                "{bad}"
            );
        }
        assert!(cmd_line("claude.exe", &[], &[("A B".into(), "x".into())]).is_err());
        assert!(cmd_line("claude.exe", &[], &[("A=B".into(), "x".into())]).is_err());
    }
}
