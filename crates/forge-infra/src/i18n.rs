//! Internationalization (i18n) support for Forge
//!
//! Provides localization using Mozilla's Fluent format.
//! Currently supports English (en-US) and Chinese (zh-CN).

use fluent::{FluentArgs, FluentResource, FluentValue};
use fluent_bundle::bundle::FluentBundle;
use intl_memoizer::concurrent::IntlLangMemoizer;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

/// Thread-safe FluentBundle type alias
type ConcurrentFluentBundle = FluentBundle<FluentResource, IntlLangMemoizer>;

/// Supported languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// English (en-US) - Default
    #[default]
    English,
    /// Chinese Simplified (zh-CN)
    Chinese,
}

impl Language {
    /// Get the language identifier
    #[must_use]
    pub fn lang_id(&self) -> LanguageIdentifier {
        match self {
            Language::English => "en-US".parse().expect("en-US is a valid language identifier"),
            Language::Chinese => "zh-CN".parse().expect("zh-CN is a valid language identifier"),
        }
    }

    /// Parse language from string
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "en" | "en-us" | "en_us" | "english" => Some(Language::English),
            "zh" | "zh-cn" | "zh_cn" | "chinese" | "中文" => Some(Language::Chinese),
            _ => None,
        }
    }

    /// Get the display name
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "中文",
        }
    }
}

/// Global I18n instance
static I18N: OnceLock<I18n> = OnceLock::new();

/// Internationalization manager
pub struct I18n {
    bundle: ConcurrentFluentBundle,
    language: Language,
}

impl I18n {
    /// Initialize the global I18n instance
    ///
    /// If language is None, automatically detects from:
    /// 1. FORGE_LANG environment variable
    /// 2. System locale
    /// 3. Falls back to English
    pub fn init(language: Option<Language>) -> &'static Self {
        I18N.get_or_init(|| {
            let lang = language.unwrap_or_else(Self::detect_language);
            tracing::info!("Initializing i18n with language: {:?}", lang);
            Self::new(lang)
        })
    }

    /// Get the global I18n instance
    ///
    /// # Panics
    /// Panics if I18n has not been initialized
    #[must_use]
    pub fn global() -> &'static Self {
        I18N.get().expect("I18n not initialized. Call I18n::init() first.")
    }

    /// Try to get the global I18n instance
    #[must_use]
    pub fn try_global() -> Option<&'static Self> {
        I18N.get()
    }

    fn new(language: Language) -> Self {
        let mut bundle = ConcurrentFluentBundle::new_concurrent(vec![language.lang_id()]);

        let ftl_string = match language {
            Language::English => include_str!("../locales/en-US/main.ftl"),
            Language::Chinese => include_str!("../locales/zh-CN/main.ftl"),
        };

        let resource =
            FluentResource::try_new(ftl_string.to_string()).expect("Failed to parse FTL string");
        bundle.add_resource(resource).expect("Failed to add FTL resource");

        Self { bundle, language }
    }

    /// Detect system language
    fn detect_language() -> Language {
        if let Ok(lang) = std::env::var("FORGE_LANG") {
            if let Some(l) = Language::from_str(&lang) {
                tracing::debug!("Using language from FORGE_LANG: {:?}", l);
                return l;
            }
        }

        if let Some(locale) = sys_locale::get_locale() {
            tracing::debug!("System locale: {}", locale);
            if locale.starts_with("zh") {
                return Language::Chinese;
            }
        }

        Language::English
    }

    /// Translate a message by ID
    #[must_use]
    pub fn translate(&self, id: &str) -> String {
        self.translate_with_args(id, &[])
    }

    /// Translate a message with arguments
    #[must_use]
    pub fn translate_with_args(&self, id: &str, args: &[(&str, &str)]) -> String {
        let msg = match self.bundle.get_message(id) {
            Some(m) => m,
            None => {
                tracing::warn!("Translation not found: {}", id);
                return id.to_string();
            }
        };

        let pattern = match msg.value() {
            Some(p) => p,
            None => {
                tracing::warn!("Translation has no value: {}", id);
                return id.to_string();
            }
        };

        let fluent_args = if args.is_empty() {
            None
        } else {
            let mut fa = FluentArgs::new();
            for (key, value) in args {
                fa.set(*key, FluentValue::from(*value));
            }
            Some(fa)
        };

        let mut errors = vec![];
        let value = self.bundle.format_pattern(pattern, fluent_args.as_ref(), &mut errors);

        if !errors.is_empty() {
            tracing::warn!("Translation errors for '{}': {:?}", id, errors);
        }

        value.to_string()
    }

    /// Get current language
    #[must_use]
    pub fn language(&self) -> Language {
        self.language
    }

    /// Check if current language is English
    #[must_use]
    pub fn is_english(&self) -> bool {
        self.language == Language::English
    }

    /// Check if current language is Chinese
    #[must_use]
    pub fn is_chinese(&self) -> bool {
        self.language == Language::Chinese
    }
}

/// Convenience macro for translation
#[macro_export]
macro_rules! t {
    ($id:expr) => {
        $crate::i18n::I18n::global().translate($id)
    };
    ($id:expr, $($key:expr => $value:expr),+ $(,)?) => {{
        let args: &[(&str, &str)] = &[$(($key, $value)),+];
        $crate::i18n::I18n::global().translate_with_args($id, args)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_str() {
        assert_eq!(Language::from_str("en"), Some(Language::English));
        assert_eq!(Language::from_str("en-US"), Some(Language::English));
        assert_eq!(Language::from_str("zh"), Some(Language::Chinese));
        assert_eq!(Language::from_str("zh-CN"), Some(Language::Chinese));
        assert_eq!(Language::from_str("中文"), Some(Language::Chinese));
        assert_eq!(Language::from_str("invalid"), None);
    }

    #[test]
    fn test_language_display_name() {
        assert_eq!(Language::English.display_name(), "English");
        assert_eq!(Language::Chinese.display_name(), "中文");
    }

    #[test]
    fn test_i18n_new_english() {
        let i18n = I18n::new(Language::English);
        assert_eq!(i18n.language(), Language::English);

        let ready = i18n.translate("status-ready");
        assert_eq!(ready, "Ready");

        let thinking = i18n.translate("status-thinking");
        assert_eq!(thinking, "Thinking...");
    }

    #[test]
    fn test_i18n_new_chinese() {
        let i18n = I18n::new(Language::Chinese);
        assert_eq!(i18n.language(), Language::Chinese);

        let ready = i18n.translate("status-ready");
        assert_eq!(ready, "就绪");

        let thinking = i18n.translate("status-thinking");
        assert_eq!(thinking, "思考中...");
    }

    #[test]
    fn test_i18n_with_args() {
        let i18n = I18n::new(Language::English);
        let status = i18n.translate_with_args("status-tool-running", &[("tool", "read")]);
        assert!(status.contains("Running:"));
        assert!(status.contains("read"));
    }

    #[test]
    fn test_i18n_missing_key() {
        let i18n = I18n::new(Language::English);
        let missing = i18n.translate("non-existent-key");
        assert_eq!(missing, "non-existent-key");
    }

    #[test]
    fn test_language_default() {
        assert_eq!(Language::default(), Language::English);
    }
}
