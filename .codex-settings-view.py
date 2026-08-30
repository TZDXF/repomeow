from pathlib import Path
p=Path('src/views/Settings.vue')
s=p.read_text(encoding='utf-8')
s=s.replace('  CalendarClock,\n', '  CalendarClock,\n  Cable,\n')
s=s.replace('import ToolchainPanel from "@/components/settings/ToolchainPanel.vue";\n', 'import ToolchainPanel from "@/components/settings/ToolchainPanel.vue";\nimport McpSettings from "@/components/settings/McpSettings.vue";\n')
needle='''  {
    id: "archive",
    labelKey: "settings.categories.archive",
    icon: Archive,
    component: ArchiveSettings,
  },
'''
insert='''  {
    id: "archive",
    labelKey: "settings.categories.archive",
    icon: Archive,
    component: ArchiveSettings,
  },
  { id: "mcp", labelKey: "settings.categories.mcp", icon: Cable, component: McpSettings },
'''
if needle not in s: raise SystemExit('category target missing')
s=s.replace(needle,insert)
p.write_text(s,encoding='utf-8',newline='\n')
