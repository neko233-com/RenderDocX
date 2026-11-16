use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub enum Language {
    #[default]
    Auto,
    English,
    Chinese,
    Japanese,
    Korean,
}

impl Language {
    pub fn no_resource(self) -> &'static str {
        self.text("No Resource", "无资源", "リソースなし", "리소스 없음")
    }

    pub fn unknown_resource(self) -> &'static str {
        self.text(
            "Unknown Resource",
            "未知资源",
            "不明なリソース",
            "알 수 없는 리소스",
        )
    }

    pub fn edited_resource_suffix(self) -> &'static str {
        self.text(" (Edited)", "（已编辑）", "（編集済み）", " (편집됨)")
    }
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::English,
        Self::Chinese,
        Self::Japanese,
        Self::Korean,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto / 自动",
            Self::English => "English",
            Self::Chinese => "简体中文",
            Self::Japanese => "日本語",
            Self::Korean => "한국어",
        }
    }

    fn resolved(self) -> Self {
        if self != Self::Auto {
            return self;
        }
        let locale = sys_locale::get_locale()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match locale.split(['-', '_']).next().unwrap_or_default() {
            "zh" => Self::Chinese,
            "ja" => Self::Japanese,
            "ko" => Self::Korean,
            _ => Self::English,
        }
    }

    pub fn text(
        self,
        english: &'static str,
        chinese: &'static str,
        japanese: &'static str,
        korean: &'static str,
    ) -> &'static str {
        match self.resolved() {
            Self::Chinese => chinese,
            Self::Japanese => japanese,
            Self::Korean => korean,
            _ => english,
        }
    }
}
