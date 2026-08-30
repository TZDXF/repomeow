from pathlib import Path
entries = {
    'src/i18n/locales/zh-CN.ts': {
        'category': '      mcp: "MCP",\n',
        'block': '''    mcp: {
      title: "MCP",
      description: "将 RepoMeow 的内置能力提供给 Codex、Claude Desktop 等 MCP 客户端",
      toolGroups: "工具组",
      toolGroupsHint: "仅已开启的工具组会对外可见;所有工具组默认关闭",
      reconnectHint: "开关在新的 MCP 连接中生效;修改后请在 MCP 客户端断开并重新连接。",
      git: {
        title: "Git 提交",
        description: "允许 MCP 客户端提交指定仓库的全部变更或所选文件",
      },
      wiki: {
        title: "Wiki 查询",
        description: "允许 MCP 客户端获取已生成完成的 Wiki 目录与 meta.json",
      },
      configuration: "如何配置",
      stepEnable: "开启需要使用的工具组。",
      stepCopy: "复制下方配置,粘贴到 MCP 客户端的服务器配置中。",
      stepReconnect: "保存配置后重启或重新连接 MCP 客户端。",
      copyConfig: "复制配置",
      loadingConfig: "正在读取 RepoMeow 程序路径...",
      configUnavailable: "无法读取 MCP 配置,请重新打开设置页后重试。",
      builtinHint: "MCP 服务内置于 RepoMeow 主程序,客户端通过 --mcp 参数启动,无需单独下载或发布 MCP 可执行文件。",
    },
''',
    },
    'src/i18n/locales/en-US.ts': {
        'category': '      mcp: "MCP",\n',
        'block': '''    mcp: {
      title: "MCP",
      description: "Expose built-in RepoMeow capabilities to MCP clients such as Codex and Claude Desktop",
      toolGroups: "Tool groups",
      toolGroupsHint: "Only enabled tool groups are visible externally; all groups are disabled by default",
      reconnectHint: "Changes apply to new MCP connections. Disconnect and reconnect your MCP client after changing a switch.",
      git: {
        title: "Git commits",
        description: "Allow MCP clients to commit all changes or selected files in a repository",
      },
      wiki: {
        title: "Wiki lookup",
        description: "Allow MCP clients to get the directory and meta.json of a completed Wiki",
      },
      configuration: "How to configure",
      stepEnable: "Enable the tool groups you want to use.",
      stepCopy: "Copy the configuration below into your MCP client's server configuration.",
      stepReconnect: "Restart or reconnect the MCP client after saving the configuration.",
      copyConfig: "Copy config",
      loadingConfig: "Reading the RepoMeow executable path...",
      configUnavailable: "Unable to read the MCP configuration. Reopen Settings and try again.",
      builtinHint: "The MCP server is built into the main RepoMeow executable. Clients start it with --mcp; no separate MCP executable needs to be downloaded or distributed.",
    },
''',
    },
}
for file, values in entries.items():
    p=Path(file)
    s=p.read_text(encoding='utf-8')
    s=s.replace('      archive: "归档项目",\n' if 'zh-CN' in file else '      archive: "Archived projects",\n',
                ('      archive: "归档项目",\n' if 'zh-CN' in file else '      archive: "Archived projects",\n') + values['category'])
    marker='    devEnv: {\n'
    if marker not in s: raise SystemExit(f'marker missing: {file}')
    s=s.replace(marker, values['block'] + marker, 1)
    p.write_text(s,encoding='utf-8',newline='\n')
