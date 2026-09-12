//! 安装器更新注册表后，运行中的 GUI 仍持有旧环境；仅刷新探测与操作子进程，避免全局环境竞态。
use std::collections::BTreeMap;
use std::process::Command;

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::RegKey;

type Environment = BTreeMap<String, String>;

pub(super) fn apply(command: &mut Command) {
    let inherited: Environment = std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().to_uppercase(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let read = |root, path: &str| {
        let Ok(key) = RegKey::predef(root).open_subkey(path) else {
            return Environment::new();
        };
        key.enum_values()
            .filter_map(Result::ok)
            .filter_map(|(name, _)| {
                key.get_value::<String, _>(&name)
                    .ok()
                    .map(|value| (name.to_uppercase(), value))
            })
            .collect()
    };
    let system = read(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    let user = read(HKEY_CURRENT_USER, "Environment");
    command.envs(merge(&inherited, &system, &user));
}

fn expand(value: &str, env: &Environment) -> String {
    let mut result = value.to_string();
    // 有界展开支持 %NVM_HOME% 等间接引用，也避免循环引用卡住探测。
    for _ in 0..8 {
        let mut next = String::new();
        let mut rest = result.as_str();
        while let Some(start) = rest.find('%') {
            next.push_str(&rest[..start]);
            let tail = &rest[start + 1..];
            let Some(end) = tail.find('%') else {
                next.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let token = &rest[start..start + end + 2];
            next.push_str(
                env.get(&tail[..end].to_uppercase())
                    .map(String::as_str)
                    .unwrap_or(token),
            );
            rest = &tail[end + 1..];
        }
        next.push_str(rest);
        if next == result {
            break;
        }
        result = next;
    }
    result
}

fn merge(inherited: &Environment, system: &Environment, user: &Environment) -> Environment {
    let mut env = inherited.clone();
    env.extend(system.clone());
    env.extend(user.clone());
    // PATH 内的 %PATH% 只能引用原进程值，不能递归展开自身。
    env.remove("PATH");
    if let Some(path) = inherited.get("PATH") {
        env.insert("PATH".into(), path.clone());
    }
    // 新系统 PATH + 用户 PATH 优先，保留开发启动器注入的进程路径作为兜底。
    let paths: Vec<_> = [system, user, inherited]
        .into_iter()
        .filter_map(|source| source.get("PATH"))
        .filter(|path| !path.is_empty())
        .map(|path| expand(path, &env))
        .collect();
    let expanded: Environment = env
        .iter()
        .map(|(key, value)| (key.clone(), expand(value, &env)))
        .collect();
    env = expanded;
    if !paths.is_empty() {
        env.insert("PATH".into(), paths.join(";"));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env(values: &[(&str, &str)]) -> Environment {
        values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn fresh_paths_and_manager_variables_override_stale_environment() {
        let old = env(&[
            ("PATH", "C:\\launcher"),
            ("NVM_HOME", "C:\\old"),
            ("USERPROFILE", "C:\\Users\\test"),
        ]);
        let system = env(&[("PATH", "C:\\system")]);
        let user = env(&[
            ("PATH", "%nvm_home%;%USERPROFILE%\\.cargo\\bin"),
            ("NVM_HOME", "C:\\new"),
        ]);
        let result = merge(&old, &system, &user);
        assert_eq!(
            result["PATH"],
            "C:\\system;C:\\new;C:\\Users\\test\\.cargo\\bin;C:\\launcher"
        );
        assert_eq!(result["NVM_HOME"], "C:\\new");
    }

    #[test]
    fn unavailable_registry_preserves_inherited_environment() {
        let old = env(&[("PATH", "C:\\bin")]);
        assert_eq!(merge(&old, &Environment::new(), &Environment::new()), old);
    }

    #[test]
    fn expansion_handles_unknown_nested_and_cyclic_references() {
        let vars = env(&[("A", "%B%"), ("B", "C:\\bin"), ("LOOP", "%LOOP%")]);
        assert_eq!(expand("%a%", &vars), "C:\\bin");
        assert_eq!(expand("%MISSING%", &vars), "%MISSING%");
        assert_eq!(expand("%LOOP%", &vars), "%LOOP%");
    }
}
