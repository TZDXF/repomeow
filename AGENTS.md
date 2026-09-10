# AGENTS.md

本文件供编码代理快速了解本仓库的工作方式，只记录从代码中不易直接看出的约定。

## 项目概述

**RepoMeow(喵库)**:Tauri 2 + Vue 3 + TypeScript 桌面应用(当前版本 0.2.0),本地开发项目管理中心。功能包括:项目登记/归档、npm scripts 与自定义命令执行、Docker compose 服务管理、Git 状态与写操作(提交/拉取/推送/分支切换)、README Markdown 渲染、Wiki 生成、项目 AI 问答(chat)、AI 资源库、定时报告、标签、多编辑器打开、系统托盘。

- 包管理器:**pnpm**(存在 `pnpm-lock.yaml` / `pnpm-workspace.yaml`,勿用 npm)。
- 主要平台:Windows(存在 macOS 图标/交叉编译产物)。
- 提交信息:**gitmoji + conventional 前缀 + 中文**,单行详细描述,参考 `git log`(例:`🐛 fix(project-ai): 修复…——原因与方案;验证:oxlint / pnpm build 通过`)。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `pnpm start` | `sem:prepare && tauri dev`,完整桌面端开发(前端 + Rust) |
| `pnpm dev` | 仅 Vite 前端(端口 1420,见 `tauri.conf.json` devUrl) |
| `pnpm build` | `vue-tsc --noEmit && vite build`,**唯一的类型检查手段** |
| `pnpm build:desktop` | `sem:prepare && tauri build` 打包(NSIS) |
| `pnpm lint` / `pnpm lint:fix` | `oxlint .` 静态检查 / 自动修复 |
| `pnpm format` / `pnpm format:check` | `oxfmt --write src/` 格式化 / 仅检查 |
| `pnpm test:unit` | `vitest run`(node 环境,仅 `src/**/*.test.ts`,无 setup 文件) |
| `pnpm mcp:dev` | 以 `--mcp` 模式运行主程序(stdio MCP Server,不开窗口) |
| `pnpm sem:prepare` / `sem:check` | 下载/校验 sem sidecar 二进制 |
| `pnpm release:*` | `scripts/release/release.mjs` 本地发布调试(check/build/sign/latest/all/local) |

改动后建议验证:`pnpm lint` + `pnpm test:unit` + `pnpm build`;Rust 侧 `cargo check` / `cargo test`(在 `src-tauri/` 下)。`cargo check` 前若 sem 二进制未准备,先跑 `pnpm sem:prepare`。GitHub Actions 有 `release.yml` 负责正式发布。

## 架构与分层

```
src/                    Vue 3 前端(<script setup> SFC)
  views/                ProjectsHome / ProjectDetail / ProjectFiles / GitGraph
                        / ProjectWiki / ReportHistory / ResourceSkillPreview
                        / Settings / TrayPopup(托盘迷你弹窗)
  components/           TitleBar.vue 在顶层;ui/ 为 shadcn-vue(reka-ui)生成组件,勿手改(滚动容器用 common/ScrollArea.vue 包装组件,承载视口高度修复);
                        业务组件按域分目录:ai-elements / chat / common / files / git / icons
                        / java / markdown / open / project / report / scripts / semantic
                        / settings / tags / update / wiki
  composables/          组合式函数,部分按子域分目录(files/ git/ wiki/)
  stores/               Pinia:projects / settings / tags / wiki / chat / ai-config
                        / background-tasks / batch-report / pins / project-assets
                        / project-overview / jdk-install / update
  i18n/locales/         zh-CN.ts(默认)、en-US.ts(回退),locales.test.ts 校验两文件键对齐
  lib/                  前端工具库,单测与源文件同目录(*.test.ts)。要点:
                        tauri.ts(cmd<T> 前后端桥 + 错误码处理)、path.ts(路径归一化)、
                        format.ts(日期格式化,勿散写 toLocaleString)、
                        utils.ts(cn / copyToClipboard / debounce 带 cancel)
  router/               Vue Router
src-tauri/src/
  lib.rs                插件注册、Db 初始化、invoke_handler 命令清单、setup(后台任务)
  main.rs               入口:--mcp 进内置 stdio MCP Server,否则进 lib.rs 的 run()
  commands/             Tauri 命令域。目录模块:account / agent / ai / chat / files / git
                        / java / open / project / report / semantic / toolchain / wiki;
                        单文件:docker / editor_icon / hidden / mcp / overview / pin
                        / prompt / scan / script / tag / usage / walk / window
  agent/                pi-agent-core 的 Rust 复刻:llm/(adapter + EventStream)、
                        harness/(session/compaction/tools/...)、chat_tools
  ai/                   AI 接入:catalog.rs + builtin_models.json(厂商/模型目录)、
                        sdk.rs、harness_support.rs、wiki_outline.rs、prompts/
  mcp/                  内置 stdio MCP Server(--mcp 模式),工具按域拆分
  scheduler/            调度引擎:calendar / config / execution / runtime
  db/                   rusqlite(全局 Mutex 单连接) + migrations.rs
  models.rs / error.rs  serde 结构;AppError 错误序列化为中文字符串传前端
  path_util.rs          clean_str / to_forward_slash / repo_relative_str 路径归一化
  time_util.rs          now_ts 等时间戳统一入口(勿散写 chrono::Utc::now())
  background_task.rs    后台任务进度事件守卫(background://task-progress)
  tray.rs / workday.rs  系统托盘 + 迷你弹窗;chinese-days 节假日缓存
src-tauri/migrations/   SQL 迁移 NNN_name.sql(当前 001~011)
scripts/                sem/(sidecar 下载)、release/(发布流程)
```

## 关键规则

1. **新增 Rust 命令**:在 `commands/` 对应域实现(返回 `AppResult<T>`)后,必须在 `lib.rs` 的 `invoke_handler!` 注册;前端经 `cmd<T>("snake_case 名", { camelCase 参数 })` 调用(Tauri 自动映射参数名)。
2. **插件**(`Cargo.toml` + `lib.rs`):`tauri` 启用 `protocol-asset` / `tray-icon` / `image-png`;插件注册顺序:single-instance(**必须最先**,二次启动聚焦已有窗口)→ opener / dialog / shell / store / updater / process / autostart(自启附 `--autostart` 参数)。
3. **数据库**:SQLite 位于 `~/.repomeow/projects.db`。迁移按 `PRAGMA user_version` 顺序应用、保证幂等;每个迁移文件顶部用 `-- App version: x.y.z` 与 `-- Status: in development|released` 标注。版本发布后不得修改已发布的迁移文件,结构变更新增 `00N_xxx.sql`;未发布的开发版本内可直接改当前版本定义。当前已应用 001_init ~ 011_system_schedules。
4. **应用数据目录 `.repomeow`**:Rust(`lib.rs` 的 `APP_DATA_DIR_NAME`)与前端(`stores/settings.ts`)各有一份常量,改动需同步。设置走 `tauri-plugin-store` → `~/.repomeow/settings.json`;AI 提示词存 `~/.repomeow/prompts/*.md`;AI 接入配置存 `~/.repomeow/ai-config.json`;Wiki 存 `~/.repomeow/wiki/<basename>-<hash>/`;运行期缓存(编辑器图标、chinese-days)在安装目录 `data/` 下(`lib.rs` 的 `runtime_data_root()`,dev 模式落在 `target/debug/data/`),安装目录不可写时静默降级。
5. **窗口与生命周期**:主窗口默认 `visible: false`,启动时统一 `show()`;带 `--autostart` 保持隐藏仅驻留托盘。托盘迷你弹窗(`TRAY_POPUP_LABEL`)永不真正关闭(失焦收起);主窗口关闭行为按设置项 `closeAction`(tray=最小化到托盘 / exit=退出进程)。
6. **后台任务**(`lib.rs` setup):`scheduler::run(handle)` 定时报告调度;`commands::git::monitor_loop(handle)` 统一 Git 检查循环并 emit `git://project-changed`;资源库启动同步检查 `commands::ai::startup_sync_check`。Git 更新后的联动逻辑订阅该事件,勿新增独立轮询。
7. **路径别名** `@/` → `src/`(tsconfig + vite + vitest 三处均配置)。
8. **路径风格统一**:禁止 ad-hoc `replace('\\', "/")`。Rust 侧走 `path_util.rs`(`clean_str` 落库/缓存 key、`to_forward_slash` IPC/git pathspec、`repo_relative_str`);前端走 `src/lib/path.ts`(`cleanPath` / `toForwardSlash` / `baseName` / `splitDirName` / `joinPath` / `displayRelativeTo`)。项目路径入库前必须 `clean_str`;IPC 输出的仓库内路径恒为 `/` 分隔;HashMap/缓存 key 先归一化再读写。
9. **sem sidecar**:官方 sem CLI 以 externalBin 内置,版本/平台/SHA-256 固定在 `scripts/sem/manifest.json`,`pnpm sem:prepare` 下载到被 gitignore 的 `src-tauri/binaries/sem-<target>`;升级版本须同步 `src-tauri/third-party/sem/NOTICE`。应用只从 Rust `commands/semantic/` 暴露固定操作,禁止前端传任意 CLI 参数。
10. **内置 MCP Server**:`main.rs` 在 Tauri 初始化前识别 `--mcp`,复用主程序二进制运行 `src/mcp/` stdio 服务(rmcp crate),不单独发布 MCP 可执行文件。工具组(git / wiki / sem / project / report)开关由设置页写入 `~/.repomeow/settings.json`,**默认全部关闭**,MCP 进程启动时读取,改配置需客户端重连。详见根目录 `MCP.md`。
11. **i18n**:词条改动需同步 zh-CN 与 en-US,`src/i18n/locales/locales.test.ts` 校验键对齐。
12. **错误处理**:Rust 错误经 `error.rs` 序列化为 `{ code, message }` 传前端;前端 `lib/tauri.ts` 对部分笼统错误码追加展示 message,新增错误码需在 Rust 错误枚举、前端处理与 i18n 文案三处同步。
13. **oxlint 配置**:correctness / suspicious / perf 为 error,style 为 warn;忽略 `dist/`、`node_modules/`、`src-tauri/`、`scripts/`。
