//! Снимок agent-kit, встроенный в PGC при сборке (ADR-015): распаковка в постоянную папку,
//! из которой Claude Code ставит плагин как из маркетплейса.

use crate::tr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Версия встроенного снимка: первые 12 знаков коммита agent-kit; пусто — не встроен.
const VERSION: &str = env!("PGC_KIT_VERSION");
static TAR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-kit.tar"));
/// Метка распакованной папки: какая версия в ней лежит.
const MARKER: &str = ".pgc-kit";

/// Версия встроенного agent-kit, если он есть в этой сборке.
pub fn embedded() -> Option<&'static str> {
    (!TAR.is_empty() && !VERSION.is_empty()).then_some(VERSION)
}

/// Постоянная папка распаковки: одна и та же от версии к версии, чтобы источник маркетплейса
/// не менялся. Своя у release, dev-сборки и изолированного запуска (`claude::base_dir`).
pub fn dir() -> Option<PathBuf> {
    Some(crate::claude::base_dir()?.join("agent-kit"))
}

/// Встроенный agent-kit, распакованный и свежий; `None` — в этой сборке его нет.
pub fn extracted() -> Result<Option<PathBuf>, String> {
    let (Some(version), Some(dir)) = (embedded(), dir()) else {
        return Ok(None);
    };
    ensure(&dir, TAR, version).map(Some)
}

/// Распаковать `tar` в `dir`, если там не эта версия или копия неполная (случай 21). Старая
/// папка заменяется через временную: на полпути `dir` не бывает наполовину новой.
pub fn ensure(dir: &Path, tar: &[u8], version: &str) -> Result<PathBuf, String> {
    let manifest = |d: &Path| d.join(".claude-plugin").join("plugin.json");
    let have = std::fs::read_to_string(dir.join(MARKER)).unwrap_or_default();
    if have.trim() == version && manifest(dir).is_file() {
        return Ok(dir.to_path_buf());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| tr!("нет папки для agent-kit", "no folder for agent-kit"))?;
    std::fs::create_dir_all(parent).map_err(|exc| exc.to_string())?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let next = parent.join(format!("agent-kit.new-{id}"));
    let archive = parent.join(format!("agent-kit.{id}.tar"));
    let result = (|| {
        std::fs::create_dir_all(&next).map_err(|exc| exc.to_string())?;
        std::fs::write(&archive, tar).map_err(|exc| exc.to_string())?;
        let mut untar = Command::new("tar");
        untar.arg("-xf").arg(&archive).arg("-C").arg(&next);
        let output = crate::connect::run(untar, Duration::from_secs(60))?;
        if output.code != Some(0) || !manifest(&next).is_file() {
            let said = output.stderr.trim();
            return Err(tr!(
                "встроенный agent-kit не распаковался: {said}",
                "the built-in agent-kit did not unpack: {said}"
            ));
        }
        std::fs::write(next.join(MARKER), version).map_err(|exc| exc.to_string())?;
        replace(dir, &next, &parent.join(format!("agent-kit.old-{id}")))
    })();
    let _ = std::fs::remove_file(&archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&next);
    }
    result.map(|()| dir.to_path_buf())
}

/// Поставить `next` на место `dir`. Старая копия убирается только после того, как новая
/// встала; не встала (файл держит антивирус) — старая возвращается на место.
fn replace(dir: &Path, next: &Path, old: &Path) -> Result<(), String> {
    let had_old = dir.exists();
    if had_old {
        std::fs::rename(dir, old).map_err(|exc| exc.to_string())?;
    }
    if let Err(exc) = std::fs::rename(next, dir) {
        if had_old {
            let _ = std::fs::rename(old, dir);
        }
        return Err(exc.to_string());
    }
    if had_old {
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Снимок как у `kit-snapshot.ps1`: tar с `.claude-plugin/plugin.json`.
    fn snapshot(work: &Path, text: &str) -> Vec<u8> {
        let tree = work.join(format!("tree-{text}"));
        std::fs::create_dir_all(tree.join(".claude-plugin")).unwrap();
        std::fs::write(tree.join(".claude-plugin").join("plugin.json"), text).unwrap();
        let tar = work.join(format!("{text}.tar"));
        let mut make = Command::new("tar");
        make.arg("-cf")
            .arg(&tar)
            .arg("-C")
            .arg(&tree)
            .arg(".claude-plugin");
        assert_eq!(
            crate::connect::run(make, Duration::from_secs(30))
                .unwrap()
                .code,
            Some(0)
        );
        std::fs::read(&tar).unwrap()
    }

    #[test]
    fn the_snapshot_is_unpacked_once_and_replaced_when_it_changes_or_breaks() {
        let work = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-kit {}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&work).unwrap();
        let dir = work.join("профиль").join("agent-kit");
        let plugin = dir.join(".claude-plugin").join("plugin.json");
        let v1 = snapshot(&work, "one");
        assert_eq!(ensure(&dir, &v1, "v1").unwrap(), dir);
        assert_eq!(std::fs::read_to_string(&plugin).unwrap(), "one");
        // Та же версия — ничего не трогает.
        std::fs::write(dir.join("local-note"), "x").unwrap();
        ensure(&dir, &v1, "v1").unwrap();
        assert!(dir.join("local-note").exists());
        // Случай 21: копия испорчена — распаковывается заново.
        std::fs::remove_file(&plugin).unwrap();
        ensure(&dir, &v1, "v1").unwrap();
        assert_eq!(std::fs::read_to_string(&plugin).unwrap(), "one");
        // Новая версия — папка заменяется целиком, без остатков старой.
        let v2 = snapshot(&work, "two");
        ensure(&dir, &v2, "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&plugin).unwrap(), "two");
        assert!(!dir.join("local-note").exists());
        // Битый архив — ошибка, а прежняя папка цела.
        assert!(ensure(&dir, b"not a tar", "v3").is_err());
        assert_eq!(std::fs::read_to_string(&plugin).unwrap(), "two");
        let leftovers: Vec<_> = std::fs::read_dir(dir.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "agent-kit")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(&work).unwrap();
    }

    #[test]
    fn a_failed_swap_puts_the_old_copy_back() {
        let work = std::env::temp_dir()
            .join("agents")
            .join(format!("ctp-swap {}", uuid::Uuid::new_v4().simple()));
        let (dir, old) = (work.join("agent-kit"), work.join("agent-kit.old"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.json"), "old").unwrap();
        // Новой папки нет — переименование падает, как если бы её держал антивирус.
        assert!(replace(&dir, &work.join("missing"), &old).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("plugin.json")).unwrap(),
            "old"
        );
        assert!(!old.exists());
        std::fs::remove_dir_all(&work).unwrap();
    }
}
