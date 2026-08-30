# RepoMeow MCP

RepoMeow 通过独立的 stdio MCP Server 对外提供能力。首版只包含代码提交和 Wiki 目录查询。

## 构建和运行

```powershell
pnpm mcp:build
```

产物位置：

```text
src-tauri/target/release/repomeow-mcp.exe
```

本地开发运行：

```powershell
pnpm mcp:dev
```

stdio 是 MCP 协议通道，服务运行时不要向 stdout 输出普通日志；错误日志只写入 stderr。

## MCP 客户端配置示例

请将 `command` 替换为本机实际的绝对路径：

```json
{
  "mcpServers": {
    "repomeow": {
      "command": "D:\\code\\project-dev\\src-tauri\\target\\release\\repomeow-mcp.exe"
    }
  }
}
```

如果需要使用非默认数据目录，可设置 `REPOMEOW_DATA_DIR`。它必须直接指向包含 `wiki/` 的 RepoMeow 数据目录；不设置时使用 `~/.repomeow`。

```json
{
  "mcpServers": {
    "repomeow": {
      "command": "D:\\path\\to\\repomeow-mcp.exe",
      "env": {
        "REPOMEOW_DATA_DIR": "D:\\repomeow-data"
      }
    }
  }
}
```

## 工具

### `commit_code`

在指定 Git 仓库中创建提交。

参数：

- `directory`：Git 仓库目录。
- `message`：非空提交信息。
- `files`：可选的仓库相对路径数组。
  - 不传：提交全部已跟踪和未跟踪变更。
  - 传入：仅提交指定路径。

示例：

```json
{
  "directory": "D:\\code\\my-project",
  "message": "feat: 增加用户搜索功能",
  "files": ["src/search.ts", "src/search.test.ts"]
}
```

文件参数只接受仓库相对路径，拒绝绝对路径和包含 `..` 的路径。返回提交哈希、短哈希、当前分支和实际提交文件列表。

### `get_wiki_directory`

获取指定项目已生成完成的 Wiki 信息。

参数：

- `projectDirectory`：RepoMeow 登记项目时使用的目录。

成功时返回：

- `projectDirectory`
- `wikiDirectory`
- `metaPath`
- `meta`：解析后的完整 `meta.json`

以下情况返回 `wiki_not_generated` 工具错误：

- Wiki 目录或 `meta.json` 不存在；
- `meta.json.status` 不是 `completed`。

`meta.json` 无法解析时返回 `wiki_meta_invalid`。
