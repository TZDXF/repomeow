from pathlib import Path
p = Path("src-tauri/src/mcp/mod.rs")
s = p.read_text(encoding="utf-8")
s = s.replace(
    '            GIT_COMMIT_ENABLED_KEY: "true",\n            WIKI_ENABLED_KEY: true,',
    '            "mcpGitCommitEnabled": "true",\n            "mcpWikiEnabled": true,',
)
p.write_text(s, encoding="utf-8", newline="\n")
