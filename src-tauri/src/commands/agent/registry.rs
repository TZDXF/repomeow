use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult, ErrorCode};

/// 分发方式:npx 包(需 Node)或原生二进制
enum AgentKind {
    Npx {
        pkg: &'static str,
        args: &'static [&'static str],
    },
    Binary {
        cmd: &'static str,
        args: &'static [&'static str],
    },
}

struct AgentDef {
    id: &'static str,
    name: &'static str,
    kind: AgentKind,
    /// 未安装/未登录时的指引(设置页展示)
    login_hint: &'static str,
}

static AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        name: "Claude Code",
        kind: AgentKind::Npx {
            pkg: "@agentclientprotocol/claude-agent-acp",
            args: &[],
        },
        login_hint: "终端运行 claude 并按提示登录(Anthropic 账号)",
    },
    AgentDef {
        id: "codex",
        name: "Codex",
        kind: AgentKind::Npx {
            pkg: "@agentclientprotocol/codex-acp",
            args: &[],
        },
        login_hint: "终端运行 codex login 登录(OpenAI 账号)",
    },
    AgentDef {
        id: "gemini",
        name: "Gemini CLI",
        kind: AgentKind::Npx {
            pkg: "@google/gemini-cli",
            args: &["--acp"],
        },
        login_hint: "终端运行 gemini 并按提示登录(Google 账号)",
    },
    AgentDef {
        id: "copilot",
        name: "GitHub Copilot",
        kind: AgentKind::Npx {
            pkg: "@github/copilot",
            args: &["--acp"],
        },
        login_hint: "终端运行 copilot 并按提示登录(GitHub 账号)",
    },
    AgentDef {
        id: "grok",
        name: "Grok Build",
        kind: AgentKind::Npx {
            pkg: "@xai-official/grok",
            args: &["agent", "stdio"],
        },
        login_hint: "终端运行 grok 并配置 xAI 凭证",
    },
    AgentDef {
        id: "qwen",
        name: "Qwen Code",
        kind: AgentKind::Npx {
            pkg: "@qwen-code/qwen-code",
            args: &["--acp"],
        },
        login_hint: "终端运行 qwen 并按提示登录",
    },
    AgentDef {
        id: "cline",
        name: "Cline",
        kind: AgentKind::Npx {
            pkg: "cline",
            args: &["--acp"],
        },
        login_hint: "终端运行 cline 并按提示登录",
    },
    AgentDef {
        id: "glm",
        name: "GLM",
        kind: AgentKind::Npx {
            pkg: "glm-acp-agent",
            args: &[],
        },
        login_hint: "配置 GLM Coding Plan 的 API Key 后使用(见 glm-acp-agent 文档)",
    },
    AgentDef {
        id: "pi",
        name: "Pi",
        kind: AgentKind::Npx {
            pkg: "pi-acp",
            args: &[],
        },
        login_hint: "先安装 pi(社区适配器 pi-acp,功能受限:无权限请求/图像)",
    },
    AgentDef {
        id: "opencode",
        name: "OpenCode",
        kind: AgentKind::Binary {
            cmd: "opencode",
            args: &["acp"],
        },
        login_hint: "安装 opencode 并完成登录(opencode.ai)",
    },
    AgentDef {
        id: "goose",
        name: "goose",
        kind: AgentKind::Binary {
            cmd: "goose",
            args: &["acp"],
        },
        login_hint: "安装 goose 并完成登录(block/goose)",
    },
    AgentDef {
        id: "cursor",
        name: "Cursor",
        kind: AgentKind::Binary {
            cmd: "cursor-agent",
            args: &["acp"],
        },
        login_hint: "安装 cursor-agent(Cursor 付费计划)",
    },
    AgentDef {
        id: "kimi",
        name: "Kimi CLI",
        kind: AgentKind::Binary {
            cmd: "kimi",
            args: &["acp"],
        },
        login_hint: "安装 Kimi CLI 并完成登录(Moonshot 账号)",
    },
];

fn agent_kind_str(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Npx { .. } => "npx",
        AgentKind::Binary { .. } => "binary",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    installed: bool,
    /// 探测到的可执行路径(npx 类为 npx 路径);未安装为 None
    detail: Option<String>,
    login_hint: &'static str,
}

/// 精选 agent 清单 + 安装探测(npx 类探测 node/npx,二进制类探测命令本身)
pub fn list_agents() -> Vec<AgentInfo> {
    AGENTS
        .iter()
        .map(|def| {
            let (installed, detail) = match &def.kind {
                AgentKind::Npx { .. } => match (which::which("node"), which::which("npx")) {
                    (Ok(_), Ok(p)) => (true, Some(p.display().to_string())),
                    _ => (false, None),
                },
                AgentKind::Binary { cmd, .. } => match which::which(cmd) {
                    Ok(p) => (true, Some(p.display().to_string())),
                    Err(_) => (false, None),
                },
            };
            AgentInfo {
                id: def.id,
                name: def.name,
                kind: agent_kind_str(&def.kind),
                installed,
                detail,
                login_hint: def.login_hint,
            }
        })
        .collect()
}

/// 解析启动命令:精选 id 或自定义命令行 → (可执行路径, 参数, 展示名)
pub(super) fn resolve_spawn(
    agent_id: Option<String>,
    custom_command: Option<String>,
) -> AppResult<(PathBuf, Vec<String>, String)> {
    if let Some(cmdline) = custom_command {
        let tokens = parse_command_line(&cmdline);
        let (program, args) = tokens
            .split_first()
            .ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, "自定义命令为空"))?;
        let program_path = resolve_program(program)?;
        return Ok((program_path, args.to_vec(), program.clone()));
    }
    let id =
        agent_id.ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, "未指定 agent"))?;
    let def = AGENTS
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, format!("未知 agent: {id}")))?;
    match &def.kind {
        AgentKind::Npx { pkg, args } => {
            let npx = which::which("npx").map_err(|_| {
                AppError::coded(
                    ErrorCode::AgentNotDetected,
                    format!("{} 需要 Node.js(npx)", def.name),
                )
            })?;
            let mut full = vec!["-y".to_string(), (*pkg).to_string()];
            full.extend(args.iter().map(|s| s.to_string()));
            Ok((npx, full, def.name.to_string()))
        }
        AgentKind::Binary { cmd, args } => {
            let path = resolve_program(cmd).map_err(|_| {
                AppError::coded(ErrorCode::AgentNotDetected, format!("未检测到 {cmd} 命令"))
            })?;
            Ok((
                path,
                args.iter().map(|s| s.to_string()).collect(),
                def.name.to_string(),
            ))
        }
    }
}

fn resolve_program(program: &str) -> AppResult<PathBuf> {
    if program.contains('/') || program.contains('\\') || Path::new(program).is_absolute() {
        Ok(PathBuf::from(program))
    } else {
        which::which(program).map_err(|_| {
            AppError::coded(
                ErrorCode::AgentNotDetected,
                format!("未找到命令: {program}"),
            )
        })
    }
}

/// 简单命令行分词:空白分隔,双引号内保留空格(不含转义序列)
pub(super) fn parse_command_line(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}
