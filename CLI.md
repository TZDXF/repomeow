# RepoMeow CLI

`repomeow-cli` 是随桌面安装包提供的独立 console 程序，复用现有命令逻辑，无需初始化 Tauri 或 WebView。普通命令不打开窗口；`terminal` 命令会启动或唤醒同目录的桌面程序。原 `repomeow.exe` 保留 GUI 启动及旧 CLI 兼容入口，脚本和 AI 应使用 `repomeow-cli`。Git 与 Docker 请直接调用其 CLI。

## 输出约定

- 命令成功结果以 **JSON 写 stdout**,退出码 0;`--help` / `--version` 例外,输出纯文本;
- 失败以 `{"code","message","detail"}` **JSON 写 stderr**：业务/运行时错误为 1，参数错误为 2，失败 stdout 为空；
- `REPOMEOW_DATA_DIR` 环境变量可覆盖数据目录(默认 `~/.repomeow`);
- CLI 不读取 stdin，可关闭或置空；原生输出始终为 UTF-8，不需要 TTY 或 PTY；
- GUI 与 CLI 使用同一个 `REPOMEOW_DATA_DIR`（均须在启动前设置）时，CLI 写入完成后 GUI 自动重新读取相关数据，后端每 500ms 检查并广播，正常约 1 秒加查询耗时；调度器同时被唤醒。写入仍在进行时延后刷新，休眠或 WebView 暂停会延长可见刷新的时限；
- 未保存的 GUI 表单草稿予以保留并提示外部变化；保存全量配置时以当前编辑为准。

## 配合 Skills 使用

打开「设置 → CLI」:

1. Windows 点击「添加到用户 PATH」并重启终端和 AI 工具;macOS / Linux 手动将包含 `repomeow-cli` 可执行文件的目录加入 PATH;
2. 点击「导入到资源库」将内置技能 `repomeow` 导入资源库(SKILL.md 为入口,references/ 下按 project / sem / report / manage / settings 细分);
3. 在「项目 → AI 资源」将技能部署到各 agent 的 skills 目录(如 `.claude/skills`、`.agents/skills`)。

技能入口 SKILL.md 含输出约定与分组路由,细分用法在 references/ 下,agent 按需加载。技能通过 PATH 调用,不保存本机可执行文件路径;重复导入只刷新资源库中的技能文本,已部署副本需在「项目 → AI 资源」重新应用后同步。

## 安装与 AI 调用

桌面安装包同时提供 `repomeow.exe`（GUI）和 `repomeow-cli.exe`（console）。开发构建用 `cargo build --release --bins --manifest-path src-tauri/Cargo.toml`，打包用 `pnpm build:desktop`。`pnpm cli:dev -- project list` 运行 console 目标。Windows 在「设置 → CLI」添加用户 PATH，重启终端/AI 工具后执行 `Get-Command repomeow-cli` 与 `repomeow-cli --version` 核对路径和版本；旧技能需要重新导入并部署。macOS 的 console 位于 `RepoMeow.app/Contents/MacOS`；Linux 按实际包安装位置使用绝对路径或添加其所在目录到 PATH。

```powershell
$env:REPOMEOW_DATA_DIR = 'C:\RepoMeowTest\data'
repomeow-cli project list > projects.json 2> errors.json
$exitCode = $LASTEXITCODE
$data = repomeow-cli project list | ConvertFrom-Json
$raw = repomeow-cli project list
```

PowerShell 5.1 按 `[Console]::OutputEncoding` 解码原生输出，`>` 默认写 UTF-16；接收中文前设置 `[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)`。它还可能将原生 stderr 包装为 `NativeCommandError`。这是 shell 的转换行为；精确的 UTF-8 字节及错误 JSON 可用 Node.js 捕获，或使用 PowerShell 7.4+ 的原生重定向。`cmd /d /c "repomeow-cli project list >projects.json 2>errors.json"` 同样保留原始 UTF-8 字节。各 shell 均可正确等待并取得 0/1/2 退出码。

以下示例保存为 `.mjs` 后用 `node` 执行；Node/pnpm 也是项目现有的开发和回归环境。已安装的独立 CLI 运行无需 Node.js 或其他解释器。

```javascript
import { spawnSync } from 'node:child_process';

const p = spawnSync('repomeow-cli', ['project', 'list'], {
  stdio: ['ignore', 'pipe', 'pipe'], timeout: 60_000,
  windowsHide: true, // 默认不开 shell，stdout/stderr 保留原始 Buffer
});
if (p.error) throw p.error; // 包括未安装/未配置 PATH、超时
if (p.signal || p.status === null) throw new Error('CLI 未正常退出');
const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const stdout = decoder.decode(p.stdout);
const stderr = decoder.decode(p.stderr);
if (p.status !== 0) {
  const failure = JSON.parse(stderr);
  throw new Error(`exit=${p.status}: ${failure.code}: ${failure.message}`);
}
const projects = JSON.parse(stdout).projects;
```

完整写入/缓存清单、一致性规则和回归入口见 [同步设计记录](docs/TZD-85-console-sync.md)。

## 命令

`<项目目录>` 指项目目录;需要登记状态的命令在项目未登记或已归档时返回 `project_not_found`(`project get/archive/unarchive/delete` 可操作已归档项目)。`project read-file`、Wiki 与 `file` 命令直接按文件系统路径操作,不要求项目已登记;`terminal create` 及终端的 `-p` 过滤要求项目已登记且未归档。`sem` 的 `-d` 可传仓库内任意路径,操作以仓库根目录为准。

删除、清空、全量覆盖配置/调度、整本 Wiki 重生成、文件覆盖与终端输入/启停只应在用户明确要求的范围内执行;报告生成前确认日期与作者口径,避免错误消耗 AI 额度。AI 配置和账号 token 属于敏感信息,不要写入日志或提交到仓库。

### `wiki`

```powershell
repomeow-cli wiki dir -p <项目目录>
repomeow-cli wiki pages -p <项目目录>
repomeow-cli wiki read -p <项目目录> --page-id <页面id>
repomeow-cli wiki generate -p <项目目录> [--language zh-CN|en-US] [--concurrency 2]
repomeow-cli wiki update -p <项目目录> [--language zh-CN|en-US]
repomeow-cli wiki config-get -p <项目目录>
repomeow-cli wiki config-set -p <项目目录> [--model providerId/modelId] [--thinking high] [--concurrency 4]
repomeow-cli wiki delete -p <项目目录>
```

未生成 Wiki 时查询命令返回 `wiki_not_generated`;页面不存在返回 `wiki_page_not_found`。`stale=true` 表示 Wiki 落后于最新代码。`generate` 整本生成或重生成并覆盖现有页面,`update` 仅重生成受代码变更影响的页面;两者都会调用 AI、消耗额度并同步等待完成。`config-set` 覆盖式写入,字段不传即跟随应用默认。

### `sem`

```powershell
repomeow-cli sem find -d <仓库目录> -q <关键词>
repomeow-cli sem context -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts] [--budget 2000] [--hops 1]
repomeow-cli sem relations -d <仓库目录> -e <实体名或entityId> [--file-path src/a.ts]
repomeow-cli sem diff -d <仓库目录>
```

实体参数含 `::` 时按 entityId 精确匹配;重名时用 `--file-path` 消歧。

### `project`

```powershell
# 登记管理
repomeow-cli project list [--archived]
repomeow-cli project get -p <项目目录>
repomeow-cli project add -p <项目目录> [--name 名称] [--description 描述]
repomeow-cli project update -p <项目目录> [--name 名称] [--description 描述]
repomeow-cli project archive -p <项目目录>
repomeow-cli project unarchive -p <项目目录>
repomeow-cli project delete -p <项目目录>
repomeow-cli project set-favorite -p <项目目录> --enabled true|false
repomeow-cli project set-auto-pull -p <项目目录> --enabled true|false
repomeow-cli project set-wiki-auto-update -p <项目目录> --enabled true|false

# 数据查询
repomeow-cli project read-file -p <项目目录> --path src/lib/ai.ts [--offset-line 1] [--max-lines 400]
repomeow-cli project reports [-p <项目目录>] [--limit 10]
repomeow-cli project commands -p <项目目录>
```

`add` 的 `--name` 缺省取目录 basename;`update` 缺省项保持原值;`delete` 仅移除登记与关联数据,不动磁盘目录。`read-file` 返回带 1-based 行号前缀的内容,`hasMore=true` 时用 `--offset-line <endLine+1>` 续读;拒绝越界与符号链接逃逸,二进制文件返回 `binary_file`。读取基于前 512KB 预览,`previewTruncated=true` 时不能继续读取预览范围之外的行,`totalLines` 也仅统计预览内容。`project commands` 是 `script list` 的兼容速查入口,新脚本优先使用后者。

### `report`

```powershell
repomeow-cli report generate -p <项目目录1>,<项目目录2> --period-type daily|weekly [--date-from YYYY-MM-DD] [--date-to YYYY-MM-DD] [--author-mode all|me] [--language zh-CN|en-US]
repomeow-cli report history-get --id <报告id>
repomeow-cli report history-delete --id <报告id>
repomeow-cli report schedules
repomeow-cli report schedules-save --file <调度.json>
repomeow-cli report system-schedules
repomeow-cli report system-schedule-save --enabled true|false --interval-minutes 10
```

其中 `system-schedules` / `system-schedule-save` 管理的是 Git 后台检查,不是报告调度。

`generate` 同步等待(通常十几秒到一分钟),调用 AI 生成正文并写入报告历史,消耗用户在 RepoMeow 配置的 AI 额度;范围内无提交时不生成(`generated=false`)。`schedules-save` 全量覆盖(结构同 `schedules` 输出数组);同目录 GUI 的调度循环自动感知变更。

### `tag`

```powershell
repomeow-cli tag list
repomeow-cli tag create --name 后端 [--color "#ff0000"]
repomeow-cli tag update --id 1 --name 前端 [--color "#00ff00"]
repomeow-cli tag delete --id 1
repomeow-cli tag set-project -p <项目目录> --tag-ids 1,2
repomeow-cli tag set-project -p <项目目录> # 省略 --tag-ids 即清空绑定
```

### `script`

```powershell
repomeow-cli script list -p <项目目录>
repomeow-cli script create -p <项目目录> --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow-cli script update --id 1 --name dev --command "pnpm dev" [--description 描述] [--icon rocket]
repomeow-cli script delete --id 1
```

### `pin` / `hidden`

```powershell
repomeow-cli pin list [-p <项目目录>]
repomeow-cli pin set -p <项目目录> --kind <类型> --target-key <标识> --pinned true|false [--label 名] [--command 内容] [--cwd 目录]
repomeow-cli hidden list -p <项目目录>
repomeow-cli hidden set -p <项目目录> --kind <类型> --target-key <标识> --hidden true|false
```

pin 的 kind:`packageScript`/`composeFile`/`composeService`/`customCommand`/`javaBuild`;hidden 的 kind:`packageFile`/`packageScript`/`composeFile`/`javaBuild`。

### `ai`

```powershell
repomeow-cli ai config-get
repomeow-cli ai config-save --file <配置.json>
repomeow-cli ai builtin-providers
repomeow-cli ai models
repomeow-cli ai test
repomeow-cli ai usage-summary
repomeow-cli ai usage-log [--offset 0] [--limit 50] [--task-type report]
repomeow-cli ai usage-clear
```

`config-get` 输出含 apiKey 明文,注意保密;`config-save` 全量覆盖(原子写 + 引用归一化)。如需导出配置编辑,不要将临时 JSON 提交到仓库或粘贴到日志,使用后安全删除。

### `prompt`

```powershell
repomeow-cli prompt get
repomeow-cli prompt default
repomeow-cli prompt set [--commit-file f.md] [--report-file f.md] [--report-weekly-file f.md] [--clear]
```

`set` 从文件读入内容;未指定项保持现状,`--clear` 将未指定项恢复内置默认。

### `account`

```powershell
repomeow-cli account list
repomeow-cli account add --provider github|gitee|gitlab --label 备注 --token <令牌> [--base-url <gitlab实例地址>]
repomeow-cli account update --id 1 --label 备注 [--token <新令牌>] [--base-url <地址>]
repomeow-cli account remove --id 1
```

`add`/`update` 先调平台 API 验证 token,失败不落库;token 输出只回显预览。命令行 `--token` 可能进入 shell 历史或进程参数,执行时应注意终端与日志环境。

### `file`

```powershell
repomeow-cli file list -p <项目目录> [--dir src/components]
repomeow-cli file search -p <项目目录> -q "mod.rs" [--limit 20]
repomeow-cli file search-text -p <项目目录> -q "TODO" [--include "src/**"] [--exclude "*.lock"] [--case-sensitive] [--whole-word] [--regex]
repomeow-cli file save -p <项目目录> --path docs/note.md (--content "内容" | --content-file ./draft.md)
```

`save` 创建或覆盖项目内文本文件(上限 512KB,父目录必须已存在;拒绝绝对路径、`..`、符号链接/junction 逃逸及已有符号链接目标)。

### `terminal`

```powershell
repomeow-cli terminal list [-p <项目目录>]
repomeow-cli terminal output --id 1 [-p <项目目录>]
repomeow-cli terminal input --id 1 --text "y" [--enter] [-p <项目目录>]
repomeow-cli terminal create npm -p <项目目录> --name dev [--package .]
repomeow-cli terminal create custom -p <项目目录> --id 1
repomeow-cli terminal start --id 1 [-p <项目目录>]
repomeow-cli terminal stop --id 1 [-p <项目目录>]
repomeow-cli terminal restart --id 1 [-p <项目目录>]
```

终端命令通过桌面应用内的真实 TerminalManager 执行;桌面未运行时会自动启动,已运行时会唤醒并显示主窗口。`output` 读取当前输出缓冲;`input` 向运行中 Tab 写入 PTY 输入,`--enter` 追加回车,已结束 Tab 返回 `written=false`;`create` 只能引用已发现的 npm script 或已登记自定义命令,由桌面端校验并生成命令,不接受任意命令文本;重复创建同一 npm/custom 命令会复用并重启原 Tab(运行中也会先停止整棵进程树);`start` 仅恢复已停止/退出的 Tab,运行中不会重复启动;`restart` 总是重启;`input/start/restart` 仅允许 npm/custom 类型 Tab,不能操作 Shell/Agent/Docker Tab 绕过限制;`stop` 停止整棵进程树。Tab ID、原命令和工作目录在启停后保留。会话仅保存在桌面进程内存中,应用退出后清空。

## 开发

```powershell
pnpm cli:dev -- project list
```

运行独立 console 目标(cargo run --bin repomeow-cli,不开窗口),`--` 后为 CLI 参数。
