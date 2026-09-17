# 项目管理与 Wiki

通过 PATH 直接调用 `repomeow`。`<项目目录>` 指 RepoMeow 登记项目时使用的目录。

## 项目登记管理

```powershell
# 列出项目(默认未归档;--archived 列出已归档)
repomeow project list [--archived]

# 按目录查询单个项目(含已归档)
repomeow project get -p <项目目录>

# 登记项目(目录必须已存在;--name 缺省取目录名)
repomeow project add -p <项目目录> [--name 名称] [--description 描述]

# 更新名称/描述(缺省项保持原值)
repomeow project update -p <项目目录> [--name 名称] [--description 描述]

# 归档 / 恢复 / 删除(删除仅移除登记与关联数据,不动磁盘目录)
repomeow project archive -p <项目目录>
repomeow project unarchive -p <项目目录>
repomeow project delete -p <项目目录>

# 项目开关:收藏 / 跟踪更新(远端有更新自动快进) / Wiki 自动增量更新
repomeow project set-favorite -p <项目目录> --enabled true|false
repomeow project set-auto-pull -p <项目目录> --enabled true|false
repomeow project set-wiki-auto-update -p <项目目录> --enabled true|false
```

## Wiki 查询与管理

```powershell
# Wiki 目录与 meta.json 元数据
repomeow wiki dir -p <项目目录>

# Wiki 大纲:每页 id/标题/简介/分区/来源文件,stale=true 表示落后于最新代码
repomeow wiki pages -p <项目目录>

# 读某页正文(id 来自 pages;超长截断,truncated 标记)
repomeow wiki read -p <项目目录> --page-id <页面id>

# 生成配置查询/覆盖式写入(字段不传即跟随应用默认)
repomeow wiki config-get -p <项目目录>
repomeow wiki config-set -p <项目目录> [--model providerId/modelId] [--thinking high] [--concurrency 4]

# 删除已生成的 Wiki(整个目录;不影响项目登记)
repomeow wiki delete -p <项目目录>
```

未生成 Wiki 时查询类命令返回 `wiki_not_generated` 错误。建议流程:先 `pages` 拿大纲,再按需 `read` 具体页面。Wiki 的 AI 生成/更新仍在应用内或 `wiki_auto_update` 触发,CLI 不提供 generate 入口。

## 项目文件

```powershell
repomeow project read-file -p <项目目录> --path src/lib/ai.ts [--offset-line 1] [--max-lines 400]
```

返回带 1-based 行号前缀的内容;`hasMore=true` 时用 `--offset-line <endLine+1>` 续读。`--path` 必须是项目内相对路径(`/` 分隔),拒绝越界与符号链接逃逸;二进制文件返回 `binary_file` 错误。

## 报告历史与自定义命令(只读速查)

```powershell
# 报告历史(-p 可省略,省略时列出全部项目)
repomeow project reports [-p <项目目录>] [--limit 10]

# 项目登记的自定义命令(了解构建/运行方式的捷径)
repomeow project commands -p <项目目录>
```

项目未在 RepoMeow 登记或已归档时返回 `project_not_found` 错误(`project get`/`archive`/`unarchive`/`delete` 对已归档项目仍可操作)。