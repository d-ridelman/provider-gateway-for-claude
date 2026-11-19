//! Interface language: Russian or English.
//!
//! One source for the window and for every message Rust sends to it, so the two never
//! disagree. `auto` follows the Windows display language (not the regional format).

use std::sync::atomic::{AtomicBool, Ordering};

// Tests run in parallel and many check Russian texts: they keep Russian and never switch.
static RUSSIAN: AtomicBool = AtomicBool::new(cfg!(test));

/// `"ru"` or `"en"` — the language in effect.
pub fn current() -> &'static str {
    if is_ru() { "ru" } else { "en" }
}

pub fn is_ru() -> bool {
    RUSSIAN.load(Ordering::Relaxed)
}

/// Applies the user's choice: `"ru"`, `"en"`, or anything else (`"auto"`, empty) for
/// the Windows display language.
pub fn apply(preference: Option<&str>) {
    RUSSIAN.store(resolve(preference, system_is_russian), Ordering::Relaxed);
}

fn resolve(preference: Option<&str>, system: impl Fn() -> bool) -> bool {
    match preference.map(str::trim) {
        Some("ru") => true,
        Some("en") => false,
        _ => system(),
    }
}

/// Russian if the Windows display language is Russian (LANGID primary language 0x19).
#[cfg(windows)]
fn system_is_russian() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: a plain Win32 call without arguments that only returns the LANGID of the
    // user's display language; it has no preconditions and touches no memory of ours.
    let langid = unsafe { GetUserDefaultUILanguage() };
    langid & 0x3ff == 0x19
}

#[cfg(not(windows))]
fn system_is_russian() -> bool {
    std::env::var("LANG").is_ok_and(|lang| lang.starts_with("ru"))
}

/// `tr!("Текст {x}", "Text {x}")` — the Russian or English variant for the current
/// language, formatted like `format!` (inline captures work).
#[macro_export]
macro_rules! tr {
    ($ru:literal, $en:literal $(,)?) => {
        if $crate::i18n::is_ru() {
            format!($ru)
        } else {
            format!($en)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choice_wins_and_auto_reads_windows() {
        assert!(resolve(Some("ru"), || false));
        assert!(!resolve(Some(" en "), || true));
        assert!(resolve(Some("auto"), || true));
        assert!(!resolve(None, || false));
        // The global language stays Russian in tests (see RUSSIAN).
        let x = 3;
        assert_eq!(current(), "ru");
        assert_eq!(tr!("три {x}", "three {x}"), "три 3");
    }
}
