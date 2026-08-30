from pathlib import Path
p=Path('AGENTS.md')
s=p.read_text(encoding='utf-8')
s=s.replace('  main.rs             二进制入口(实际逻辑在 lib.rs 的 run())\n', '  main.rs             二进制入口(--mcp 进入内置 stdio MCP,否则逻辑在 lib.rs 的 run())\n')
s=s.replace('                      / agent(ACP 客户端,本地 coding agent 会话)/ window / usage\n', '                      / agent(ACP 客户端,本地 coding agent 会话)/ window / usage / mcp\n')
s=s.replace('                      commands::<域>::* 路径稳定;仅测试引用的 re-export 须 #[cfg(test)] 门控)\n', '                      commands::<域>::* 路径稳定;仅测试引用的 re-export 须 #[cfg(test)] 门控)\n  mcp/                 内置 stdio MCP Server(--mcp 模式),按 settings.json 工具组开关过滤工具\n')
s=s.replace(' / window / usage / semantic`。', ' / window / usage / semantic / mcp`。')
marker='9. **sem 语义分析 Sidecar**:'
pos=s.find(marker)
if pos == -1: raise SystemExit('sem marker missing')
line_end=s.find('\n',pos)
sem_line=s[pos:line_end]
addition='\n10. **内置 MCP Server**:`main.rs` 在普通 Tauri 初始化前识别 `--mcp`,命中后复用主程序二进制运行 `src-tauri/src/mcp/` 的 stdio 服务,不得恢复独立 MCP 二进制或 Sidecar。工具组开关 `mcpGitCommitEnabled` / `mcpWikiEnabled` 由前端设置页写入 `~/.repomeow/settings.json`,MCP 进程启动时读取并过滤工具,默认均关闭;配置变更在客户端重连后生效。\n'
s=s[:line_end]+addition+s[line_end+1:]
p.write_text(s,encoding='utf-8',newline='\n')
