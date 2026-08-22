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
                      / wiki.ts / wiki-parse.ts / wiki-generator.ts / async-pool.ts 等
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
  main.rs             二进制入口(实际逻辑在 lib.rs 的 run())
  commands/*.rs       Tauri 命令,按域拆分:project / git / script / docker / files
                      / open / tag / walk / scan / report / overview / pin / account
                      / java / toolchain / editor_icon / hidden / prompt / wiki / window / mod
  db/                 rusqlite 连接(全局 Mutex 单连接) + migrations.rs 迁移执行器
  models.rs           serde 数据结构(命令/项目/扫描/报告等)
  path_util.rs        clean_str / to_forward_slash 等路径归一化辅助
  time_util.rs        now_ts / now_ts_nanos 时间戳统一入口(勿散写 chrono::Utc::now())
  scheduler.rs        后台 tokio 任务(日报定时调度等;report_http_client 为报告/AI 共用异步客户端,仅连接超时)
  tray.rs             系统托盘图标 + 迷你弹窗窗口(MAIN_WINDOW_LABEL / TRAY_POPUP_LABEL)
  workday.rs          chinese-days 节假日数据拉取与缓存
  error.rs            AppError / AppResult,错误序列化为中文字符串传前端
src-tauri/migrations/ SQL 迁移,NNN_name.sql(当前 001~007)
```

关键规则:

1. **新增 Rust 命令**:在 `commands/*.rs` 实现(返回 `AppResult<T>`)后,必须在 `lib.rs` 的 `invoke_handler!` 里注册,前端经 `cmd<T>("snake_case 名", { camelCase 参数 })` 调用(Tauri 自动做参数名映射)。当前命令域:`project / git / script / docker / files / open / tag / walk / scan / report / overview / pin / account / java / toolchain / editor_icon / hidden / prompt / wiki / window`。
2. **Tauri 插件与特性**(`src-tauri/Cargo.toml` + `lib.rs`):`tauri` 启用 `protocol-asset` / `tray-icon` / `image-png` 三个 feature;插件依次注册 `single-instance`(必须最先注册,二次启动聚焦已有窗口并退出新进程)、`opener` / `dialog` / `shell` / `store` / `http` / `updater` / `process` / `autostart`(自启时附 `--autostart` 参数,用于静默驻留托盘)。新增插件后请同步在这里登记。
3. **数据库与持久化迁移**:SQLite 文件在 `~/.repomeow/projects.db`(Windows: `C:\Users\<user>\.repomeow\`)。以版本是否已正式发布或对外分发作为迁移边界:当前版本尚未发布时,同一开发版本内的 SQL、设置、配置及其他持久化格式变更可直接更新该版本的定义,无需为开发快照之间新增迁移;这不保证已有本地开发数据自动升级,必要时可重建开发数据库或配置。版本发布后,不得修改该版本已经使用的迁移文件;数据库结构变更需新增 `migrations/00N_xxx.sql` 并在 `db/migrations.rs` 中按 `PRAGMA user_version` 顺序应用、保证幂等,配置键名/类型/语义等变更也必须提供迁移或兼容处理。每个 SQL 迁移文件顶部必须用 `-- App version: x.y.z` 标明对应应用版本,并用 `-- Status: in development` 标记正在开发的版本;正式发布后改为 `-- Status: released`。当前已应用 001_init / 002_favorite / 003_pinned_commands / 004_git_account_token_invalid / 005_auto_pull / 006_schedule_tag_ids / 007_daily_previous_day / 008_wiki_auto_update。
4. **应用数据目录名 `.repomeow`** 在 Rust(`lib.rs` 的 `APP_DATA_DIR_NAME`)和前端(`stores/settings.ts`)各有一份常量,改动需同步。设置持久化走 `tauri-plugin-store` → `~/.repomeow/settings.json`。AI 提示词不走 store/SQLite,存 `~/.repomeow/prompts/*.md`(`commands/prompt.rs` 读写,文件缺失/为空 = 前端 `lib/ai-prompts.ts` 的内置默认模板);仅 commit/日报/周报三类开放自定义,wiki 大纲/单页提示词因输出格式与解析管线(XML 大纲、sources 引用块)强耦合而固定为内置模板,不进提示词管理。项目 wiki 同样不进 SQLite:落盘 `~/.repomeow/wiki/<basename>-<8位fnv1a哈希>/` 下的 `meta.json`(大纲+headSha+generatedAt,最后写入,status=completed 才有效)与 `pages/NN-slug.md`(tmp+rename 原子写),目录名由 `commands/wiki.rs` 按 clean_str 路径派生,前端一律经命令拿路径,不自算。可再生运行期缓存则放在**安装目录** `<exe 所在目录>/data/` 下(`lib.rs` 的 `runtime_data_root()`,dev 模式落在 `target/debug/data/`):编辑器真实图标缓存为 `data/icons/<kind>.png`(`commands/editor_icon.rs` 从本机 exe / .app 提取,settings 表 `editor_icon_cache` 记录源文件 mtime,变化才重提),chinese-days 节假日缓存为 `data/chinese-days.json`(`workday.rs`,TTL 30 天,缺失/过期自动重拉 CDN)。安装目录不可写时两者按既有语义静默降级(图标回退 lucide 通用图标,节假日数据不缓存)。
5. **窗口与生命周期**(`lib.rs` + `tray.rs`):主窗口默认 `visible: false`,启动时统一 `show()`;带 `--autostart` 参数时保持隐藏仅驻留托盘。托盘迷你弹窗(`TRAY_POPUP_LABEL`)永不真正关闭,失焦自动收起;主窗口(`MAIN_WINDOW_LABEL`)关闭时按设置项 `closeAction`(默认 `tray` = 最小化到托盘,`exit` = 真退出整个进程)。新增窗口/托盘行为请同步登记。
6. **后台任务**(`lib.rs` 的 `setup`):`scheduler::run(handle)` 跑日报定时调度,`commands::git::status_refresher_loop(handle)` 跑批量 git 状态推送(替代前端轮询),`commands::git::auto_pull_loop(handle)` 跑「跟踪更新」自动拉取(auto_pull=1 的项目 fetch 后 `git merge --ff-only @{u}`,无法快进即静默取消;快进成功除广播状态外另 emit `git://auto-pulled`,供前端 wiki 自动增量更新消费,见 wiki 管线一节);变更这几个域时请检查启动序列是否仍按 notify / handle 顺序装配。
7. **路径别名** `@/` → `src/`(tsconfig + vite + vitest 三处均已配置)。
8. **路径风格统一**:禁止各处 ad-hoc `replace('\\', "/")` / `split("/")`,一律走统一辅助——Rust 侧 `path_util.rs`(`clean_str` 落库/缓存 key 用平台分隔符、`to_forward_slash` IPC/git pathspec 用 `/`)、前端 `src/lib/path.ts`(`cleanPath` / `toForwardSlash` / `baseName` / `splitDirName` / `displayRelativeTo`)。项目路径入库前必须 `clean_str`;IPC 输出的仓库内路径恒为 `/` 分隔;各类 HashMap/缓存 key 必须经归一化后再做读写与 invalidate。

## wiki 生成管线(commands/wiki.rs + lib/wiki-generator.ts)

- 两阶段(参照 deepwiki-open,无 embedding/RAG):`collect_wiki_context` 收集过滤后的文件树(walk 缓存 + 产物/二进制/锁文件黑名单,超预算按目录折叠)+ README + 根目录清单文件 → LLM 产裸 XML 大纲(`wiki-parse.ts` 三层容错:fence 剥离/合成闭合标签/逐 `<page>` 切分,配单测)→ 逐页 `read_wiki_files` 取相关文件全文喂 LLM(单页重试 2 次,并发走 `aiConcurrency`)→ `save_wiki_page` 逐页落盘 → `save_wiki_meta` 收尾。取消时不写 meta,整本视为无效。
- 生成状态托管在全局 `stores/wiki.ts`(单例):**离开 wiki 页不中止生成**,回来后按 `genFor` 路径匹配续看进度;同时只允许一个整本生成,期间其他项目可正常查看已有 wiki。生成中与最终查看**复用同一左右布局**(左侧页面列表在生成中把 importance 色点换成单页状态图标,不展示阶段文字/计数/进度条,底部多一个取消按钮;右侧正文换成流式预览),流式预览自动跟随滚动到底部,用户上翻阅读即暂停跟随、滚回底部自动恢复(见 `ProjectWiki.vue` 的 pinned/suppress 逻辑)。
- AI 思考模式由应用按场景决定,无用户设置:wiki 大纲与页面生成固定 `getChatModel(true)`(不注入 provider 关闭思考参数,模型按默认行为输出 `<think>` 块);commit 信息/报告/测试连接默认 `getChatModel(false)`(命中已知推理模型提供方时注入关闭思考参数,避免思考带来的延迟副作用)。`stripThinking` 的兜底剥除对响应起始位置的闭合思考块始终生效。
- 页面正文走**流式生成**(`ai.ts` 的 `streamWikiPage`,streamText + Tauri http 插件 pull 式读响应体),进度面板用 Markdown `mode="streaming"` 实时预览第一个进行中页面;大纲仍为非流式(generateText)。
- stale 检测:meta.headSha 与当前 HEAD(git2 `open_repo`,wiki.rs 内复用)不一致即过时;stale 时可**增量更新**(`wiki_changed_files` 取 headSha..HEAD 变更文件清单与提交数 commitCount,仅重生成 relevantFiles 命中的页面并推进 meta.headSha;无 headSha/历史改写时退化为整本重生成)。
- **wiki 自动增量更新(「跟踪更新」联动,两级开关)**:全局开关与阈值在设置页「跟踪更新」(`stores/settings.ts` 的 `wikiAutoUpdate`/`wikiAutoUpdateThreshold`,默认 10)——**全局开 = 所有跟踪项目都参与(项目勾选被忽略并在 UI 禁用);全局关 = 仅项目勾选了的参与**(projects 表 `wiki_auto_update` 列,迁移 008,`set_project_wiki_auto_update` 命令)。auto-pull 快进成功后 Rust emit `git://auto-pulled`(project_id + 本轮拉取提交数),主窗口 App.vue 按上述规则决定是否调 `stores/wiki.ts` 的 `autoUpdate`(内部串行队列):「meta.headSha..HEAD 提交数 ≥ 阈值」才执行,复用手动增量更新的 `applyUpdate` 同一管线;不参与/正忙/无 wiki/无 headSha/历史改写时静默跳过,失败 toast 提示,**绝不自动整本重生成**(只留给用户手动触发)。
- 页面底部「来源文件」chips 来自大纲的 relevantFiles(非 LLM 正文解析),点击经 `SourceFileDialog.vue`(复用 CodeViewer + `read_file_preview`)查看文件。**行级引用**(参照 deepwiki-open citation):页面提示词要求 LLM 在正文末尾输出不可见的 `<!-- sources -->` 注释块(每行 `path` 或 `path:start-end`,1-based 闭区间,仅限本页喂入的文件),喂给 LLM 的源文件逐行带 `N: ` 行号前缀(`ai.ts` 的 `buildWikiPageUserPrompt`;提示词声明该前缀仅供引用、不得进入代码引用块);`wiki-parse.ts` 的 `parseWikiSources` 解析并剥离该块(含流式中途的未闭合尾巴,配单测),静态正文与流式预览均渲染剥离后的 body,chips 按路径合并 `:start-end` 徽标,点击时 `SourceFileDialog` 把 startLine/endLine 传给 CodeViewer 的 `revealLines`(`cm-line-cite` 行高亮 + 居中滚动,越界自动收敛;同文件换区间不重读文件)。整文件引用不带行区间纯靠提示词约束(同 deepwiki-open 的 "omit line numbers when the whole file is relevant",不加运行时兜底判定);sources 块里的 bare filename 由 `parseWikiSources(content, relevantFiles)` 按 basename 查表补全为全路径(亦参照 deepwiki-open 的容错)。可见的来源清单段落仍被提示词禁止(信息由 chips 承担,避免重复)。页面提示词要求产出 mermaid 图(flowchart 强制 TD),由 vue-stream-markdown 内置 mermaid 支持渲染。
- **wiki 目录用 git 管理**:每个项目的 wiki 目录本身是本地 git 仓库(首次提交时 `git init`,init 后本地固化 `user.name=RepoMeow` / `user.email=repomeow@localhost` / `commit.gpgsign=false` / `core.autocrlf=false`,提交再加 `-c commit.gpgsign=false --no-verify` 兜底,不依赖用户全局 git 配置)。整本生成与增量更新在 `save_wiki_meta` 落盘后自动快照提交(`commitKind` 参数决定中文提交信息措辞:generate=生成/update=增量更新;提交信息统一附「当前代码 HEAD 前 7 位」,即 `commit_message` 的 head 参数,非 git 项目省略),提交失败仅 eprintln 不阻断落盘;单页重新生成不经 meta,由前端调 `commit_wiki` 命令补提交(kind=page + 页面标题);`begin_wiki` 只清 pages/ 与 meta.json 而**保留 .git**,整本重新生成在同一历史上演进;`delete_wiki` 不走 git,整目录直接删除(`remove_wiki_dir` 会先清 Windows 只读位——git 对象文件只读,裸 `remove_dir_all` 会「拒绝访问」)。快照提交幂等:`git status --porcelain` 为空即跳过。
- git.rs 的 `open_repo` 是 pub(crate),git 读路径优先复用;wiki 快照提交复用 git.rs 的 `run_git`/`git_command`(亦 pub(crate))。

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
