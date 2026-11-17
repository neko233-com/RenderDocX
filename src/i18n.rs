use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub enum Language {
    #[default]
    Auto,
    English,
    Chinese,
}

// Custom deserialization keeps older settings files that stored an
// unsupported language (or an unknown value) loadable instead of failing the
// whole settings document; unsupported values behave as "detect automatically".
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "English" => Self::English,
            "Chinese" => Self::Chinese,
            _ => Self::Auto,
        })
    }
}

impl Language {
    pub fn no_resource(self) -> &'static str {
        self.text("No Resource", "无资源")
    }

    pub fn unknown_resource(self) -> &'static str {
        self.text("Unknown Resource", "未知资源")
    }

    pub fn edited_resource_suffix(self) -> &'static str {
        self.text(" (Edited)", "（已编辑）")
    }
    pub const ALL: [Self; 3] = [Self::Auto, Self::English, Self::Chinese];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto / 自动",
            Self::English => "English",
            Self::Chinese => "简体中文",
        }
    }

    pub fn resolved(self) -> Self {
        if self != Self::Auto {
            return self;
        }
        Self::from_locale(sys_locale::get_locale().as_deref().unwrap_or_default())
    }

    fn from_locale(locale: &str) -> Self {
        let locale = locale.to_ascii_lowercase();
        match locale.split(['-', '_']).next().unwrap_or_default() {
            "zh" => Self::Chinese,
            _ => Self::English,
        }
    }

    pub fn text(self, english: &'static str, chinese: &'static str) -> &'static str {
        match self.resolved() {
            Self::Chinese => chinese,
            _ => english,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn system_locale_mapping_and_unknown_fallback() {
        for (locale, expected) in [
            ("zh-CN", Language::Chinese),
            ("zh_Hans_CN", Language::Chinese),
            ("en-US", Language::English),
            ("JA-jp", Language::English),
            ("ko-KR", Language::English),
            ("fr-FR", Language::English),
            ("", Language::English),
        ] {
            assert_eq!(Language::from_locale(locale), expected, "{locale}");
        }
    }

    #[test]
    fn manual_language_selection_and_settings_round_trip() {
        for (language, expected) in [(Language::English, "en"), (Language::Chinese, "zh")] {
            assert_eq!(language.text("en", "zh"), expected);
        }
        for language in Language::ALL {
            let json = serde_json::to_string(&language).unwrap();
            assert_eq!(serde_json::from_str::<Language>(&json).unwrap(), language);
        }
    }

    #[test]
    fn unsupported_saved_languages_fall_back_to_auto() {
        for value in ["\"Japanese\"", "\"Korean\"", "\"de-DE\"", "\"\""] {
            assert_eq!(
                serde_json::from_str::<Language>(value).unwrap(),
                Language::Auto,
                "{value}"
            );
        }
    }
}
