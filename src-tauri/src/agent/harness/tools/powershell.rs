//! powershell 工具:对齐 `packages/coding-agent/src/core/tools/powershell.ts`。
//!
//! 完全复用 bash 工具管线(schema/timeout/流式输出/截断/临时文件),差异仅三点:
//! env 须以 [`ShellKind::PowerShell`](crate::agent::harness::env::ShellKind) 构造
//! (pwsh 优先、固定 `-NoProfile -NonInteractive -ExecutionPolicy Bypass`);
//! 每条命令前注入 Console UTF-8 编码前缀(对齐 `UTF8_OUTPUT_PREFIX`);
//! 仅 Windows 可用。

use std::sync::Arc;

use crate::agent::harness::env::ShellKind;
use crate::agent::harness::types::ExecutionEnv;
use crate::agent::types::AgentTool;
use crate::agent::harness::tools::bash::{create_bash_tool, BashToolOptions};

/// PowerShell 命令前缀:强制 Console 输出编码为 UTF-8(对齐蓝本
/// `UTF8_OUTPUT_PREFIX`;失败静默忽略,兼容受限主机)。
pub const UTF8_OUTPUT_PREFIX: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";

/// powershell 工具描述(对齐蓝本经 ShellToolConfig 派生的措辞)。
fn powershell_tool_description() -> String {
    format!(
        "Execute a PowerShell command in the current working directory. Returns stdout and stderr. Output is truncated to last {} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.",
        crate::agent::harness::utils::truncate::DEFAULT_MAX_LINES,
        crate::agent::harness::utils::truncate::DEFAULT_MAX_BYTES / 1024
    )
}

/// 创建 powershell 工具。
///
/// `env` 必须以 `TokioEnvOptions { shell_kind: ShellKind::PowerShell, .. }` 构造
/// (与蓝本 `createLocalPowerShellOperations` 在构造期绑定 PowerShell 一致);
/// 传入 bash env 时 PowerShell 语法会在 bash 下执行,属调用方契约错误。
/// 非 Windows 平台返回 `None`(蓝本同:工具仅 Windows 注册)。
pub fn create_powershell_tool(
    env: Arc<dyn ExecutionEnv>,
    options: Option<BashToolOptions>,
) -> Option<AgentTool> {
    if !cfg!(windows) {
        return None;
    }
    let options = options.unwrap_or_default();
    if options.command_prefix.is_some() {
        // command_prefix 在此工具中保留给 UTF-8 前缀,不支持外部覆写。
        return None;
    }
    let mut tool = create_bash_tool(
        env,
        Some(BashToolOptions {
            command_prefix: Some(UTF8_OUTPUT_PREFIX.to_string()),
            ..options
        }),
    );
    tool.name = "powershell".to_string();
    tool.label = "powershell".to_string();
    tool.description = powershell_tool_description();
    Some(tool)
}

/// 便捷构造:以 PowerShell shell 种类克隆 cwd 构造 env 并创建工具。
pub fn create_local_powershell_tool(
    cwd: impl Into<std::path::PathBuf>,
    options: Option<BashToolOptions>,
) -> Option<AgentTool> {
    let env: Arc<dyn ExecutionEnv> = Arc::new(crate::agent::harness::env::TokioEnv::with_options(
        crate::agent::harness::env::TokioEnvOptions {
            cwd: cwd.into(),
            shell_kind: ShellKind::PowerShell,
            shell_path: None,
            shell_env: None,
        },
    ));
    create_powershell_tool(env, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{AgentToolResult, TextOrImageContent};

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                TextOrImageContent::Text { text, .. } => Some(text.clone()),
                TextOrImageContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn executes_powershell_command_with_utf8_output() {
        let temp_dir = std::env::temp_dir().join(format!("repomeow-ps-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        // 非 ASCII 输出验证 UTF-8 编码前缀生效(中文 + 反引号转义)。
        let tool = create_local_powershell_tool(&temp_dir, None).unwrap();
        let result = (tool.execute)(
            "call-1".to_string(),
            serde_json::json!({ "command": "Write-Output 'héllo-喵'" }),
            None,
            None,
        )
        .await
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("héllo-喵"), "got: {text}");
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn reports_nonzero_exit_code() {
        let temp_dir = std::env::temp_dir().join(format!("repomeow-ps-exit-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let tool = create_local_powershell_tool(&temp_dir, None).unwrap();
        let error = (tool.execute)(
            "call-2".to_string(),
            serde_json::json!({ "command": "exit 3" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exited with code 3"), "{error}");
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn unavailable_off_windows() {
        // 非 Windows 平台构造器返回 None(与 cfg(windows) 测试互补,保证分支编译)。
        if cfg!(windows) {
            let tool = create_local_powershell_tool(std::env::temp_dir(), None);
            assert!(tool.is_some());
        } else {
            assert!(create_local_powershell_tool(std::env::temp_dir(), None).is_none());
        }
    }

    #[test]
    fn rejects_command_prefix_override() {
        if !cfg!(windows) {
            return;
        }
        let tool = create_local_powershell_tool(
            std::env::temp_dir(),
            Some(BashToolOptions {
                command_prefix: Some("echo hi".to_string()),
                ..Default::default()
            }),
        );
        assert!(tool.is_none());
    }
}
