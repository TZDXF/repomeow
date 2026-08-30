from pathlib import Path
p=Path('src/i18n/locales/zh-CN.ts')
s=p.read_text(encoding='utf-8')
s=s.replace('      archive: "归档项目",\n      mcp: "MCP",\n      archiveConfirm:', '      archive: "归档项目",\n      archiveConfirm:', 1)
p.write_text(s,encoding='utf-8',newline='\n')
