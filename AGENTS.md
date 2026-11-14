# AGENTS.md

本文件供编码代理快速了解本仓库的工作方式，只记录从代码中不易直接看出的约定。

## 执行权限

- 本项目允许编码代理执行完成任务所需的所有操作，包括文件读取、创建、修改、删除，命令执行、依赖安装、测试构建，以及 Git 提交、推送、合并和发布。
- 执行操作无需额外申请权限或逐项确认，按用户目标直接推进并完成验证。

## 项目概述

**RepoMeow(喵库)**:Tauri 2 + Vue 3 + TypeScript 桌面应用(版本以 `package.json` 与 `src-tauri/Cargo.toml` 为准),本地开发项目管理中心。功能包括:项目登记/归档、npm scripts 与自定义命令执行、Docker compose 服务管理、Git 状态与写操作(提交/拉取/推送/分支切换)、README Markdown 渲染、Wiki 生成、项目 AI 问答(chat)、AI 资源库、定时报告、标签、多编辑器打开、系统托盘。

- 包管理器:**pnpm 11.10.0**(存在 `pnpm-lock.yaml` / `pnpm-workspace.yaml`,勿用 npm);Node 为 `^20.19.0 || >=22.12.0`(`package.json` engines 与 Vite 8 一致,CI 使用 22)。
- 主要平台:Windows 本地开发为主;release.yml 以四 job 矩阵同时发布 Windows(NSIS)/ macOS(dmg,arm64+x64)/ Linux(deb+AppImage)。
- 提交信息:**gitmoji + conventional 前缀 + 中文**,单行详细描述,参考 `git log`(例:`🐛 fix(project-ai): 修复…——原因与方案;验证:oxlint / pnpm build 通过`)。

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `pnpm start` | `sem:prepare && tauri dev --config src-tauri/tauri.dev.conf.json`,完整桌面端开发(前端 + Rust);dev 用独立 identifier 以便与安装版同时运行(见关键规则 2) |
| `pnpm dev` | 仅 Vite 前端(端口 1420,见 `tauri.conf.json` devUrl) |
| `pnpm build` | `vue-tsc --noEmit && vite build` + 首屏入口 500 KiB 预算检查,**唯一的类型检查手段** |
| `pnpm build:desktop` | `sem:prepare && tauri build` 打包(NSIS) |
| `pnpm lint` / `pnpm lint:fix` | `oxlint .` 静态检查 / 自动修复 |
| `pnpm format` / `pnpm format:check` | `oxfmt --write src/` 格式化 / 仅检查 |
| `pnpm test:unit` | `vitest run`(node 环境,仅 `src/**/*.test.ts`,无 setup 文件) |
| `pnpm cli:dev -- <args>` | 运行独立 console 目标 repomeow-cli(不开窗口) |
| `pnpm test:cli --exe <程序>` / `pnpm test:cli:shells --exe <程序>` | Node 内置模块驱动的 CLI 回归 / PS 5.1+7 Actions 包装六组回归，无额外解释器依赖 |
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
                        / settings / tags / terminal / update / wiki
  composables/          组合式函数,部分按子域分目录(files/ git/ wiki/)
  stores/               Pinia:projects / settings / tags / wiki / chat / ai-config / terminal
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
  main.rs               入口:首参数命中内置子命令进 CLI,否则进 lib.rs 的 run()
  bin/repomeow-cli.rs    console CLI 入口(含 Windows release);与 GUI 共用 cli/ 分发
  data_sync.rs          CLI 写锁/持久化域版本、GUI 监视与调度/缓存失效
  commands/             Tauri 命令域。目录模块:account / agent / ai / chat / files / git
                        / java / open / project / report / semantic / toolchain / wiki;
                        单文件:docker / editor_icon / hidden / cli / overview / pin
                        / prompt / scan / script / tag / usage / walk / window
  agent/                pi-agent-core 的 Rust 复刻:llm/(adapter + EventStream)、
                        harness/(session/compaction/tools/...)、chat_tools
  ai/                   AI 接入:catalog.rs + builtin_models.json(厂商/模型目录)、
                        sdk.rs、harness_support.rs、wiki_outline.rs、prompts/
  cli/                  内置 CLI(clap 子命令,JSON 输出;按域拆 *_tool.rs,技能文本在仓库根 skills/)
  scheduler/            调度引擎:calendar / config / execution / runtime
  terminal_ipc.rs       CLI 终端控制与桌面进程的一次性 single-instance 握手
  db/                   rusqlite(全局 Mutex 单连接) + migrations.rs
  models.rs / error.rs  serde 结构;AppError 错误序列化为中文字符串传前端
  path_util.rs          clean_str / to_forward_slash / repo_relative_str 路径归一化
  time_util.rs          now_ts 等时间戳统一入口(勿散写 chrono::Utc::now())
  background_task.rs    后台任务进度事件守卫(background://task-progress)
  tray.rs / workday.rs  系统托盘 + 迷你弹窗;chinese-days 节假日缓存
src-tauri/migrations/   SQL 迁移 NNN_name.sql(完整清单以该目录与 `db/migrations.rs` 为准)
scripts/                sem/(sidecar 下载)、release/(发布流程)
```

## 关键规则

1. **新增 Rust 命令**:在 `commands/` 对应域实现(返回 `AppResult<T>`)后,必须在 `lib.rs` 的 `invoke_handler!` 注册;前端经 `cmd<T>("snake_case 名", { camelCase 参数 })` 调用(Tauri 自动映射参数名)。
2. **插件**(`Cargo.toml` + `lib.rs`):`tauri` 启用 `protocol-asset` / `tray-icon` / `image-png`;插件注册顺序:single-instance(**必须最先**,二次启动聚焦已有窗口)→ opener / dialog / shell / store / updater / process / autostart(自启附 `--autostart` 参数)。single-instance 在 Windows 上按 `tauri.conf.json` 的 `identifier` 派生判重 mutex,WebView2 用户数据目录同样按 identifier 落在 `%LOCALAPPDATA%`;dev 经 `tauri.dev.conf.json`(`pnpm start` 的 `--config` 参数,仅 dev 生效)覆盖为 `com.repomeow.dev`,使 dev 与安装版可同时运行,发布构建不经过该文件、identifier 保持 `com.repomeow.app`。
3. **数据库**:SQLite 位于 `~/.repomeow/projects.db`。迁移按 `PRAGMA user_version` 顺序应用、保证幂等;每个迁移文件顶部用 `-- App version: x.y.z` 与 `-- Status: in development|released` 标注。版本发布后不得修改已发布的迁移文件,结构变更新增 `00N_xxx.sql`;未发布的开发版本内可直接改当前版本定义。新增迁移时同步检查目录清单与加载逻辑，不在代理文档中硬编码易过期的当前范围。
4. **应用数据目录 `.repomeow`**:GUI 经 `lib.rs::app_data_dir`、CLI 经 `cli/util.rs::repomeow_data_root` 解析，均支持 `REPOMEOW_DATA_DIR`；前端 settings 通过 `get_app_data_dir` 获取同一目录。设置走 `tauri-plugin-store` → `<数据根>/settings.json`；AI 提示词在 `prompts/*.md`、AI 配置在 `ai-config.json`、Wiki 在 `wiki/<basename>-<hash>/`。运行期可再生缓存仍在安装目录 `data/` 下，不属于隔离的业务数据。
5. **窗口与生命周期**:主窗口默认 `visible: false`,启动时统一 `show()`;带 `--autostart` 保持隐藏仅驻留托盘。托盘迷你弹窗(`TRAY_POPUP_LABEL`)永不真正关闭(失焦收起);主窗口关闭行为按设置项 `closeAction`(tray=最小化到托盘 / exit=退出进程)。
6. **后台任务**(`lib.rs` setup):`scheduler::run(handle)` 定时报告调度;`commands::git::monitor_loop(handle)` 统一 Git 检查循环并 emit `git://project-changed`;资源库启动同步检查 `commands::ai::startup_sync_check`。Git 更新后的联动逻辑订阅该事件,勿新增独立轮询。用户切回窗口(失焦→聚焦)时,`App.vue` 的 `useWindowFocusSignal` 递增聚焦版本号(托盘迷你弹窗跳过),项目页的 `useGitFocusRefresh` 据此重查当前工作区:同一路径由 `src/lib/git-check-coordinator.ts` 的 `gitCheckCoordinator` 做 15s 复用与进行中请求复用(与后端 `STATUS_TTL` 对齐,`force` 同样受限,确需重扫的入口用 `refreshGitStatus(..., { recheck: true })`),因此大仓库不会被窗口切换反复扫描。这类"事件驱动的即时补充"与后台循环互补,仍需避免改成定时轮询。
7. **路径别名** `@/` → `src/`(tsconfig + vite + vitest 三处均配置)。
8. **路径风格统一**:禁止 ad-hoc `replace('\\', "/")`。Rust 侧走 `path_util.rs`(`clean_str` 落库/缓存 key、`to_forward_slash` IPC/git pathspec、`repo_relative_str`);前端走 `src/lib/path.ts`(`cleanPath` / `toForwardSlash` / `baseName` / `splitDirName` / `joinPath` / `displayRelativeTo`)。项目路径入库前必须 `clean_str`;IPC 输出的仓库内路径恒为 `/` 分隔;HashMap/缓存 key 先归一化再读写。
9. **sem sidecar**:官方 sem CLI 以 externalBin 内置,版本/平台/SHA-256 固定在 `scripts/sem/manifest.json`,`pnpm sem:prepare` 下载到被 gitignore 的 `src-tauri/binaries/sem-<target>`;升级版本须同步 `src-tauri/third-party/sem/NOTICE`。应用只从 Rust `commands/semantic/` 暴露固定操作,禁止前端传任意 CLI 参数。
10. **CLI + Skills**:独立 console 程序 `repomeow-cli`（含 Windows release）直接复用 `cli::run_cli_blocking()`；GUI `main.rs` 保留旧首参数 CLI 兼容入口。CLI 覆盖项目、Wiki、语义分析、报告与调度、标签、命令、pin/hidden、AI 配置/用量、提示词、账号、项目文件及桌面终端；git/docker 直接调用本机工具。命令逻辑复用 `commands/*` 的纯函数/blocking 变体；成功 JSON 写 stdout，失败错误 JSON 写 stderr，退出码 0/1/2，help/version 为文本。新增写命令需在 `cli/mod.rs::write_domains` 声明受影响域，并在 GUI 回读对应缓存；同步机制见 `data_sync.rs` 和 `docs/TZD-85-console-sync.md`。技能名称仍为 `repomeow`，源码在 `skills/repomeow/`；技能通过 PATH 调用 `repomeow-cli`，不嵌入本机路径。经「设置 → CLI」导入资源库，再在项目 AI 资源中部署。完整用法见 `CLI.md`。
11. **i18n**:词条改动需同步 zh-CN 与 en-US,`src/i18n/locales/locales.test.ts` 校验键对齐。
12. **错误处理**:Rust 错误经 `error.rs` 序列化为 `{ code, message }` 传前端;前端 `lib/tauri.ts` 对部分笼统错误码追加展示 message,新增错误码需在 Rust 错误枚举、前端处理与 i18n 文案三处同步。
13. **oxlint 配置**:correctness / suspicious / perf 为 error,style 为 warn;忽略 `dist/`、`node_modules/`、`src-tauri/`、`scripts/`。`pnpm lint` 以零 warning 为门禁;`one-var` 关闭以保留独立声明及初始化顺序,`max-params` 上限为 6;生成的 `components/ui/` 不手改,其可选 props 与无类型 ref 规则在配置中豁免。
