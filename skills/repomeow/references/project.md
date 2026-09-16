# Wiki 与项目查询

可执行文件:`{{REPOMEOW_CLI}}`(以 `repomeow` 代称)。以下命令均为只读;`<项目目录>` 指 RepoMeow 登记项目时使用的目录。

## Wiki

```powershell
# Wiki 目录与 meta.json 元数据
repomeow wiki dir -p <项目目录>

# Wiki 大纲:每页 id/标题/简介/分区/来源文件,stale=true 表示落后于最新代码
repomeow wiki pages -p <项目目录>

# 读某页正文(id 来自 pages;超长截断,truncated 标记)
repomeow wiki read -p <项目目录> --page-id <页面id>
```

未生成 Wiki 时返回 `wiki_not_generated` 错误。建议流程:先 `pages` 拿大纲,再按需 `read` 具体页面。

## 项目文件

```powershell
repomeow project read-file -p <项目目录> --path src/lib/ai.ts [--offset-line 1] [--max-lines 400]
```

返回带 1-based 行号前缀的内容;`hasMore=true` 时用 `--offset-line <endLine+1>` 续读。`--path` 必须是项目内相对路径(`/` 分隔),拒绝越界与符号链接逃逸;二进制文件返回 `binary_file` 错误。

## 报告历史与自定义命令

```powershell
# 报告历史(-p 可省略,省略时列出全部项目)
repomeow project reports [-p <项目目录>] [--limit 10]

# 项目登记的自定义命令(了解构建/运行方式的捷径)
repomeow project commands -p <项目目录>
```

项目未在 RepoMeow 登记或已归档时返回 `project_not_found` 错误。
