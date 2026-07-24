use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

type VariableList = Vec<(String, String)>;

#[derive(Debug, Default)]
pub(crate) struct ParsedEnvGroups {
    pub(crate) groups: Vec<(String, VariableList)>,
    pub(crate) secret_keys: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EnvValue {
    String(String),
    Object { value: String, #[serde(default, rename = "type")] var_type: String },
}

#[derive(Debug, Deserialize, Default)]
struct VariableGroupFile {
    #[serde(default)]
    environments: BTreeMap<String, BTreeMap<String, EnvValue>>,
    #[serde(default)]
    variable: Vec<VariableEntry>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VariableEntry {
    key: String,
    value: String,
    #[serde(default, rename = "type")]
    var_type: String,
}

fn is_secret(var_type: &str) -> bool {
    var_type.eq_ignore_ascii_case("secret")
}

fn take_env_value(v: EnvValue) -> (String, bool) {
    match v {
        EnvValue::String(value) => (value, false),
        EnvValue::Object { value, var_type } => (value, is_secret(&var_type)),
    }
}

pub(crate) fn parse_variable_groups(path: &Path) -> Result<ParsedEnvGroups> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read variable group file {}", path.display()))?;
    let file: VariableGroupFile = serde_json::from_str(&content)
        .with_context(|| "Failed to parse variable group file as JSON")?;

    let mut groups = Vec::new();
    let mut secret_keys = BTreeSet::new();

    if let Some(name) = file.name {
        let mut vars = VariableList::new();
        for v in file.variable {
            if is_secret(&v.var_type) {
                secret_keys.insert(v.key.clone());
            }
            vars.push((v.key, v.value));
        }
        groups.push((name, vars));
    } else if !file.variable.is_empty() {
        let mut vars = VariableList::new();
        for v in file.variable {
            if is_secret(&v.var_type) {
                secret_keys.insert(v.key.clone());
            }
            vars.push((v.key, v.value));
        }
        groups.push(("default".to_string(), vars));
    }

    for (name, vars) in file.environments {
        let mut list = VariableList::new();
        for (key, value) in vars {
            let (value, secret) = take_env_value(value);
            if secret {
                secret_keys.insert(key.clone());
            }
            list.push((key, value));
        }
        groups.push((name, list));
    }

    Ok(ParsedEnvGroups { groups, secret_keys })
}

pub(crate) fn env_overrides(collection_variables: &[(String, String)]) -> VariableList {
    let mut group = VariableList::new();
    for (key, _) in collection_variables {
        if let Some(value) = env_value_for_key(key) {
            group.push((key.clone(), value));
        }
    }
    group
}

fn env_value_for_key(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| std::env::var(key.to_uppercase()).ok())
        .or_else(|| std::env::var(to_screaming_snake(key)).ok())
}

fn to_screaming_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::new();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            result.push('_');
            continue;
        }
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let prev_upper_and_next_lower = i > 0
                && chars[i - 1].is_uppercase()
                && i + 1 < chars.len()
                && chars[i + 1].is_lowercase();
            if prev_lower || prev_upper_and_next_lower {
                result.push('_');
            }
        }
        for uc in c.to_uppercase() {
            result.push(uc);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screaming_snake_cases() {
        assert_eq!(to_screaming_snake("baseUrl"), "BASE_URL");
        assert_eq!(to_screaming_snake("myApiKey"), "MY_API_KEY");
        assert_eq!(to_screaming_snake("APIKey"), "API_KEY");
        assert_eq!(to_screaming_snake("base_url"), "BASE_URL");
        assert_eq!(to_screaming_snake("URL"), "URL");
    }

    #[test]
    fn env_overrides_match_shell_env_var_names() {
        unsafe {
            std::env::set_var("BASE_URL", "https://example.com");
            std::env::set_var("MY_API_KEY", "secret");
        }
        let variables = vec![
            ("baseUrl".to_string(), "http://localhost".to_string()),
            ("myApiKey".to_string(), "local".to_string()),
            ("other".to_string(), "value".to_string()),
        ];
        let overrides = env_overrides(&variables);
        assert_eq!(overrides.len(), 2);
        assert!(overrides.contains(&("baseUrl".to_string(), "https://example.com".to_string())));
        assert!(overrides.contains(&("myApiKey".to_string(), "secret".to_string())));
        unsafe {
            std::env::remove_var("BASE_URL");
            std::env::remove_var("MY_API_KEY");
        }
    }
}
