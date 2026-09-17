# RepoMeow CLI

RepoMeow CLI 内置于主程序,不单独发布。首参数传入子命令即进入 CLI 模式(不启动桌面窗口),面向 AI agent 与脚本提供应用的管理能力:项目登记、Wiki、语义分析、标签、自定义命令、报告与调度、AI 配置/用量、提示词、Git 账号与项目文件。Git 与 Docker 等本机工具请直接使用其 CLI,RepoMeow CLI 不做包装。

## 输出约定

- 成功结果以 **JSON 写 stdout**,退出码 0;
- 失败以 `{"code","message","detail"}` **JSON 写 stderr**,退出码非 0;
- `REPOMEOW_DATA_DIR` 环境变量可覆盖数据目录(默认 `~/.repomeow`);
- CLI 与运行中的桌面应用共享同一 SQLite 与配置文件,数据立即可见;调度类变更(报告调度、系统调度)需应用重启后才被调度器感知。

## 配合 Skills 使用

打开「设置 → CLI」:

1. 复制 CLI 可执行文件路径(与桌面应用同一可执行文件);
2. 点击「导入到资源库」将内置技能 `repomeow` 导入资源库(SKILL.md 为入口,references/ 下按 project / sem / report / manage / settings 细分);
3. 在「项目 → AI 资源」将技能部署到各 agent 的 skills 目录(如 `.claude/skills`、`.agents/skills`)。

技能入口 SKILL.md 含输出约定与分组路由,细分用法在 references/ 下,agent 按需加载。重复导入会重建技能内容(同步最新可执行文件路径)。

## 命令

`<项目目录>` 指 RepoMeow 登记项目时使用的目录;未登记或已归档返回 `project_not_found`(项目自身的 add/get/archive/unarchive/delete 除外)。`sem` 的 `-d` 可传仓库内任意路径,操作以仓库根目录为准。

### `wiki`

```powershell
repomeow wiki dir -p <项目目录>
repomeow wiki pages -p <项目目录>
repomeow wiki read -p <项目目录> --page-id <页面id>
repomeow wiki config-get -p <项目目录>
repomeow wiki config-set -p <项目目录> [--model providerId/modelId] [--thinking high] [--concurrency 4]
repomeow wiki delete -p <项目目录>
```

未生成 Wiki 时返回 `wiki_not_generated`;页面不存在返回 `wiki_page_not_found`。`stale=true` 表示 Wiki 落后于最新代码。`config-set` 覆盖式写入,字段不传即跟随应用默认。

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
# 登记管理
repomeow project list [--archived]
repomeow project get -p <项目目录>
repomeow project add -p <项目目录> [--name 名称] [--description 描述]
repomeow project update -p <项目目录> [--name 名称] [--description 描述]
repomeow project archive -p <项目目录>
repomeow project unarchive -p <项目目录>
repomeow project delete -p <项目目录>
repomeow project set-favorite -p <项目目录> --enabled true|false
repomeow project set-auto-pull -p <项目目录> --enabled true|false
repomeow project set-wiki-auto-update -p <项目目录> --enabled true|false

# 数据查询
repomeow project read-file -p <项目目录> --path src/lib/ai.ts [--offset-line 1] [--max-lines 400]
repomeow project reports [-p <项目目录>] [--limit 10]
repomeow project commands -p <项目目录>
```

`add` 的 `--name` 缺省取目录 basename;`update` 缺省项保持原值;`delete` 仅移除登记与关联数据,不动磁盘目录。`read-file` 返回带 1-based 行号前缀的内容,`hasMore=true` 时用 `--offset-line <endLine+1>` 续读;拒绝越界与符号链接逃逸,二进制文件返回 `binary_file`。

### `report`

```powershell
repomeow report generate -p <项目目录1>,<项目目录2> --period-type daily|weekly [--date-from YYYY-MM-DD] [--date-to YYYY-MM-DD] [--author-mode all|me] [--language zh-CN|en-US]
repomeow report history-get --id <报告id>
repomeow report history-delete --id <报告id>
repomeow report schedules
repomeow report schedules-save --file <调度.json>
repomeow report system-schedules
repomeow report system-schedule-save --enabled true|false --interval-minutes 10
```

`generate` 同步等待(通常十几秒到一分钟),调用 AI 生成正文并写入报告历史,消耗用户在 RepoMeow 配置的 AI 额度;范围内无提交时不生成(`generated=false`)。`schedules-save` 全量覆盖(结构同 `schedules` 输出数组);调度变更需应用重启后生效。

### `tag`

```powershell
repomeow tag list
repomeow tag create --name 后端 [--color "#ff0000"]
repomeow tag update --id 1 --name 前端 [--color "#00ff00"]
repomeow tag delete --id 1
repomeow tag set-project -p <项目目录> --tag-ids 1,2
```

### `script`

```powershell
repomeow script list -p <项目目录>
repomeow script create -p <项目目录> --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow script update --id 1 --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow script delete --id 1
```

### `pin` / `hidden`

```powershell
repomeow pin list [-p <项目目录>]
repomeow pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned true|false [--label 名] [--command 内容] [--cwd 目录]
repomeow hidden list -p <项目目录>
repomeow hidden set -p <项目目录> --kind <类型> --target-key <标识> --hidden true|false
```

pin 的 kind:`packageScript`/`composeFile`/`composeService`/`customCommand`/`javaBuild`;hidden 的 kind:`packageFile`/`packageScript`/`composeFile`/`javaBuild`。

### `ai`

```powershell
repomeow ai config-get
repomeow ai config-save --file <配置.json>
repomeow ai builtin-providers
repomeow ai models
repomeow ai test
repomeow ai usage-summary
repomeow ai usage-log [--offset 0] [--limit 50] [--task-type report]
repomeow ai usage-clear
```

`config-get` 输出含 apiKey 明文,注意保密;`config-save` 全量覆盖(原子写 + 引用归一化)。

### `prompt`

```powershell
repomeow prompt get
repomeow prompt default
repomeow prompt set [--commit-file f.md] [--report-file f.md] [--report-weekly-file f.md] [--clear]
```

`set` 从文件读入内容;未指定项保持现状,`--clear` 将未指定项恢复内置默认。

### `account`

```powershell
repomeow account list
repomeow account add --provider github|gitee|gitlab --label 备注 --token <令牌> [--base-url <gitlab实例地址>]
repomeow account update --id 1 --label 备注 [--token <新令牌>] [--base-url <地址>]
repomeow account remove --id 1
```

`add`/`update` 先调平台 API 验证 token,失败不落库;token 输出只回显预览。

### `file`

```powershell
repomeow file list -p <项目目录> [--dir src/components]
repomeow file search -p <项目目录> -q "mod.rs" [--limit 20]
repomeow file search-text -p <项目目录> -q "TODO" [--include "src/**"] [--exclude "*.lock"] [--case-sensitive] [--whole-word] [--regex]
repomeow file save -p <项目目录> --path docs/note.md (--content "内容" | --content-file ./draft.md)
```

`save` 创建或覆盖项目内文本文件(上限 512KB,父目录必须已存在;拒绝绝对路径与 `..`)。

## 开发

```powershell
pnpm cli:dev -- project list
```

以 CLI 模式运行主程序(cargo run,不开窗口),`--` 后为 CLI 参数。