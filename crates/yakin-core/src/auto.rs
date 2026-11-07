//! `[auto]`: the profile follows the focused window, by rules on its app id and title.

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;

use crate::Result;

/// `[auto]` as written.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AutoToml {
    pub enabled: bool,
    /// Profile when no rule matches; none leaves the profile alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rule: Vec<RuleToml>,
}

/// One `[[auto.rule]]`: regexes on the window, the profile it gets.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RuleToml {
    pub app_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub profile: String,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub app_id: Regex,
    pub title: Option<Regex>,
    pub profile: String,
}
impl Rule {
    fn matches(
        &self,
        app_id: &str,
        title: &str,
    ) -> bool {
        self.app_id.is_match(app_id)
            && self
                .title
                .as_ref()
                .is_none_or(|regex| regex.is_match(title))
    }
}
impl PartialEq for Rule {
    fn eq(
        &self,
        other: &Self,
    ) -> bool {
        self.app_id.as_str() == other.app_id.as_str()
            && self.title.as_ref().map(Regex::as_str) == other.title.as_ref().map(Regex::as_str)
            && self.profile == other.profile
    }
}

/// `[auto]` with its regexes compiled. Profile names are checked by the config.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Auto {
    pub enabled: bool,
    pub default: Option<String>,
    pub rules: Vec<Rule>,
}
impl Auto {
    /// Index of the first rule matching the window.
    pub fn rule(
        &self,
        app_id: &str,
        title: &str,
    ) -> Option<usize> {
        self.rules
            .iter()
            .position(|rule| rule.matches(app_id, title))
    }
}
impl TryFrom<&AutoToml> for Auto {
    type Error = anyhow::Error;

    fn try_from(toml: &AutoToml) -> Result<Self> {
        let compile = |index: usize, field: &str, source: &str| {
            Regex::new(source).map_err(|err| anyhow!("auto.rule.{index}.{field}: {err}"))
        };
        let mut rules = Vec::new();
        for (index, rule) in toml.rule.iter().enumerate() {
            let title = match &rule.title {
                Some(title) => Some(compile(index, "title", title)?),
                None => None,
            };
            rules.push(Rule {
                app_id: compile(index, "app_id", &rule.app_id)?,
                title,
                profile: rule.profile.clone(),
            });
        }
        Ok(Self {
            enabled: toml.enabled,
            default: toml.default.clone(),
            rules,
        })
    }
}

/// The profile the window should have: its first matching rule's, else the default.
pub fn auto_profile<'a>(
    auto: &'a Auto,
    app_id: &str,
    title: &str,
) -> Option<&'a str> {
    match auto.rule(app_id, title) {
        Some(index) => Some(&auto.rules[index].profile),
        None => auto.default.as_deref(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(text: &str) -> Result<Auto> {
        let toml: AutoToml = toml::from_str(text)?;
        Auto::try_from(&toml)
    }

    const RULES: &str = r#"
        enabled = true
        default = "desktop"
        [[rule]]
        app_id = "^steam_app_"
        profile = "pad"
        [[rule]]
        app_id = "^firefox$"
        title = "YouTube"
        profile = "browser"
        [[rule]]
        app_id = "^firefox$"
        profile = "desktop"
    "#;

    #[test]
    fn first_matching_rule_wins() {
        let auto = auto(RULES).unwrap();
        assert_eq!(auto_profile(&auto, "steam_app_1234", ""), Some("pad"));
        assert_eq!(
            auto_profile(&auto, "firefox", "Cats - YouTube"),
            Some("browser")
        );
        assert_eq!(auto.rule("firefox", "Cats - YouTube"), Some(1));
        assert_eq!(auto.rule("firefox", "News"), Some(2));
    }

    #[test]
    fn default_when_nothing_matches() {
        let auto = auto(RULES).unwrap();
        assert_eq!(auto.rule("foot", "~"), None);
        assert_eq!(auto_profile(&auto, "foot", "~"), Some("desktop"));
        assert_eq!(auto_profile(&auto, "", ""), Some("desktop"));
    }

    #[test]
    fn no_default_leaves_the_profile() {
        let auto = auto("[[rule]]\napp_id = \"x\"\nprofile = \"pad\"").unwrap();
        assert_eq!(auto_profile(&auto, "foot", ""), None);
    }

    #[test]
    fn rule_profiles_must_exist() {
        let config = "[profile.desktop]\n[auto]\ndefault = \"desktop\"\n";
        assert!(crate::profile::Config::parse(config).is_ok());
        let err = crate::profile::Config::parse(&format!(
            "{config}[[auto.rule]]\napp_id = \"x\"\nprofile = \"nope\"\n"
        ))
        .unwrap_err();
        assert_eq!(err.to_string(), "auto.rule.0: No profile named nope");
        let err = crate::profile::Config::parse("[auto]\ndefault = \"nope\"").unwrap_err();
        assert_eq!(err.to_string(), "auto.default: No profile named nope");
    }

    #[test]
    fn bad_regex_names_the_rule() {
        let err = auto("[[rule]]\napp_id = \"(\"\nprofile = \"pad\"").unwrap_err();
        assert!(err.to_string().starts_with("auto.rule.0.app_id: "), "{err}");
        let err = auto("[[rule]]\napp_id = \"x\"\ntitle = \"[\"\nprofile = \"pad\"").unwrap_err();
        assert!(err.to_string().starts_with("auto.rule.0.title: "), "{err}");
    }
}
