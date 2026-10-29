//! UI language: Turkish, English or Spanish. The choice made in the app is saved; until then the
//! system language is used, and English when the system language is none of the three.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Tr,
    En,
    Es,
}

pub const ALL: [Lang; 3] = [Lang::Tr, Lang::En, Lang::Es];

static LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);

impl Lang {
    /// The language's own name, for the picker.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Tr => "Türkçe",
            Lang::En => "English",
            Lang::Es => "Español",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Lang::Tr => "tr",
            Lang::En => "en",
            Lang::Es => "es",
        }
    }

    /// From a locale like "tr_TR.UTF-8", "es-419" or "en".
    fn from_locale(locale: &str) -> Option<Self> {
        let code = locale.trim().trim_matches('"').get(..2)?.to_ascii_lowercase();
        ALL.into_iter().find(|l| l.code() == code)
    }
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        0 => Lang::Tr,
        2 => Lang::Es,
        _ => Lang::En,
    }
}

/// The text for the current language.
pub fn t(tr: &'static str, en: &'static str, es: &'static str) -> &'static str {
    match lang() {
        Lang::Tr => tr,
        Lang::En => en,
        Lang::Es => es,
    }
}

/// `format!` with one template per language: `tf!("{n} dosya", "{n} files", "{n} archivos")`.
#[macro_export]
macro_rules! tf {
    ($tr:literal, $en:literal, $es:literal $(, $($arg:tt)*)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::Tr => format!($tr $(, $($arg)*)?),
            $crate::i18n::Lang::En => format!($en $(, $($arg)*)?),
            $crate::i18n::Lang::Es => format!($es $(, $($arg)*)?),
        }
    };
}

fn saved_path() -> Option<std::path::PathBuf> {
    Some(crate::config_dir()?.join("lang"))
}

/// Picks the language at startup: the saved choice, else the system language.
pub fn init() {
    let saved = saved_path().and_then(|p| std::fs::read_to_string(p).ok());
    let lang = saved.as_deref().and_then(Lang::from_locale).or_else(system).unwrap_or(Lang::En);
    LANG.store(lang as u8, Ordering::Relaxed);
}

/// Switches the language and remembers it.
pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
    if let Some(path) = saved_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, lang.code());
    }
}

fn system() -> Option<Lang> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(lang) = std::env::var(var).ok().as_deref().and_then(Lang::from_locale) {
            return Some(lang);
        }
    }
    // Apps opened from Finder get no LANG; the user's preferred languages are in the global defaults.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output() {
        // "(\n    \"tr-TR\",\n    \"en-US\"\n)": the first entry is the preferred one.
        return String::from_utf8_lossy(&out.stdout).split('"').nth(1).and_then(Lang::from_locale);
    }
    // Windows sets none of those variables; the user's display language lives in the locale API.
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        // LOCALE_NAME_MAX_LENGTH, the most the API ever writes.
        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        // The count includes the trailing NUL, e.g. "tr-TR\0" is 6.
        if len > 1 {
            return Lang::from_locale(&String::from_utf16_lossy(&buf[..len as usize - 1]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_locales() {
        assert_eq!(Lang::from_locale("tr_TR.UTF-8"), Some(Lang::Tr));
        assert_eq!(Lang::from_locale("\"es-419\""), Some(Lang::Es));
        assert_eq!(Lang::from_locale("en"), Some(Lang::En));
        assert_eq!(Lang::from_locale("de_DE"), None);
        assert_eq!(Lang::from_locale("C"), None);
    }
}
