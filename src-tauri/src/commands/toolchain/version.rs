use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::models::{ToolchainRemoteVersion, ToolchainVersion};

use super::process::run_with_timeout;

pub(super) fn probe_version(exe: &Path, attempts: &[&str]) -> Option<String> {
    let mut last_ok_output = String::new();
    for args in attempts {
        if let Some((true, output)) = run_with_timeout(exe, &[args]) {
            if let Some(version) = extract_semver(&output) {
                return Some(version);
            }
            last_ok_output = output;
        }
    }
    first_nonempty_line(&last_ok_output)
}

pub(super) fn extract_semver(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d+(\.\d+)+").unwrap());
    re.find(text).map(|matched| matched.as_str().to_string())
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

pub(super) fn parse_nvm_list(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let current = line.trim_start().starts_with('*');
            let rest = line.trim().trim_start_matches('*').trim_start();
            let name = rest.split_whitespace().next()?;
            version_token(name).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

pub(super) fn parse_token_versions(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let current =
                line.starts_with('*') || line.contains("default") || line.contains("current");
            let rest = line.trim_start().trim_start_matches('*').trim_start();
            let token = rest.split_whitespace().next()?;
            version_token(token).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

pub(super) fn parse_vp_env_list(text: &str) -> Vec<ToolchainVersion> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(text, "")
        .lines()
        .filter_map(|line| {
            let current = line.contains("current");
            let rest = line.trim_start().trim_start_matches('*').trim_start();
            let token = rest.split_whitespace().next()?;
            version_token(token).map(|name| ToolchainVersion { name, current })
        })
        .collect()
}

pub(super) fn version_token(token: &str) -> Option<String> {
    let bare = token.trim_start_matches('v');
    bare.starts_with(|character: char| character.is_ascii_digit())
        .then(|| bare.to_string())
}

pub(super) fn parse_dotnet_sdks(text: &str) -> Vec<ToolchainVersion> {
    text.lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            version_token(name).map(|name| ToolchainVersion {
                name,
                current: false,
            })
        })
        .collect()
}

pub(super) fn uv_python_versions(uv: &Path) -> Vec<ToolchainVersion> {
    let Some((true, output)) = run_with_timeout(uv, &["python", "list", "--only-installed"]) else {
        return Vec::new();
    };
    let current = run_with_timeout(uv, &["python", "find"])
        .filter(|(ok, _)| *ok)
        .map(|(_, output)| output)
        .and_then(|output| python_version_from_path(&output));
    parse_uv_python_list(&output)
        .into_iter()
        .map(|name| {
            let matched = current.as_deref().is_some_and(|current| {
                name == current
                    || name.starts_with(&format!("{current}."))
                    || current.starts_with(&format!("{name}."))
            });
            ToolchainVersion {
                current: matched,
                name,
            }
        })
        .collect()
}

pub(super) fn parse_uv_python_list(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let is_python = token.starts_with("cpython-") || token.starts_with("pypy-");
            let version = extract_semver(token)?;
            is_python
                .then_some(version)
                .filter(|version| seen.insert(version.clone()))
        })
        .collect()
}

pub(super) fn parse_uv_python_remote(text: &str) -> Vec<ToolchainRemoteVersion> {
    let mut seen = HashSet::new();
    text.lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let rest = token
                .strip_prefix("cpython-")
                .or_else(|| token.strip_prefix("pypy-"))?;
            let version = rest.split(['-', '+']).next()?.to_string();
            seen.insert(version.clone())
                .then_some(ToolchainRemoteVersion {
                    name: version,
                    tag: None,
                })
        })
        .collect()
}

pub(super) fn python_version_from_path(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?:cpython|pypy)-(\d+\.\d+\.\d+)|[Pp]ython3?(\d{2})[\\/]|python3\.(\d+)")
            .unwrap()
    });
    let captures = re.captures(text)?;
    if let Some(matched) = captures.get(1) {
        return Some(matched.as_str().to_string());
    }
    if let Some(matched) = captures.get(2) {
        return Some(format!("3.{}", matched.as_str()));
    }
    captures
        .get(3)
        .map(|matched| format!("3.{}", matched.as_str()))
}

pub(super) fn parse_gh_auth_status(text: &str) -> Option<String> {
    let mut first = None;
    let mut last = None;
    let mut active = None;
    for line in text.lines() {
        if let Some(rest) = line.split("account ").nth(1) {
            if let Some(name) = rest.split_whitespace().next() {
                first.get_or_insert_with(|| name.to_string());
                last = Some(name.to_string());
            }
        } else if line.contains("Active account: true") {
            if let Some(name) = last.take() {
                active = Some(name);
            }
        }
    }
    active.or(first)
}

pub(super) fn parse_nvm_available_table(text: &str) -> Vec<ToolchainRemoteVersion> {
    let mut column_tags = Vec::new();
    let mut versions = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .collect();
        if cells.is_empty() {
            continue;
        }
        if column_tags.is_empty() {
            if cells.iter().all(|cell| {
                ["CURRENT", "LTS", "STABLE", "UNSTABLE"]
                    .iter()
                    .any(|keyword| cell.contains(keyword))
            }) {
                column_tags = cells.iter().map(|cell| cell.to_string()).collect();
            }
            continue;
        }
        if cells.len() != column_tags.len() {
            continue;
        }
        for (cell, tag) in cells.iter().zip(&column_tags) {
            if let Some(name) = version_token(cell) {
                versions.push(ToolchainRemoteVersion {
                    name,
                    tag: Some(tag.clone()),
                });
            }
        }
    }
    versions
}

pub(super) fn parse_vp_remote(text: &str) -> Vec<ToolchainRemoteVersion> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    re.replace_all(text, "")
        .lines()
        .filter_map(|line| {
            let token = line.split_whitespace().next()?;
            let name = version_token(token)?;
            let tag = if line.contains('(') { "LTS" } else { "Current" };
            Some(ToolchainRemoteVersion {
                name,
                tag: Some(tag.to_string()),
            })
        })
        .collect()
}

pub(super) fn parse_remote_tokens(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_whitespace().next().and_then(version_token))
        .collect()
}

pub(super) fn natural_version_cmp(a: &str, b: &str) -> Ordering {
    let parse = |value: &str| -> Vec<u64> {
        value
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parse(a).cmp(&parse(b))
}
