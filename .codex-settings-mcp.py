from pathlib import Path
p = Path('src/stores/settings.ts')
s = p.read_text(encoding='utf-8')
s = s.replace(
    '  const enableGhCli = ref(false);\n',
    '  const enableGhCli = ref(false);\n  /** MCP Git 提交工具组(默认关闭,外部客户端仅在显式授权后可见) */\n  const mcpGitCommitEnabled = ref(false);\n  /** MCP Wiki 查询工具组(默认关闭,避免默认暴露本地项目元数据) */\n  const mcpWikiEnabled = ref(false);\n',
)
s = s.replace(
    '        enableGhCli: "false",\n        worktreeDirTemplate:',
    '        enableGhCli: "false",\n        mcpGitCommitEnabled: "false",\n        mcpWikiEnabled: "false",\n        worktreeDirTemplate:',
)
s = s.replace(
    '    // worktree 默认目录模板:自由文本,trim 后非空才采用\n',
    '    // MCP 工具组均为显式 opt-in；Rust MCP 进程从同一 settings.json 读取这两个字符串键\n    const savedMcpGitCommit = await fileStore.get<string>("mcpGitCommitEnabled");\n    if (savedMcpGitCommit === "true" || savedMcpGitCommit === "false") {\n      mcpGitCommitEnabled.value = savedMcpGitCommit === "true";\n    }\n    const savedMcpWiki = await fileStore.get<string>("mcpWikiEnabled");\n    if (savedMcpWiki === "true" || savedMcpWiki === "false") {\n      mcpWikiEnabled.value = savedMcpWiki === "true";\n    }\n    // worktree 默认目录模板:自由文本,trim 后非空才采用\n',
)
s = s.replace(
    '  async function setWorktreeDirTemplate(value: string) {\n',
    '  async function setMcpGitCommitEnabled(value: boolean) {\n    mcpGitCommitEnabled.value = value;\n    await persist("mcpGitCommitEnabled", String(value));\n  }\n\n  async function setMcpWikiEnabled(value: boolean) {\n    mcpWikiEnabled.value = value;\n    await persist("mcpWikiEnabled", String(value));\n  }\n\n  async function setWorktreeDirTemplate(value: string) {\n',
)
s = s.replace(
    '    enableGhCli,\n    worktreeDirTemplate,\n',
    '    enableGhCli,\n    mcpGitCommitEnabled,\n    mcpWikiEnabled,\n    worktreeDirTemplate,\n',
)
s = s.replace(
    '    setEnableGhCli,\n    setWorktreeDirTemplate,\n',
    '    setEnableGhCli,\n    setMcpGitCommitEnabled,\n    setMcpWikiEnabled,\n    setWorktreeDirTemplate,\n',
)
p.write_text(s, encoding='utf-8', newline='\n')
