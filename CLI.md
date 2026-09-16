# RepoMeow CLI

RepoMeow CLI 内置于主程序,不单独发布。首参数传入子命令即进入 CLI 模式(不启动桌面窗口),面向 AI agent 与脚本提供 Git、Wiki、语义分析、项目数据与报告能力。

## 输出约定

- 成功结果以 **JSON 写 stdout**,退出码 0;
- 失败以 `{"code","message","detail"}` **JSON 写 stderr**,退出码非 0;
- `REPOMEOW_DATA_DIR` 环境变量可覆盖数据目录(默认 `~/.repomeow`)。

## 配合 Skills 使用

打开「设置 → CLI」:

1. 复制 CLI 可执行文件路径(与桌面应用同一可执行文件);
2. 点击「导入到资源库」将内置技能 `repomeow` 导入资源库(SKILL.md 为入口,references/ 下按 git / project / sem / report 细分);
3. 在「项目 → AI 资源」将技能部署到各 agent 的 skills 目录(如 `.claude/skills`、`.agents/skills`)。

技能入口 SKILL.md 含输出约定与分组路由,细分用法在 references/ 下,agent 按需加载。重复导入会重建技能内容(同步最新可执行文件路径)。

## 命令

### `git`

```powershell
repomeow git status -d <仓库目录>
repomeow git commit -d <仓库目录> -m "feat: 提交信息" [--files src/a.ts,src/b.ts]
```

- `status`:当前分支、与上游领先/落后、暂存/未暂存/未跟踪/冲突文件数、最后抓取与提交时间;非 git 目录返回 `isRepo=false`。
- `commit`:`--files` 省略时提交全部变更(含未跟踪文件);传入时仅提交指定的仓库相对路径(拒绝绝对路径与 `..`)。返回提交哈希、短哈希、分支与实际提交文件列表。

### `wiki`

```powershell
repomeow wiki dir -p <项目目录>
repomeow wiki pages -p <项目目录>
repomeow wiki read -p <项目目录> --page-id <页面id>
```

未生成 Wiki 时返回 `wiki_not_generated`;页面不存在返回 `wiki_page_not_found`。`stale=true` 表示 Wiki 落后于最新代码。

### `sem`

```powershell
repomeow sem find -d <仓库目录> -q <关键词>
repomeow sem context -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts] [--budget 2000] [--hops 1]
repomeow sem relations -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts]
repomeow sem diff -d <仓库目录>
```

实体参数含 `::` 时按 entityId 精确匹配;重名时用 `--file-path` 消歧。

### `project`

```powershell
repomeow project read-file -p <项目目录> --path src/lib/ai.ts [--offset-line 1] [--max-lines 400]
repomeow project reports [-p <项目目录>] [--limit 10]
repomeow project commands -p <项目目录>
```

`read-file` 返回带 1-based 行号前缀的内容,`hasMore=true` 时用 `--offset-line <endLine+1>` 续读;拒绝越界与符号链接逃逸,二进制文件返回 `binary_file`。项目未登记或已归档时返回 `project_not_found`。

### `report`

```powershell
repomeow report generate -p <项目目录1>,<项目目录2> --period-type daily|weekly [--date-from YYYY-MM-DD] [--date-to YYYY-MM-DD] [--author-mode all|me] [--language zh-CN|en-US]
```

同步等待(通常十几秒到一分钟),调用 AI 生成正文并写入报告历史,消耗用户在 RepoMeow 配置的 AI 额度;范围内无提交时不生成(`generated=false`)。

## 开发

```powershell
pnpm cli:dev -- git status -d .
```

以 CLI 模式运行主程序(cargo run,不开窗口),`--` 后为 CLI 参数。
