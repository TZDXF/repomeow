//! 终端子进程的环境变量刷新(仅 Windows)。
//!
//! 背景:Windows 进程的环境块是启动那一刻从父进程复制的快照,运行期间
//! 不会感知「系统属性 → 环境变量」/ setx 的修改——那只是写注册表并广播
//! WM_SETTINGCHANGE,已运行进程不会自行重读。应用长时间驻留(托盘常驻),
//! 用户改完环境变量再执行自定义命令/npm 脚本,子进程继承的仍是应用启动
//! 时的旧快照,此前只能重启应用解决。
//!
//! 方案:每次拉起子进程前实时读注册表最新值,以「逐键覆盖」方式注入,
//! 不清空原有环境:
//! - 系统环境(HKLM\...\Session Manager\Environment)先入,用户环境
//!   (HKCU\Environment)后入,同名键用户覆盖系统(与 Explorer 合成一致);
//! - PATH 特殊拼接为「系统 PATH;用户 PATH」;
//! - REG_EXPAND_SZ 先经 ExpandEnvironmentStringsW 展开 %VAR%(引用取当前
//!   进程环境,如 %USERPROFILE%);
//! - 仅覆盖注册表存在的键:父进程私有变量(dev 终端注入等)原样继承;
//!   注册表中已删除的变量不追溯清除,旧值仍随快照继承,重启应用后彻底生效;
//! - 注册表读取失败(键不存在等)返回空,行为退化为继承父进程环境。
//!
//! 交互式 PTY 会话无需接入:portable-pty 的 CommandBuilder 在 Windows 上
//! 构造时已内建同样逻辑(get_base_env 直读注册表),天然拿到最新值。

/// 注册表最新环境覆盖集,每次调用实时读取
#[cfg(windows)]
pub(crate) fn refreshed_overrides() -> Vec<(String, String)> {
    let system = read_registry_env(
        winreg::enums::HKEY_LOCAL_MACHINE,
        "System\\CurrentControlSet\\Control\\Session Manager\\Environment",
    );
    let user = read_registry_env(winreg::enums::HKEY_CURRENT_USER, "Environment");
    merge_registry_env(system, user)
}

/// 把注册表最新值覆盖到 std::process::Command 环境。须在应用显式注入
/// (如项目绑定的 JAVA_HOME)之前调用,保持显式注入的优先级更高。
#[cfg(windows)]
pub(crate) fn apply_to_command(cmd: &mut std::process::Command) {
    for (name, value) in refreshed_overrides() {
        cmd.env(name, value);
    }
}

#[cfg(not(windows))]
pub(crate) fn apply_to_command(_cmd: &mut std::process::Command) {}

/// 读取一个注册表环境键下的全部字符串值(REG_SZ / REG_EXPAND_SZ),
/// 其他类型(DWORD 等)不作为文本环境变量,跳过
#[cfg(windows)]
fn read_registry_env(hkey: winreg::HKEY, subkey: &str) -> Vec<(String, String)> {
    use winreg::enums::RegType;

    let Ok(root) = winreg::RegKey::predef(hkey).open_subkey(subkey) else {
        return Vec::new();
    };
    root.enum_values()
        .filter_map(Result::ok)
        .filter_map(|(name, value)| {
            // 注册表残留的 username 可能与真实用户名不一致,跳过防污染(对齐 portable-pty)
            if name.eq_ignore_ascii_case("username") {
                return None;
            }
            let text = match value.vtype {
                RegType::REG_EXPAND_SZ => expand_utf16(&value.bytes),
                RegType::REG_SZ => decode_utf16(&value.bytes),
                _ => return None,
            }?;
            Some((name, text))
        })
        .collect()
}

/// 系统环境与用户环境合成:同名键用户覆盖系统,PATH 拼接「系统;用户」。
/// 键名按 Windows 环境变量语义做大小写不敏感比较。
#[cfg(windows)]
fn merge_registry_env(
    system: Vec<(String, String)>,
    user: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut merged = system;
    for (name, value) in user {
        match merged.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&name)) {
            Some((k, v)) if k.eq_ignore_ascii_case("path") => {
                v.push(';');
                v.push_str(&value);
            }
            Some(slot) => slot.1 = value,
            None => merged.push((name, value)),
        }
    }
    merged
}

/// winreg 的字符串值字节(UTF-16LE,含结尾 NUL)解码为 Rust 字符串
#[cfg(windows)]
fn decode_utf16(bytes: &[u8]) -> Option<String> {
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    while let Some(&0) = units.last() {
        units.pop();
    }
    Some(String::from_utf16_lossy(&units))
}

/// 展开 REG_EXPAND_SZ 中的 %VAR% 引用(引用经当前进程环境解析);
/// 失败返回 None,由调用方跳过该变量
#[cfg(windows)]
fn expand_utf16(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    unsafe {
        let needed = ExpandEnvironmentStringsW(units.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u16; needed as usize];
        let written = ExpandEnvironmentStringsW(units.as_ptr(), buf.as_mut_ptr(), needed);
        if written == 0 || written as usize > buf.len() {
            return None;
        }
        buf.truncate(written as usize - 1); // 去掉结尾 NUL
        Some(String::from_utf16_lossy(&buf))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn merge_user_overrides_system_and_joins_path() {
        let system = vec![
            ("Path".to_string(), r"C:\Windows".to_string()),
            ("JAVA_HOME".to_string(), r"C:\old".to_string()),
        ];
        let user = vec![
            ("JAVA_HOME".to_string(), r"C:\new".to_string()),
            // 大小写不同也应识别为同一变量
            ("PATH".to_string(), r"C:\Users\me\bin".to_string()),
            ("M2_HOME".to_string(), r"D:\maven".to_string()),
        ];
        let merged = merge_registry_env(system, user);
        assert_eq!(merged.len(), 3);
        let path = merged
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("path"))
            .unwrap();
        assert_eq!(path.1, r"C:\Windows;C:\Users\me\bin");
        let java = merged
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("java_home"))
            .unwrap();
        assert_eq!(java.1, r"C:\new");
        assert!(merged.iter().any(|(k, _)| k == "M2_HOME"));
    }

    #[test]
    fn expands_percent_refs() {
        // %SystemRoot% 为系统必有变量,展开后不应残留百分号引用
        let bytes: Vec<u8> = "%SystemRoot%\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let expanded = expand_utf16(&bytes).unwrap();
        assert!(!expanded.contains('%'));
        assert!(expanded.to_ascii_uppercase().starts_with("C:\\"));
    }

    #[test]
    fn decode_strips_trailing_nul() {
        let bytes: Vec<u8> = "abc\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_utf16(&bytes).as_deref(), Some("abc"));
    }
}
