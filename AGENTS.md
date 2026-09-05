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
                      / ProjectWiki / ReportHistory / Settings / TrayPopup(系统托盘迷你弹窗)
  components/ui/      shadcn-vue(reka-ui)组件,勿手改生成文件风格
  components/{common,files,git,icons,java,markdown,open,project,report,scripts,settings,tags,update}/
                      业务组件;TitleBar 在 components/ 顶层
  composables/        useCollapsibleOpen 等小组件式组合式函数
  stores/             Pinia:projects / settings / tags / batch-report / jdk-install
                      / pins / project-assets / project-overview / update
  i18n/locales/       zh-CN.ts(默认)、en-US.ts(回退),两文件键必须对齐
  lib/                tauri.ts(前后端桥 cmd<T>)/ ai.ts / path.ts / diff.ts / diff-highlight.ts
                      / wiki.ts / wiki-parse.ts / wiki-generator.ts(双生成内核 WikiGenKernel)/
                      agent.ts(ACP agent 前端桥)/ async-pool.ts 等
                      / git-graph.ts / branch-tree.ts / favorites.ts / open-with.ts 等
                      / format.ts(formatDate/parseDateStr/formatLocalDateTime 等日期工具
                      与 formatRelativeTime,日期格式化勿散写 toLocaleString)
                      / utils.ts(cn / copyToClipboard / debounce,debounce 带 cancel
                      ——@vueuse/core 的 useDebounceFn 无 cancel,需取消语义时用这个)
                      单元测试与源文件同目录(lib/*.test.ts,stores/*.test.ts)
  router/             Vue Router(index.ts)
  styles/markdown/    base.css + index.css + themes/{default,github,notion,serif}.css
src-tauri/src/
  lib.rs              插件注册、Db 初始化、invoke_handler 命令清单
  main.rs             二进制入口(--mcp 进入内置 stdio MCP,否则逻辑在 lib.rs 的 run())
  commands/           Tauri 命令,按域拆分:project / git / script / docker / files
                      / open / tag / walk / scan / report / overview / pin / account
                      / java / toolchain / editor_icon / hidden / prompt / wiki
                      / agent(ACP 客户端,本地 coding agent 会话)/ window / usage / mcp
                      / chat(项目问答:per-project agent 会话 + ChatEvent 流)/ mod
                      大域为目录模块(<域>/mod.rs 作命令门面,re-export 保持
                      commands::<域>::* 路径稳定;仅测试引用的 re-export 须 #[cfg(test)] 门控)
  agent/              pi-agent-core(packages/agent @0.84.4,蓝本 earendil-works/pi)的
                      Rust 完整复刻:llm/(pi-ai 类型契约 + EventStream + openai-completions
                      provider + JSON Schema 参数校验)、core(agent_loop/agent/stream_fn,
                      steering/followUp 队列、工具循环、abort)、harness/(session jsonl/
                      compaction/内置工具/reducer/skills/telemetry 等)。chat 功能即基于
                      core + chat_tools.rs(RepoMeow 工具表)组装;serde 输出与 TS JSON
                      兼容,模块允许 dead_code(复刻完整 API 面)
  mcp/                 内置 stdio MCP Server(--mcp 模式),按 settings.json 工具组开关过滤工具
  db/                 rusqlite 连接(全局 Mutex 单连接) + migrations.rs 迁移执行器
  models.rs           serde 数据结构(命令/项目/扫描/报告等)
  path_util.rs        clean_str / to_forward_slash 等路径归一化辅助
  time_util.rs        now_ts / now_ts_nanos 时间戳统一入口(勿散写 chrono::Utc::now())
  scheduler.rs        后台 tokio 任务入口(日报定时调度等;report_http_client 为报告/AI 共用异步客户端,仅连接超时)
  scheduler/          调度引擎拆分:calendar(日历/过滤)/ config(默认命名与提示词)
                      / execution(fire_schedule 执行与落库)/ runtime(主循环)
  tray.rs             系统托盘图标 + 迷你弹窗窗口(MAIN_WINDOW_LABEL / TRAY_POPUP_LABEL)
  workday.rs          chinese-days 节假日数据拉取与缓存
  error.rs            AppError / AppResult,错误序列化为中文字符串传前端
src-tauri/migrations/ SQL 迁移,NNN_name.sql(当前 001~010)
```

关键规则:

1. **新增 Rust 命令**:在 `commands/*.rs` 实现(返回 `AppResult<T>`)后,必须在 `lib.rs` 的 `invoke_handler!` 里注册,前端经 `cmd<T>("snake_case 名", { camelCase 参数 })` 调用(Tauri 自动做参数名映射)。当前命令域:`project / git / script / docker / files / open / tag / walk / scan / report / overview / pin / account / java / toolchain / editor_icon / hidden / prompt / wiki / agent / window / usage / semantic / mcp / chat`。
2. **Tauri 插件与特性**(`src-tauri/Cargo.toml` + `lib.rs`):`tauri` 启用 `protocol-asset` / `tray-icon` / `image-png` 三个 feature;插件依次注册 `single-instance`(必须最先注册,二次启动聚焦已有窗口并退出新进程)、`opener` / `dialog` / `shell` / `store` / `http` / `updater` / `process` / `autostart`(自启时附 `--autostart` 参数,用于静默驻留托盘)。新增插件后请同步在这里登记。
3. **数据库与持久化迁移**:SQLite 文件在 `~/.repomeow/projects.db`(Windows: `C:\Users\<user>\.repomeow\`)。以版本是否已正式发布或对外分发作为迁移边界:当前版本尚未发布时,同一开发版本内的 SQL、设置、配置及其他持久化格式变更可直接更新该版本的定义,无需为开发快照之间新增迁移;这不保证已有本地开发数据自动升级,必要时可重建开发数据库或配置。版本发布后,不得修改该版本已经使用的迁移文件;数据库结构变更需新增 `migrations/00N_xxx.sql` 并在 `db/migrations.rs` 中按 `PRAGMA user_version` 顺序应用、保证幂等,配置键名/类型/语义等变更也必须提供迁移或兼容处理。每个 SQL 迁移文件顶部必须用 `-- App version: x.y.z` 标明对应应用版本,并用 `-- Status: in development` 标记正在开发的版本;正式发布后改为 `-- Status: released`。当前已应用 001_init / 002_favorite / 003_pinned_commands / 004_git_account_token_invalid / 005_auto_pull / 006_schedule_tag_ids / 007_daily_previous_day / 008_wiki_auto_update / 009_ai_usage_log / 010_ai_usage_cached_tokens。
4. **应用数据目录名 `.repomeow`** 在 Rust(`lib.rs` 的 `APP_DATA_DIR_NAME`)和前端(`stores/settings.ts`)各有一份常量,改动需同步。设置持久化走 `tauri-plugin-store` → `~/.repomeow/settings.json`。AI 提示词不走 store/SQLite,存 `~/.repomeow/prompts/*.md`(`commands/prompt.rs` 读写,文件缺失/为空 = 前端 `lib/ai-prompts.ts` 的内置默认模板);仅 commit/日报/周报三类开放自定义,wiki 大纲/单页提示词因输出格式与解析管线(JSON 大纲、sources 引用块)强耦合而固定为内置模板,不进提示词管理。AI 接入配置(多厂商/模型/问答偏好)同样不走 store/SQLite,存 `~/.repomeow/ai-config.json`(见下方「AI 接入配置」节)。项目 wiki 同样不进 SQLite:落盘 `~/.repomeow/wiki/<basename>-<8位fnv1a哈希>/` 下的 `meta.json`(大纲+headSha+generatedAt,最后写入,status=completed 才有效)与 `pages/NN-slug.md`(tmp+rename 原子写),目录名由 `commands/wiki/` 按 clean_str 路径派生,前端一律经命令拿路径,不自算。可再生运行期缓存则放在**安装目录** `<exe 所在目录>/data/` 下(`lib.rs` 的 `runtime_data_root()`,dev 模式落在 `target/debug/data/`):编辑器真实图标缓存为 `data/icons/<kind>.png`(`commands/editor_icon.rs` 从本机 exe / .app 提取,settings 表 `editor_icon_cache` 记录源文件 mtime,变化才重提),chinese-days 节假日缓存为 `data/chinese-days.json`(`workday.rs`,TTL 30 天,缺失/过期自动重拉 CDN)。安装目录不可写时两者按既有语义静默降级(图标回退 lucide 通用图标,节假日数据不缓存)。
5. **窗口与生命周期**(`lib.rs` + `tray.rs`):主窗口默认 `visible: false`,启动时统一 `show()`;带 `--autostart` 参数时保持隐藏仅驻留托盘。托盘迷你弹窗(`TRAY_POPUP_LABEL`)永不真正关闭,失焦自动收起;主窗口(`MAIN_WINDOW_LABEL`)关闭时按设置项 `closeAction`(默认 `tray` = 最小化到托盘,`exit` = 真退出整个进程)。新增窗口/托盘行为请同步登记。
6. **后台任务**(`lib.rs` 的 `setup`):`scheduler::run(handle)` 跑日报定时调度,`commands::git::monitor_loop(handle)` 每 30 秒经统一管线检查全部未归档项目(本地状态 → fetch → auto_pull 项目按需 `git merge --ff-only @{u}` → 最终状态),并统一 emit `git://project-changed`;前端全部/单项目/worktree 主动检查同样走 `check_git_status` 的 all/project/path scope,Git 写操作成功后也通过同一事件发布函数广播。需要在 Git 更新后执行的逻辑订阅该事件的变化标记,勿新增独立轮询或并行 fetch 循环。
7. **路径别名** `@/` → `src/`(tsconfig + vite + vitest 三处均已配置)。
8. **路径风格统一**:禁止各处 ad-hoc `replace('\\', "/")` / `split("/")`,一律走统一辅助——Rust 侧 `path_util.rs`(`clean_str` 落库/缓存 key 用平台分隔符、`to_forward_slash` IPC/git pathspec 用 `/`、`repo_relative_str` 归一化 LLM 产出的仓库相对路径)、前端 `src/lib/path.ts`(`cleanPath` / `toForwardSlash` / `baseName` / `splitDirName` / `joinPath` / `displayRelativeTo`)。项目路径入库前必须 `clean_str`;IPC 输出的仓库内路径恒为 `/` 分隔;各类 HashMap/缓存 key 必须经归一化后再做读写与 invalidate。

9. **sem 语义分析 Sidecar**:官方 sem CLI 以 Tauri xternalBin 内置,固定版本/平台资产/SHA-256 在 scripts/sem/manifest.json,运行 pnpm sem:prepare 下载到被忽略的 src-tauri/binaries/sem-<target>;升级版本须同步 src-tauri/third-party/sem/NOTICE 并做安装包冒烟测试。应用只从 Rust commands/semantic/ 暴露固定操作,禁止前端传任意 CLI 参数,子进程固定设置 DO_NOT_TRACK / SEM_NO_NETWORK 等环境变量,不得调用 sem update/login/mcp;Sidecar 只随 RepoMeow 版本更新。cargo check 前若二进制尚未准备,先运行 pnpm sem:prepare。
10. **内置 MCP Server**:`main.rs` 在普通 Tauri 初始化前识别 `--mcp`,命中后复用主程序二进制运行 `src-tauri/src/mcp/` 的 stdio 服务,不得恢复独立 MCP 二进制或 Sidecar。工具组开关 `mcpGitCommitEnabled` / `mcpWikiEnabled` / `mcpSemEnabled` / `mcpProjectEnabled` / `mcpReportEnabled` 由前端设置页写入 `~/.repomeow/settings.json`,MCP 进程启动时读取并过滤工具,默认均关闭;配置变更在客户端重连后生效。五组工具:git(`commit_code` / `get_git_status`)、wiki(`get_wiki_directory` / `list_wiki_pages` / `read_wiki_page`,经 `load_wiki_at(data_root)` 无 AppHandle 读取)、sem(`sem_find` / `sem_context` / `sem_relations` / `sem_diff`,经 `SemLauncher::Standalone` + `resolve_sem_binary` 按可执行文件旁路径 spawn sidecar)、project(`read_project_file` / `list_reports` / `list_custom_commands`,Db 直接打开 `~/.repomeow/projects.db`,项目目录经 `clean_str` 匹配 projects 表)、report(`generate_report`,走 `mcp_generate_and_save_report` headless 管线,不发前端事件)。MCP 无 chat 的 ask/all 权限门禁,写操作工具(提交、报告)务必保持独立分组、默认关闭。
11. **全局 AI 资源库**(`commands/ai/resource_library/` + `components/settings/Resource*`):设置页「资源管理」只管理全局 Skills/MCP 与备份,当前**不与项目 AI 资产页联动**。数据固定在 `~/.repomeow/resource-library/`:明文 `library.json`、`skills.json`、`skills/<directory>/SKILL.md`,以及可选加密的 `mcp.json`;同步运行状态单独放仓库外 `~/.repomeow/resource-library-state.json`,避免同步后 Git 工作树变脏。Skill 支持多分组,正文 frontmatter 是 name/description 事实源。每次应用内修改自动本地 git 快照,配置 origin 后后台 fetch/fast-forward/push;分叉只允许用户选择远端 reset 或带显式租约的 `force-with-lease`,首次强制导入前保留带时间戳的完整备份目录。MCP 可由用户口令以 Argon2id + XChaCha20Poly1305 加密,口令/派生密钥只驻留进程内;启用加密会重建 Git 历史清除可达的明文 MCP 提交。前端所有 IPC 与后端字段映射集中在 `lib/resource-library.ts`,勿在组件散写命令名。

## wiki 生成管线(commands/wiki/ + lib/wiki-generator.ts)

- 内置 Agent 后端两阶段(无 embedding/RAG):`collect_wiki_context` 收集过滤后的文件树(walk 缓存 + 产物/二进制/锁文件黑名单,超预算按目录折叠)+ README + 根目录清单文件 → 仓库内置 `AgentHarness` 按项目 config 的 `model`(缺省 = 设置页 `defaultModel`)生成严格 JSON 大纲(`ai/wiki_outline.rs` 校验完整对象、字段、页数、文件与交叉引用;失败最多纠错 2 次)→ 逐页把 `relevantFiles` 全文喂入独立 Harness,允许用 `read/grep/find/ls` 少量补充探索,并用 `write/edit` 直接维护当前页面唯一暂存文件。受限 `ExecutionEnv` 只读项目目录、只写该暂存文件,不注册 bash/powershell;应用校验 H1/sources 后原子提升为正式页面,失败或取消清理暂存且不破坏旧页。页面并发取项目 config 的 `concurrency`(缺省回退设置页全局 `aiConcurrency`),思考强度取项目 config 的 `thinking`(缺省 = 推理模型中档、其余关闭),模型与思考强度对大纲/页面任务统一生效,全部成功后 `save_wiki_meta` 收尾;取消时不写 meta,整本无效。
- 生成状态托管在全局 `stores/wiki.ts`(单例):**离开 wiki 页不中止生成**,回来后按 `genFor` 路径匹配续看进度;同时只允许一个整本生成,期间其他项目可正常查看已有 wiki。生成中与最终查看**复用同一左右布局**(左侧页面列表在生成中把 importance 色点换成单页状态图标,不展示阶段文字/计数/进度条,底部多一个取消按钮;右侧正文换成流式预览),流式预览自动跟随滚动到底部,用户上翻阅读即暂停跟随、滚回底部自动恢复(见 `composables/wiki/useWikiPreviewScroll.ts` 的 pinned/suppress 逻辑);流式内容尚未产出时右侧占位展示「文档书写中」动画(`components/wiki/WikiWritingAnimation.vue`,逐行打字机书写 + 闪烁光标),不再展示逐行工具活动日志。
- AI 思考模式由应用按场景决定,无用户设置:内置 Wiki Agent 对 reasoning 模型使用中等思考强度、非 reasoning 模型关闭;commit 信息/报告/测试连接仍按已知提供方注入关闭思考参数。ACP agent 使用自身配置。大纲最终文本仍经 `ai/sdk.rs` 的 `strip_thinking` 处理;内置页面正文以暂存文件为单一事实源,不从 assistant 文本落盘。
- 内置页面预览来自流式 `write.content` 工具参数;`edit` 完成后重读暂存文件刷新。进度面板继续用 Markdown `mode="streaming"` 预览第一个进行中页面;大纲不展示正文流。
- stale 检测:meta.headSha 与当前 HEAD(git2 `open_repo`,wiki 命令域内复用)不一致即过时;stale 时可**增量更新**(`wiki_changed_files` 取 headSha..HEAD 变更文件清单,仅重生成 relevantFiles 命中的页面并推进 meta.headSha;手动增量更新在无 headSha/历史改写/**生成后端切换(meta.generator 不一致)**时退化为整本重生成;自动更新则忽略旧生成后端与模型,使用项目当前配置增量生成)。
- **双生成后端**(配置判别符保持 `builtin` / `agent`,meta 记录 `builtin` / `acp:<id>`):①**内置 Agent**(默认,仓库内 `AgentHarness` + coding 工具 + 受限暂存写入,模型缺省取设置页 defaultModel、可按项目覆盖);②**本地 coding agent 后端**(经 ACP 协议调用户本机已登录的 agent CLI,复用订阅额度):精选 13 个 agent(claude/codex/gemini/copilot/grok/qwen/cline/glm/pi + 原生二进制 opencode/goose/cursor/kimi,条目 vendor 在 `commands/agent/`,数据源自 ACP registry;**对话框下拉只列已安装的项**,自定义命令行的后端支持保留但已无 UI 入口);agent 自行探索仅限大纲阶段(文件树/README 作提示,提示词预算补读 ≤20 个文件);**页面生成是混合模式**——relevantFiles 全文按内置后端同预算(行号前缀)直接喂入 prompt,agent 仅在不足时补读 ≤5 个文件,提示词禁止跑命令/构建/测试。复用同一严格 JSON 大纲解析(带容错降级:从夹带围栏/前言的输出中提取首个平衡 JSON 对象,未知字段宽松解析,见 `ai/wiki_outline.rs`)与 sources 注释块(行级引用为尽力而为),**每页独立 ACP 会话**(每次重试也换新会话——长会话上下文累积是 max_tokens/max_turn_requests 中断的主因),**页面按配置并发**(config.json backend.concurrency,默认 2、上限 8;会话槽位为集合,取消时终止全部进行中会话),页面同样流式(`agent_message_chunk` 累积)。非 end_turn 的 stop_reason 连同已累计文本透传调用方分类处置(refusal 快速失败,其余换会话重试;限流按错误文本识别 429 退避)。生成面板左侧列表在完成页上展示该页耗时(Page 事件的 durationMs);agent 工具调用不再逐行展示,由 stores/wiki.ts 从 tool 活动累计次数(权限决策行不计入,活动文本不再留存),作为「文档书写中」动画的输入——每次调用触发素材飞入粒子与文档辉光脉冲,累计次数显示为动画右上角徽标。生成配置按项目独立保存为对应 wiki 目录的 `config.json`(`version` + `backend`,两种后端对象均含 model/thinking/concurrency,agent 另含 agentId/customCommand;空模型 = 默认(内置 = 设置页 defaultModel,agent = agent 自身配置),空思考强度同理,并发数内置缺省走全局 aiConcurrency、agent 缺省为 2),不再写全局 settings;旧版全局 wiki agent 配置在项目首次读取时惰性复制为该项目 config,之后互不影响。wiki 页点「生成/重新生成」(或右上角配置入口,edit 模式确认=仅保存)弹出 `components/wiki/WikiGenerateDialog.vue`,表单含后端选择;模型/思考强度/并发数两种后端均有——内置模型按厂商分组列出 ai-config 全部模型(ai-elements ModelSelector,复合值 providerId/modelId),agent 模型/思考强度走探测清单,内置思考强度用 chat 七档(不展示安装路径、无「测试」按钮),打开时读取当前项目配置(配置指向未安装 agent 时归一回内置),选中 agent 后自动探测其模型/思考强度清单(`acpTestCached`,lib/agent.ts 的应用会话级 Promise 缓存,同一 agent 不重复 spawn),确认才写回当前项目,取消丢弃改动;整本生成、增量更新、单页重生成与自动更新均由 Rust 按项目路径读取同一配置。meta 记录 `generator` 字段("builtin"/"acp:<id>",serde default 向后兼容)与「agent 名称 · 所选模型」;自动更新使用项目配置对应的内置 Agent 或 ACP agent(每页临时起独立会话)。
- **ACP 客户端层**(`commands/agent/` + 官方 `agent-client-protocol` crate,JSON-RPC/NDJSON over stdio):前端继续做生成编排,Rust 提供 `agent_list`(安装探测,which crate)/`acp_start`(spawn→initialize(V1 协商)→session/new,握手 60s 超时;可选 model/thinking 参数,建会话后应用,见下)/`acp_prompt`(请求作用域 `Channel<AcpEvent>` 流式推送 chunk/activity,照抄 `git_graph_log` 先例;**单次 prompt 15 分钟总超时**,超时发 session/cancel + 5s 宽限仍未收尾即放弃会话,驱动任务退出并杀进程树,调用方按可重试错误换新会话)/`acp_cancel`(先从注册表移除使驱动循环退出,再 session/cancel + 5s 宽限后 taskkill /T /F 杀进程树)/`acp_test`(spawn+握手+建临时会话(TEMP 目录)后即收尾,返回 agent 名称与其上报的 configOptions/modes——生成配置对话框模型/思考强度下拉的数据源);进程自 spawn 持有 PID(unix 设 process_group(0),Windows CREATE_NO_WINDOW + taskkill /T 覆盖 npx.cmd→node 包装链——crate 内置 ChildGuard 在 Windows 只杀直接子进程,故不用其内置 spawn;**注意 async-process 的 `From<std::process::Command>` 不携带「已配置管道」标记,spawn 时会把未置位的流覆盖为 inherit()——std 侧 piped 后必须再用它自己的 stdin/stdout/stderr setter 重设一次**,否则 child.stdin 为 None),`AGENT_JOBS`/`AGENT_PIDS` 注册表照抄 git.rs `CLONE_JOBS` 模式,退出钩子 `cleanup_on_exit` 与 git 并列挂在 `RunEvent::Exit`。**模型/思考强度经协议原生通道**:session/new 响应的 `config_options`(category=model/thought_level,select 类,分组拍平后透传前端)用 `session/set_config_option` 应用;agent 未上报模型项时回退旧式 modes(`session/set_mode`);值不在上报列表内或请求失败仅 eprintln 不阻断会话。客户端回调:`fs/read_text_file`(canonicalize 限定会话 cwd 内 + 256KB 上限)与权限请求(**按工具类别白名单自动决策**:读/搜索/思考/抓取等只读类放行,Edit/Delete/Move/Execute/SwitchMode 一律拒绝,优先一次性选项避免「总是」扩散;决策作为 activity 事件外显——headless 生成不暴露用户决策,无对应设置项)。错误码 `agent_not_detected`/`agent_spawn_failed`/`agent_handshake_failed`/`agent_prompt_failed`/`agent_canceled`(error.rs + i18n 三处同步)。
- **wiki 自动增量更新(本地 HEAD 更新联动,两级开关)**:全局开关在设置页「跟踪更新」(`stores/settings.ts` 的 `wikiAutoUpdate`)——**全局开 = 所有项目都参与(项目勾选被忽略并在 UI 禁用);全局关 = 仅项目勾选了的参与**(projects 表 `wiki_auto_update` 列,迁移 008,`set_project_wiki_auto_update` 命令)。项目级 Wiki 开关与 `auto_pull` 相互独立:不开追踪时不会擅自拉取远端,但手动 pull、应用内 Git 写操作或外部操作改变本地 HEAD 后仍可触发。Rust 统一 emit `git://project-changed`,主窗口 App.vue 仅在 `head_changed` 时按两级开关决定是否调 `stores/wiki.ts` 的 `autoUpdate`(内部串行队列):检查 meta.headSha..HEAD 的变更文件,仅在命中页面 `relevantFiles` 时重生成受影响页面,无命中也推进 meta.headSha 避免重复检查;自动更新不比较 meta 中的旧生成后端或模型,始终使用项目当前 `config.json` 配置生成受影响页面并回写 meta;不参与/正忙/无 wiki/无 headSha/历史改写时静默跳过,失败 toast 提示,**绝不自动整本重生成**(只留给用户手动触发)。
- 页面底部「来源文件」chips 来自大纲的 relevantFiles(非 LLM 正文解析),点击经 `SourceFileDialog.vue`(复用 CodeViewer + `read_file_preview`)查看文件。**行级引用**(参照 deepwiki-open citation):页面提示词要求 LLM 在正文末尾输出不可见的 `<!-- sources -->` 注释块(每行 `path` 或 `path:start-end`,1-based 闭区间,仅限本页喂入的文件),喂给 LLM 的源文件逐行带 `N: ` 行号前缀(`ai.ts` 的 `buildWikiPageUserPrompt`;提示词声明该前缀仅供引用、不得进入代码引用块);`wiki-parse.ts` 的 `parseWikiSources` 解析并剥离该块(含流式中途的未闭合尾巴,配单测),静态正文与流式预览均渲染剥离后的 body,chips 按路径合并 `:start-end` 徽标,点击时 `SourceFileDialog` 把 startLine/endLine 传给 CodeViewer 的 `revealLines`(`cm-line-cite` 行高亮 + 居中滚动,越界自动收敛;同文件换区间不重读文件)。整文件引用不带行区间纯靠提示词约束(同 deepwiki-open 的 "omit line numbers when the whole file is relevant",不加运行时兜底判定);sources 块里的 bare filename 由 `parseWikiSources(content, relevantFiles)` 按 basename 查表补全为全路径(亦参照 deepwiki-open 的容错)。可见的来源清单段落仍被提示词禁止(信息由 chips 承担,避免重复)。页面提示词要求产出 mermaid 图(flowchart 强制 TD),由 vue-stream-markdown 内置 mermaid 支持渲染。
- **wiki 目录用 git 管理**:每个项目的 wiki 目录本身是本地 git 仓库(首次提交时 `git init`,init 后本地固化 `user.name=RepoMeow` / `user.email=repomeow@localhost` / `commit.gpgsign=false` / `core.autocrlf=false`,提交再加 `-c commit.gpgsign=false --no-verify` 兜底,不依赖用户全局 git 配置)。整本生成与增量更新在 `save_wiki_meta` 落盘后自动快照提交(`commitKind` 参数决定中文提交信息措辞:generate=生成/update=增量更新;提交信息统一附「当前代码 HEAD 前 7 位」,即 `commit_message` 的 head 参数,非 git 项目省略),提交失败仅 eprintln 不阻断落盘;单页重新生成不经 meta,由前端调 `commit_wiki` 命令补提交(kind=page + 页面标题);`begin_wiki` 只清 pages/ 与 meta.json 而**保留 config.json 与 .git**,整本重新生成在同一配置和历史上演进;`delete_wiki` 不走 git,整目录直接删除(`remove_wiki_dir` 会先清 Windows 只读位——git 对象文件只读,裸 `remove_dir_all` 会「拒绝访问」)。快照提交幂等:`git status --porcelain` 为空即跳过。
- git.rs 的 `open_repo` 是 pub(crate),git 读路径优先复用;wiki 快照提交复用 git.rs 的 `run_git`/`git_command`(亦 pub(crate))。

## AI 接入配置(ai/catalog.rs + commands/ai/config.rs + stores/ai-config.ts)

- 多厂商配置单一事实源:`~/.repomeow/ai-config.json`(camelCase + `version`,格式对齐 pi 的 models.json)。顶层:`providers`(BTreeMap<厂商id, {name/baseUrl/apiKey/api/models[]}>)、`defaultModel`(commit/report/wiki/测试连接使用的投影)、`chat`(问答偏好:providerId/modelId/thinking/permission)。`api` 是 wire adapter 判别符,支持 `openai-completions` / `openai-responses` / `anthropic-messages` / `google-generative-ai`;模型条目可用可选 `api` 覆盖厂商默认值(优先级 model.api > provider.api),未知字符串无损保存但调用时明确报错。`agent/llm/` 按 pi 0.84.4 对齐四类 adapter 的统一 `streamSimple` 事件契约,chat/wiki/commit/report/定时任务/测试连接均经同一 dispatcher,不再另走 OpenAI 专用调用栈。模型定义带元数据(contextWindow/maxTokens/reasoning/cost $/M),支撑 chat 的上下文占用显示、思考参数与成本计算;模型另可带 `compat`(按最终 API 应用,缺省 = adapter 默认/自动探测),设置页只在 OpenAI Completions 下展示常用四项高级配置,UI 未暴露字段保存时透传。
- 文件缺失时用内置目录 `src-tauri/src/ai/builtin_models.json`(include_str!,来源 models.dev/pi 精选:openai/anthropic/google/deepseek/moonshotai/zhipuai/alibaba/siliconflow/openrouter/ollama/lmstudio)播种;播种时若旧 `settings.json` 的 `aiBaseUrl/aiApiKey/aiModel` 三键仍齐备,额外合成「自定义」厂商并指向它完成迁移(旧键不删除)。文件损坏时备份为 `ai-config.json.corrupt-<ts>` 后重新播种。读写均为 tmp+rename 原子写;`normalize` 负责去空白/剔空/引用失效归一。
- 消费方:`sdk::load_config` 改读该文件(defaultModel 投影成三键 `AiConfig`,commit/report/wiki/测试连接调用点不变);`scheduler/config.rs` 同步切换;chat 读 `chat` 段(chat 引用失效回退 defaultModel)。每次调用热读文件,无缓存/监听。命令面:`ai_config_get` / `ai_config_save` / `ai_config_reveal` / `ai_config_builtin_providers`(添加厂商对话框的内置目录候选),前端经 `lib/ai-config.ts` + `stores/ai-config.ts` 读写;settings.json 中旧三键已从 `stores/settings.ts` 移除。
- chat 偏好热切换:`chat_send` 每次前置 busy 前重读配置,thinking/permission 变化就地生效且会话历史保留;模型与密钥由 `chat_stream_fn` 每次 LLM 调用重读解析(`catalog::resolve_model` 填全元数据,`Model::from_settings` 已门控为仅测试用)。工具权限两档均向模型暴露全部 14 个工具:`all` 直接执行;`ask` 通过 chat 专属 `before_tool_call` 硬门禁拦截 `update_wiki` / `regenerate_wiki` / `add_custom_command` / `generate_report` / `set_wiki_model`,向前端推 `ToolPermissionRequest` 并等待单次允许/拒绝,取消或 2 分钟超时自动阻断;旧配置值 `readOnly` 兼容归一为 `ask`。工具含 `get_ai_config`(只读:厂商/模型清单、默认模型、本项目 wiki 生成配置及其模型有效性,不含密钥)与 `set_wiki_model`(写回项目 wiki config.json 的内置后端模型,先校验模型存在且厂商有密钥;agent 后端不适用),用于 wiki 生成/更新因「配置的模型不存在」失败时的换模型恢复;`regenerate_wiki` 后台启动前会预检内置后端模型,失败直接回传错误给 agent。
- 问答底栏控件(ChatDock):模型选择器(ai-elements `model-selector/`,按厂商分组,复合值 `providerId/modelId`)、思考强度(`chat.thinkingLevels.*` 七档,模型 reasoning=false 时禁用)、权限切换(Eye=ask「确认后执行」/ Wrench=all)、上下文占用(ai-elements `context/` 官方组件套件:HoverCard 悬停弹层,触发器仅环形进度图标;token 口径 = 最近 assistant 消息 usage.total_tokens,TurnEnd 逐轮推送,同事件附带 tiktoken 本地估算的上下文构成 system_prompt/tools/messages,弹层展示各部分占比与平均缓存命中率——前端按各轮 Σcached/Σinput 加权累计;成本按模型元数据 cost 费率估算,不用 tokenlens)。回答回合进行中这些控件禁用,避免在途调用与界面状态不一致;ask 模式的受控工具在过程卡片中展示完整参数与「拒绝/允许本次」按钮。设置页 `AiSettings.vue` 重构为:默认模型选择器 + 厂商 Collapsible 卡片(增删改、按厂商「获取模型列表」合并、模型行元数据编辑)+ 保存/测试连接 + 配置文件所在目录打开。

## 项目 AI 面板(commands/ai/assets.rs + components/project/AiAssetsView.vue)

- 详情页内容区经分段控件切换「执行 / AI」两视图(`repomeow.project-detail-view` localStorage 全局记忆,控件在头部右下角):执行 = 脚本/Docker/SpringBoot/自定义命令卡片网格;AI 视图 = 全宽非卡片布局的「AI 资产」面板:`scan_project_ai_assets` 单次 IPC 聚合——指令/规则/设置文件(CLAUDE.md、AGENTS.md、GEMINI.md、.cursorrules、.cursor/rules/*.mdc 等固定路径探测,**不做全仓库递归**)、项目内 MCP 配置(见下条 `MCP_TARGETS`,列出服务器名+原始定义)、.claude/skills/*/SKILL.md 与 .agents/skills/*/SKILL.md(最小 frontmatter 提取 name/description,跨目录按技能名去重、`.claude/skills` 优先,列表行尾部标注所在目录)、以及 registry 13 个 agent 的安装状态 × 项目配置命中(`AGENT_PROBES` 静态映射)。
- **MCP 可视化管理**(`assets/manage.rs` + `assets/mcp_formats.rs`):七个管理目标静态表 `MCP_TARGETS`,按各家项目级配置文件与方言(claude=`.mcp.json`、cursor=`.cursor/mcp.json`、copilot=`.vscode/mcp.json`(键 `servers`)、gemini=`.gemini/settings.json` 的 mcpServers、codex=`.codex/config.toml` 的 `mcp_servers` TOML 表、opencode=`opencode.json` 的 `mcp` 键、zcode=`.zcode/config.json` 的嵌套 `mcp.servers` 键(服务器对象与 claude 同形,复用 claude 方言;JSON 键名支持点号嵌套路径);**pi 官方无 MCP 支持故不在列**)扫描返回 `dialect`/归属 agents 与完整服务器定义,`create_project_skill` / `delete_project_skill` 管 skills,`set_project_mcp_server` / `remove_project_mcp_server` 表单新增/修改/移除服务器——只允许写表内配置文件,键名由后端按表推导,防任意路径写;写入语义:JSON 方言 tmp+rename 原子写且保留其他键,codex 经 toml_edit **保格式编辑**(手写注释/排版不丢,嵌套 env 渲染为子表),各方言字段映射由前端表单按方言组装(claude 带 type、gemini 远程 url/httpUrl、codex 无 type 远程头 `http_headers`、opencode type=local/remote 且 command 为数组、env 叫 `environment`),编辑时仅覆盖方言托管键、自定义字段(如 zcode 的 `enable` 停用标记)保留。`ProjectAiAssets.mcpTargets` 返回全部目标(含未创建文件)供「添加」对话框选择。
- 点击文件/技能条目打开 `AiFileDrawer.vue` 右侧抽屉:read_file_preview 读取,Markdown 渲染态 ↔ CodeViewer editable 编辑态(新增 `editable` prop 与 `getText()`,附 history/默认键位,依赖 @codemirror/commands),保存走 save_text_file,有未保存改动时 Esc/遮罩不静默丢弃。
- **cc-switch 导入**(只读 `~/.cc-switch`,不回写):`ai_cc_switch_assets` 读 cc-switch.db 的 `skills` / `mcp_servers` 表(缺表容忍,复用「复制 db+WAL 到临时目录」防锁;skills 正文在 `~/.cc-switch/skills/<directory>/`);`set_project_cc_skill` 勾选即递归复制(跳过符号链接)到项目 `.claude/skills/<dir>`(已存在整体替换;删除前先清 Windows 只读位),取消即删除;`set_project_cc_mcp` 把 server_config 按 name **合并**写入项目 `.mcp.json` 的 mcpServers(项目自有条目不动、损坏 JSON 报错不盲写、tmp+rename 原子写),取消按键名移除。勾选状态不另建存储,由重新扫描项目文件推导。

## AI 用量统计(commands/usage.rs + lib/ai-usage.ts)

- 每次 LLM 调用逐条写入 SQLite `ai_usage_log`(迁移 009):时间、任务类型(`commit / report / wiki / chat` 四类,日报周报并入 report,测试连接不计量;前后端取值一致)、模型、输入/输出/合计 tokens、耗时、缓存命中 tokens(cached_tokens,输入的子集:OpenAI 兼容的 prompt_tokens_details.cached_tokens / ACP 的 cachedReadTokens;迁移 010);token 列可空(provider 未返回 usage 时行仍在,计入调用次数但不计入 SUM)。展示在设置页「AI 用量」分类(`AiUsageSettings.vue`:汇总五格 + 最近半年热力图 + 任务类型分布列表(类型 · 次数 · 合计 tokens)+ 明细分页列表与清空;热力图网格/分档逻辑在 `lib/usage-heatmap.ts`,27 周 × 7 行周一起始,强度按窗口内最大日 tokens 分 4 档,有调用但无 tokens 的日子记最低档)。
- **三条采集链路**:①前端 commit/报告调用收敛在 `lib/ai.ts`(AI SDK 的 usage 已解析,generateText 解构 `usage`、streamText 消费 `result.usage`,SDK 自动带 `stream_options.include_usage`),经 `lib/ai-usage.ts` 的 `recordAiUsage` fire-and-forget 上报,**记录失败绝不影响生成主流程**;②Rust 侧:wiki 内置 Agent 按 harness Usage 事件**逐次 LLM 请求**落库(task_type="wiki",含单请求耗时;多轮工具循环拆成多行),定时报告(scheduler.rs)`ChatResponse` 解析 `usage` 字段与报告历史同一事务落库(均复用 `insert_usage_row`);③ACP agent 后端:`agent-client-protocol` 开启 `unstable_end_turn_token_usage` feature 读 PromptResponse.usage——按**单次 prompt 口径**直接记录(不可对相邻响应做累计差分,否则后一请求用量较小时会误记为空);agent 未上报时用 `tiktoken-rs` 统计应用可见的 prompt 与最终正文(已知 OpenAI 模型选择对应编码器,未知/第三方模型回退 `o200k_base`),该兜底不含 agent 内部工具调用和上下文,属于保守估算,cached tokens 留空。
- 重试语义:大纲/页面每次重试尝试各自成行;失败请求不记录。日志保留期 190 天(`lib.rs` setup 时 `prune_old_entries` 清理;与半年热力图窗口对齐);命令域另有 `list_ai_usage_log`(倒序分页 + 任务筛选)与 `clear_ai_usage_log`。按日聚合用 SQLite `date(created_at,'unixepoch','localtime')`(本机时区)。

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
- **执行命令的终端选择**:设置项 `terminal`(settings.json,cmd/powershell/gitbash,默认 cmd,仅 Windows 生效)由 Rust 侧 `commands/open.rs` 的 `resolve_shell` 经 `tray::read_setting_string` 在执行时读取,前端调用点不传参;`spawn_terminal` 按 shell 分支(cmd 走原 wt `cmd /k` / `start` 兜底;powershell 把工作目录交给 wt `-d` / `start /D`,命令整体 UTF-16LE+base64 走 `-EncodedCommand`,可执行文件优先 pwsh(PowerShell 7 才支持 `&&`),回退系统自带 powershell;gitbash 由 `find_git_bash` 从 `where git` 推导 `<Git>\bin\bash.exe`——禁用裸 `where bash`,会命中 WSL——未安装回退 cmd)。`toolchain_op` 的命令文本是 cmd 专属语法(`del /f`、`%USERPROFILE%`、`&` 串联),固定 `ShellKind::Cmd` 不跟随设置。多行命令摊平(`flatten_multiline`)先按保守规则合并 `\` 续行(仅当 `\` 前是空白字符,避免误并行尾是 Windows 路径的既有命令),再按 shell 用 ` & `/`; ` 连接。
- `run_in_terminal` 在系统终端新窗口执行命令,Windows 优先 Windows Terminal。
- Vite dev 端口 1420 被占用会直接失败(strictPort),`tauri dev` 前先确认端口空闲。
- `src-tauri/target-cdk/` 与 `scripts/` 是未跟踪的本地目录,勿当源码处理。
- 改动设置项时同步检查:`stores/settings.ts` 的持久化默认值、Settings 页面 UI、i18n 词条三处。
- **JDK / 工具链在线安装**(`commands/java/` + `commands/toolchain/`):从 Adoptium / Zulu 等源下载并解压(`zip` crate,仅 deflate),默认目标路径在设置中;安装过程解压失败/磁盘不足时按既有提示降级,不要改 `lib/jdk.ts` 的安装步骤 UI 而忘了 Rust 端产物路径同步。
- **walk 缓存失效**:`commands/walk.rs` 依赖 `notify` crate 监听项目内 `package.json` / `*.yaml` 变更即时让目录遍历缓存失效,改动 `walk` 行为时请保留文件监听。
- **cargo test 全体启动失败(0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND)**:lib 测试二进制没有应用清单,comctl32 解析到 v5,而 tauri-plugin-dialog 的 rfd 依赖启用 `common-controls-v6` 会静态导入仅存在于 v6 的 `TaskDialogIndirect`,加载期即失败(与测试代码无关)。修复方案:`build.rs` 关闭 tauri-build 自带的清单嵌入(`WindowsAttributes::new_without_app_manifest()`,图标/版本信息资源不受影响),改为对**全部目标**统一 `cargo:rustc-link-arg=/MANIFEST:EMBED + /MANIFESTINPUT:windows/app.manifest`(内容与 tauri 默认清单一致)。两个坑:①`cargo:rustc-link-arg-tests` 不覆盖 lib 单元测试二进制,且包内无 `tests/` 目标时 cargo 直接报 "does not have a test target";②tauri-build 的清单嵌入与 `/MANIFESTINPUT` 同时开启会因重复 MANIFEST 资源报 CVT1100/LNK1123,二者只能留一个。
