//! 启动页文案的本地化。
//!
//! 语言由前端（`ui/i18n.js`）按系统语言探测后通过 `set_language` 告诉 Rust，
//! 因为进度行 / 卸载步骤这些文字是 Rust 生成的，得用同一种语言。

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    ZhCn,
    ZhTw,
    Ja,
    Ko,
    #[default]
    En,
}

impl Lang {
    pub fn from_tag(tag: &str) -> Lang {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.starts_with("zh") {
            // 新标签是 zh-Hans / zh-Hant；旧的 zh-CN / zh-TW 也继续认
            let traditional = tag.contains("hant")
                || tag.contains("-tw")
                || tag.contains("-hk")
                || tag.contains("-mo");
            return if traditional { Lang::ZhTw } else { Lang::ZhCn };
        }
        if tag.starts_with("ja") {
            return Lang::Ja;
        }
        if tag.starts_with("ko") {
            return Lang::Ko;
        }
        Lang::En
    }

    pub fn tag(self) -> &'static str {
        match self {
            Lang::ZhCn => "zh-Hans",
            Lang::ZhTw => "zh-Hant",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
            Lang::En => "en",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Lang::ZhCn => 0,
            Lang::ZhTw => 1,
            Lang::Ja => 2,
            Lang::Ko => 3,
            Lang::En => 4,
        }
    }
}

/// 按当前语言从 5 个候选里挑一个（顺序：简中 / 繁中 / 日 / 韩 / 英）。
pub fn pick(lang: Lang, texts: [&'static str; 5]) -> &'static str {
    texts[lang.index()]
}

/// `tr!(lang, "简体", "繁體", "日本語", "한국어", "English")`
macro_rules! tr {
    ($lang:expr, $zh:expr, $tw:expr, $ja:expr, $ko:expr, $en:expr) => {
        crate::i18n::pick($lang, [$zh, $tw, $ja, $ko, $en])
    };
}

pub(crate) use tr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_map_to_supported_languages() {
        assert_eq!(Lang::from_tag("zh-Hans"), Lang::ZhCn);
        assert_eq!(Lang::from_tag("zh-Hans-CN"), Lang::ZhCn);
        assert_eq!(Lang::from_tag("zh-SG"), Lang::ZhCn);
        // 旧标签继续认（老 settings.json / 浏览器给的老标签）
        assert_eq!(Lang::from_tag("zh-CN"), Lang::ZhCn);
        assert_eq!(Lang::from_tag("zh-Hant"), Lang::ZhTw);
        assert_eq!(Lang::from_tag("zh-HK"), Lang::ZhTw);
        assert_eq!(Lang::from_tag("zh-TW"), Lang::ZhTw);
        assert_eq!(Lang::from_tag("ja-JP"), Lang::Ja);
        assert_eq!(Lang::from_tag("ko-KR"), Lang::Ko);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        // 其它语言一律英文
        assert_eq!(Lang::from_tag("de-DE"), Lang::En);
        assert_eq!(Lang::from_tag(""), Lang::En);
    }

    #[test]
    fn pick_returns_the_matching_slot() {
        let texts = ["简", "繁", "日", "한", "en"];
        assert_eq!(pick(Lang::ZhCn, texts), "简");
        assert_eq!(pick(Lang::ZhTw, texts), "繁");
        assert_eq!(pick(Lang::Ja, texts), "日");
        assert_eq!(pick(Lang::Ko, texts), "한");
        assert_eq!(pick(Lang::En, texts), "en");
        assert_eq!(Lang::Ja.tag(), "ja");
    }
}
