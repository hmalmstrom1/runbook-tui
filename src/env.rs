use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

type VariableList = Vec<(String, String)>;

#[derive(Debug, Deserialize, Default)]
struct VariableGroupFile {
    #[serde(default)]
    environments: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    variable: Vec<VariableEntry>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VariableEntry {
    key: String,
    value: String,
}

pub(crate) fn parse_variable_groups(path: &Path) -> Result<Vec<(String, VariableList)>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read variable group file {}", path.display()))?;
    let file: VariableGroupFile = serde_json::from_str(&content)
        .with_context(|| "Failed to parse variable group file as JSON")?;

    let mut groups = Vec::new();

    if let Some(name) = file.name {
        let vars: VariableList = file.variable.into_iter().map(|v| (v.key, v.value)).collect();
        groups.push((name, vars));
    } else if !file.variable.is_empty() {
        let vars: VariableList = file.variable.into_iter().map(|v| (v.key, v.value)).collect();
        groups.push(("default".to_string(), vars));
    }

    for (name, vars) in file.environments {
        let list: VariableList = vars.into_iter().collect();
        groups.push((name, list));
    }

    Ok(groups)
}
