use serde_json::{json};
use crate::agent::types::{AgentTool};
use crate::commands::files::read_file_preview;
use crate::path_util::to_forward_slash_str;
use super::*;

// ── 文件读取 ─────────────────────────────────────────────────────────

pub(super) fn read_project_file_tool(ctx: &ChatToolContext) -> AgentTool {
    tool(
        "read_project_file",
        "读项目文件",
        "读取项目内单个文本文件的指定行区间(带 1-based 行号前缀)。sem_context 的结果不够、或需要查看完整文件/配置时使用;只读工具,路径必须是仓库内相对路径(/ 分隔)。参数:path(必填)仓库相对路径,如 src/lib/ai.ts;offset_line(可选)起始行,默认 1;max_lines(可选)最多返回行数,默认 400(上限 5000)。",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "仓库内相对路径(/ 分隔),如 src/lib/ai.ts。"
                },
                "offset_line": {
                    "type": "integer",
                    "description": "起始行(1-based),默认 1。"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "最多返回行数,默认 400,上限 5000。"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        false,
        {
            let root = ctx.work_dir();
            move |args, _on_update| {
                let root = root.clone();
                Box::pin(async move {
                    let rel_path = to_forward_slash_str(require_str(&args, "path")?.trim());
                    let offset_line = arg_u64_opt(&args, "offset_line")?.unwrap_or(1).max(1);
                    let max_lines = arg_u64_opt(&args, "max_lines")?
                        .unwrap_or(READ_FILE_DEFAULT_LINES)
                        .clamp(1, READ_FILE_MAX_LINES) as usize;
                    // read_file_preview 内部已做 canonicalize + 根目录前缀校验,
                    // 拒绝越界路径与符号链接逃逸。
                    let preview = read_file_preview(root.clone(), rel_path.clone())
                        .map_err(tool_err)?;
                    let Some(text) = preview.text else {
                        return text_result(format!(
                            "「{rel_path}」是二进制或非 UTF-8 文件,无法按行读取。"
                        ));
                    };
                    let lines: Vec<&str> = text.lines().collect();
                    let total = lines.len();
                    let start = ((offset_line as usize).saturating_sub(1)).min(total);
                    if start >= total {
                        return text_result(format!(
                            "文件共 {total} 行,offset_line={offset_line} 超出范围。"
                        ));
                    }
                    let end = (start + max_lines).min(total);
                    let body = lines[start..end]
                        .iter()
                        .enumerate()
                        .map(|(index, line)| format!("{}: {}", start + index + 1, line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut out = body;
                    if preview.truncated {
                        out.push_str("\n\n…(文件超过 512KB 预览上限,尾部已被截断)");
                    }
                    if end < total {
                        out.push_str(&format!(
                            "\n\n…(共 {total} 行,以上为第 {}~{} 行;继续读取请传 offset_line={})",
                            start + 1,
                            end,
                            end + 1
                        ));
                    }
                    text_result(truncate_bytes(out, TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}


