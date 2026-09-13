# RepoMeow MCP

RepoMeow 的 MCP Server 内置在主程序中，不单独发布。MCP 客户端启动 RepoMeow 主程序并附加 `--mcp` 参数建立 stdio 连接。

## 在应用中开启工具

打开 RepoMeow 的"设置 → MCP"按需开启工具组，默认全部关闭：

- **Git 提交**：`commit_code`
- **Wiki 查询**：`get_wiki_directory`

工具组配置写入 `~/.repomeow/settings.json`，由 MCP 进程启动时读取；修改开关后需在客户端断开并重连。

## 配置 MCP 客户端

"设置 → MCP"页面读取当前运行的 RepoMeow 主程序绝对路径，生成可直接复制的配置：

```json
{
  "mcpServers": {
    "repomeow": {
      "command": "C:\\path\\to\\RepoMeow.exe",
      "args": ["--mcp"]
    }
  }
}
```

粘贴到 Codex、Claude Desktop 或其他兼容 MCP 客户端的服务器配置后重启或重连客户端。

开发环境可执行：

```powershell
pnpm mcp:dev
```

实际运行 RepoMeow 主程序的 `--mcp` 模式，不启动桌面窗口。

## 工具

### `commit_code`

在指定 Git 仓库创建提交。

参数：

- `directory`：Git 仓库目录。
- `message`：非空提交信息。
- `files`：可选的仓库相对路径数组。不传：提交全部已跟踪和未跟踪变更；传入：仅提交指定路径。

示例：

```json
{
  "directory": "D:\\code\\my-project",
  "message": "feat: 增加用户搜索功能",
  "files": ["src/search.ts", "src/search.test.ts"]
}
```

`files` 只接受仓库相对路径，拒绝绝对路径与含 `..` 的路径。返回提交哈希、短哈希、当前分支和实际提交文件列表。

### `get_wiki_directory`

获取指定项目已生成的 Wiki 信息。

参数：`projectDirectory`（RepoMeow 登记项目时使用的目录）。

成功时返回 `projectDirectory`、`wikiDirectory`、`metaPath` 与 `meta`（解析后的完整 `meta.json`）。

Wiki 目录或 `meta.json` 不存在、或 `meta.json.status` 不是 `completed` 时，返回 `wiki_not_generated` 工具错误。

`meta.json` 无法解析时返回 `wiki_meta_invalid`。
