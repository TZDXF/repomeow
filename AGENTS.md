# AGENTS.md

本文件供 ZCode 代理快速了解本仓库的工作方式。只记录从代码中不易直接看出的约定。

## 项目概述

Tauri 2 + Vue 3 + TypeScript 桌面应用(项目名称 `RepoMeow`,中文名“喵库”):本地开发项目管理中心,功能包括项目登记/归档、npm scripts 与自定义命令执行、docker compose 服务管理、git 状态与写操作(提交/拉取/推送/分支切换)、README Markdown 渲染、标签、多编辑器打开。

- 包管理器:**pnpm**(有 `pnpm-lock.yaml` / `pnpm-workspace.yaml`,勿用 npm)。
- 主要平台:Windows(终端启动优先 `wt.exe`,失败回退 `cmd`);存在 macOS 交叉编译产物。
- 提交信息:中文 + conventional 前缀(`feat:` 等),单行详细描述,参考 `git log`。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `pnpm start` | `tauri dev`,完整桌面端开发(前端 + Rust) |
| `pnpm dev` | 仅 Vite 前端(固定端口 1420,strictPort) |
| `pnpm build` | `vue-tsc --noEmit && vite build`,**唯一的类型检查手段** |
| `pnpm build:desktop` | `tauri build` 打包 |
| `pnpm lint` | `oxlint .` 静态检查(correctness/suspicious/perf 报错,style 仅告警) |
| `pnpm lint:fix` | `oxlint . --fix` 自动修复可修问题 |
| `pnpm format` | `oxfmt --write src/` 格式化 src/ 下 TS/Vue |
| `pnpm format:check` | `oxfmt --check src/` 仅检查不改(CI 用) |

仓库**已配置 oxlint(静态检查)、oxfmt(代码格式化)与 vitest(单元测试,`vitest.config.ts` 仅覆盖 `src/**/*.test.ts`,node 环境、无 setup 文件)**;改动后建议 `pnpm lint` + `pnpm test:unit` + `pnpm build` 验证。Rust 侧用 `cargo check`(在 `src-tauri/` 下)。release 流程另有 `pnpm release:check / build / sign / latest / all / local`(`scripts/release/release.mjs`),由 CI 之外的本地调试用,不替代 GitHub Actions 上的 release.yml。

## 架构与分层

```
src/                  Vue 3 前端(<script setup> SFC)
  views/              页面:ProjectsHome / ProjectDetail / ProjectFiles / GitGraph
                      / ReportHistory / Settings / TrayPopup(系统托盘迷你弹窗)
  components/ui/      shadcn-vue(reka-ui)组件,勿手改生成文件风格
  components/{common,files,git,icons,java,markdown,open,project,report,scripts,settings,tags,update}/
                      业务组件;TitleBar 在 components/ 顶层
  composables/        useCollapsibleOpen 等小组件式组合式函数
  stores/             Pinia:projects / settings / tags / batch-report / jdk-install
                      / pins / project-assets / project-overview / update
  i18n/locales/       zh-CN.ts(默认)、en-US.ts(回退),两文件键必须对齐
  lib/                tauri.ts(前后端桥 cmd<T>)/ ai.ts / path.ts / diff.ts / diff-highlight.ts
                      / git-graph.ts / branch-tree.ts / favorites.ts / open-with.ts 等
                      单元测试与源文件同目录(lib/*.test.ts,stores/*.test.ts)
  router/             Vue Router(index.ts)
  styles/markdown/    base.css + index.css + themes/{default,github,notion,serif}.css
src-tauri/src/
  lib.rs              插件注册、Db 初始化、invoke_handler 命令清单
  main.rs             二进制入口(实际逻辑在 lib.rs 的 run())
  commands/*.rs       Tauri 命令,按域拆分:project / git / script / docker / files
                      / open / tag / walk / scan / report / overview / pin / account
                      / java / toolchain / editor_icon / hidden / prompt / window / mod
  db/                 rusqlite 连接(全局 Mutex 单连接) + migrations.rs 迁移执行器
  models.rs           serde 数据结构(命令/项目/扫描/报告等)
  path_util.rs        clean_str / to_forward_slash 等路径归一化辅助
  scheduler.rs        后台 tokio 任务(日报定时调度等)
  tray.rs             系统托盘图标 + 迷你弹窗窗口(MAIN_WINDOW_LABEL / TRAY_POPUP_LABEL)
  workday.rs          chinese-days 节假日数据拉取与缓存
  error.rs            AppError / AppResult,错误序列化为中文字符串传前端
src-tauri/migrations/ SQL 迁移,NNN_name.sql(当前 001~004)
```

关键规则:

1. **新增 Rust 命令**:在 `commands/*.rs` 实现(返回 `AppResult<T>`)后,必须在 `lib.rs` 的 `invoke_handler!` 里注册,前端经 `cmd<T>("snake_case 名", { camelCase 参数 })` 调用(Tauri 自动做参数名映射)。当前命令域:`project / git / script / docker / files / open / tag / walk / scan / report / overview / pin / account / java / toolchain / editor_icon / hidden / prompt / window`。
2. **Tauri 插件与特性**(`src-tauri/Cargo.toml` + `lib.rs`):`tauri` 启用 `protocol-asset` / `tray-icon` / `image-png` 三个 feature;插件依次注册 `single-instance`(必须最先注册,二次启动聚焦已有窗口并退出新进程)、`opener` / `dialog` / `shell` / `store` / `http` / `updater` / `process` / `autostart`(自启时附 `--autostart` 参数,用于静默驻留托盘)。新增插件后请同步在这里登记。
3. **数据库与持久化迁移**:SQLite 文件在 `~/.repomeow/projects.db`(Windows: `C:\Users\<user>\.repomeow\`)。以版本是否已正式发布或对外分发作为迁移边界:当前版本尚未发布时,同一开发版本内的 SQL、设置、配置及其他持久化格式变更可直接更新该版本的定义,无需为开发快照之间新增迁移;这不保证已有本地开发数据自动升级,必要时可重建开发数据库或配置。版本发布后,不得修改该版本已经使用的迁移文件;数据库结构变更需新增 `migrations/00N_xxx.sql` 并在 `db/migrations.rs` 中按 `PRAGMA user_version` 顺序应用、保证幂等,配置键名/类型/语义等变更也必须提供迁移或兼容处理。每个 SQL 迁移文件顶部必须用 `-- App version: x.y.z` 标明对应应用版本,并用 `-- Status: in development` 标记正在开发的版本;正式发布后改为 `-- Status: released`。当前已应用 001_init / 002_favorite / 003_pinned_commands / 004_git_account_token_invalid。
4. **应用数据目录名 `.repomeow`** 在 Rust(`lib.rs` 的 `APP_DATA_DIR_NAME`)和前端(`stores/settings.ts`)各有一份常量,改动需同步。设置持久化走 `tauri-plugin-store` → `~/.repomeow/settings.json`。AI 提示词不走 store/SQLite,存 `~/.repomeow/prompts/*.md`(`commands/prompt.rs` 读写,文件缺失/为空 = 前端 `lib/ai-prompts.ts` 的内置默认模板)。可再生运行期缓存则放在**安装目录** `<exe 所在目录>/data/` 下(`lib.rs` 的 `runtime_data_root()`,dev 模式落在 `target/debug/data/`):编辑器真实图标缓存为 `data/icons/<kind>.png`(`commands/editor_icon.rs` 从本机 exe / .app 提取,settings 表 `editor_icon_cache` 记录源文件 mtime,变化才重提),chinese-days 节假日缓存为 `data/chinese-days.json`(`workday.rs`,TTL 30 天,缺失/过期自动重拉 CDN)。安装目录不可写时两者按既有语义静默降级(图标回退 lucide 通用图标,节假日数据不缓存)。
5. **窗口与生命周期**(`lib.rs` + `tray.rs`):主窗口默认 `visible: false`,启动时统一 `show()`;带 `--autostart` 参数时保持隐藏仅驻留托盘。托盘迷你弹窗(`TRAY_POPUP_LABEL`)永不真正关闭,失焦自动收起;主窗口(`MAIN_WINDOW_LABEL`)关闭时按设置项 `closeAction`(默认 `tray` = 最小化到托盘,`exit` = 真退出整个进程)。新增窗口/托盘行为请同步登记。
6. **后台任务**(`lib.rs` 的 `setup`):`scheduler::run(handle)` 跑日报定时调度,`commands::git::status_refresher_loop(handle)` 跑批量 git 状态推送(替代前端轮询);变更这两个域时请检查启动序列是否仍按 notify / handle 顺序装配。
7. **路径别名** `@/` → `src/`(tsconfig + vite + vitest 三处均已配置)。
8. **路径风格统一**:禁止各处 ad-hoc `replace('\\', "/")` / `split("/")`,一律走统一辅助——Rust 侧 `path_util.rs`(`clean_str` 落库/缓存 key 用平台分隔符、`to_forward_slash` IPC/git pathspec 用 `/`)、前端 `src/lib/path.ts`(`cleanPath` / `toForwardSlash` / `baseName` / `splitDirName` / `displayRelativeTo`)。项目路径入库前必须 `clean_str`;IPC 输出的仓库内路径恒为 `/` 分隔;各类 HashMap/缓存 key 必须经归一化后再做读写与 invalidate。

## 前端约定

- **UI 体系**:shadcn-vue + Tailwind CSS v4 + lucide 图标;样式合并用 `@/lib/utils` 的 `cn()`。主题用 CSS 变量,亮/暗经根节点 `.dark` 类切换,皮肤经 `data-theme="island"`。
- **Markdown 渲染**:用 `vue-stream-markdown`(Shiki 高亮);MD 主题经根节点 `data-md-theme` 属性切换(default/github/notion/serif);自定义图片/链接渲染器要保留本地 `asset:` 协议与系统打开行为(`src-tauri/Cargo.toml` 已启用 `protocol-asset` feature)。
- **代码编辑器 / diff / 文件树**:CodeMirror 6(各 `lang-*` + `legacy-modes`)在视图层做只读 / 可编辑渲染;diff 高亮、行内差异与并排同步在 `src/lib/diff.ts` + `diff-highlight.ts`;文件树懒加载在 `src/lib/lazy-file-tree.ts`;命令行图标语义在 `command-icons.ts`;`git-graph.ts` 与 `branch-tree.ts` 驱动 GitGraph 视图。
- **i18n**:所有用户可见文案走 `vue-i18n`,键定义在 `src/i18n/locales/zh-CN.ts` 与 `en-US.ts`,新增键两语言必须同时补。仓库有专用翻译子代理(`.zcode/skills/i18n-translator/`,用法见 `docs/i18n-translator.md`),批量翻译/审计时优先调用它。Rust 侧错误文案(error.rs)目前是硬编码中文,属已知现状。
- **TS 严格模式**:`noUnusedLocals` / `noUnusedParameters` 开启,未用变量会导致 build 失败。
- **代码规范**:oxlint 配置在 `.oxlintrc.json`(plugins: typescript/vue/import;correctness/suspicious/perf = error、style = warn;`rules` 中关闭了若干与本项目约定冲突的高噪规则,如 `sort-imports`/`sort-keys`/`id-length`/`func-style`/`no-shadow`/`import/no-unassigned-import`,新增告警前先看现有 `rules` 注释)。oxfmt 配置在 `.oxfmtrc.json`(双引号、分号、2 空格、`trailingComma: all`、`printWidth: 100`、`endOfLine: lf`),**只格式化 `src/`**,根目录配置文件与 `src-tauri/` 不在范围。VS Code 用 Oxc 扩展(`oxc.oxc-vscode`,lint + format 一体),保存自动格式化见 `.vscode/settings.json`。`src/components/ui/`(shadcn-vue 生成文件)会被 oxfmt 一并格式化属正常,**但不要手改其结构**。
- **单元测试**:`vitest` 仅覆盖 `src/**/*.test.ts`(node 环境、无 setup 文件,见 `vitest.config.ts`),用例与源文件同目录(如 `src/lib/diff.test.ts`、`src/stores/settings.test.ts`)。`pnpm test:unit` 跑一次,**不改业务代码时请保持零新增失败用例**。

## 注意事项(Gotchas)

- **git 双层实现**:只读查询(status/分支/worktree 列表/log/graph/commit 文件与 diff/remote 列表/当前用户)走 git2(libgit2,`Cargo.toml` 里 default-features = false);写操作(checkout/commit/merge/rebase/worktree 增删/init)与网络操作(fetch/pull/push/clone)仍走系统 git CLI(`git_command`/`run_git`),以继承用户凭证环境(GCM 等)。新增 git 读路径优先复用 git2 层的 `open_repo`/`format_git_time`/`commit_diff` 等辅助;libgit2 revwalk 的纯时间排序在完全相同时间戳下不稳定,需排序时加 `Sort::TOPOLOGICAL` 保底。
- git 相关命令已禁用终端凭据交互询问;涉及凭证的改动注意保持非交互。
- `run_in_terminal` 在系统终端新窗口执行命令,Windows 优先 Windows Terminal。
- Vite dev 端口 1420 被占用会直接失败(strictPort),`tauri dev` 前先确认端口空闲。
- `src-tauri/target-cdk/` 与 `scripts/` 是未跟踪的本地目录,勿当源码处理。
- 改动设置项时同步检查:`stores/settings.ts` 的持久化默认值、Settings 页面 UI、i18n 词条三处。
- **JDK / 工具链在线安装**(`commands/java.rs` + `commands/toolchain.rs`):从 Adoptium / Zulu 等源下载并解压(`zip` crate,仅 deflate),默认目标路径在设置中;安装过程解压失败/磁盘不足时按既有提示降级,不要改 `lib/jdk.ts` 的安装步骤 UI 而忘了 Rust 端产物路径同步。
- **walk 缓存失效**:`commands/walk.rs` 依赖 `notify` crate 监听项目内 `package.json` / `*.yaml` 变更即时让目录遍历缓存失效,改动 `walk` 行为时请保留文件监听。
