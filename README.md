# RepoMeow · 喵库

基于 **Tauri 2 + Vue 3 + TypeScript** 的本地开发项目管理中心（桌面端）。一窗集中登记本地项目，完成脚本、Docker Compose、Git、README 预览、Wiki 生成、项目 AI 问答、定时报告等操作；并通过内置 CLI + Skills 开放给 Codex、Claude Code 等 AI agent。

## 功能特性

### 项目与脚本

- **项目管理**：添加/归档/删除项目，可重指定或移动目录；卡片、表格双视图；名称/描述搜索、标签筛选、收藏
- **脚本执行**：解析 `package.json` scripts 与自定义命令，分组折叠、一键在系统终端运行；常用命令在托盘弹窗快速执行
- **Docker Compose**：自动扫描 compose 文件，解析服务与端口；`compose ps` 状态指示，浏览器直达端口
- **Java 支持**：JDK 检测/安装引导、Spring Boot 识别

### Git

- **状态与写操作**：状态总览、分支切换、提交/拉取/推送；拉取冲突弹窗引导；push 被拒提示先拉取；未跟踪文件显式勾选；支持自动拉取
- **提交图谱**：d3 渲染分支/提交拓扑（GitGraph）
- **Stash 与 Worktree**：Stash 列表、弹出/清理、差异查看；Worktree 管理
- **冲突解决**：合并冲突逐个处理，可交给内置 Agent 自动修改并暂存文本冲突
- **账号绑定**：绑定 GitHub / Gitee / GitLab 账号，浏览仓库、一键克隆添加

### 阅读与文档

- **文件预览**：项目文件树、CodeMirror 6 语法高亮、vscode-icons 文件图标
- **Markdown 预览**：README 抽屉式渲染（Shiki 高亮、代码复制/折叠、表格复制与导出 CSV/TSV/MD），四套 MD 主题
- **AI Wiki**：自动生成结构化 Wiki，支持增量更新与整本重生成，可经 ACP Agent 驱动

### AI 能力

- **项目问答**：基于项目上下文与 AI 对话（流式输出、思考过程展示、上下文占用指示），多轮会话与历史记录
- **AI 接入**：内置多厂商/模型目录与自定义接入配置；提示词管理；AI 用量统计（含缓存 token）
- **AI 资源库**：技能（Skill）市场浏览/安装/分组、通用 MCP 服务器定义、一键生成 AGENTS.md（自动对齐 CLAUDE.md 引用）、从 CC Switch 导入；可作 git 仓库备份同步远端，支持安全扫描
- **语义分析**：内置 sem sidecar，代码实体语义搜索、上下文与调用关系分析、未提交变更汇总

### 报告与自动化

- **报告生成**：按项目与日期生成日报/周报（AI 汇总提交与变更），报告历史归档
- **定时任务**：内置调度引擎，定时报告计划，结合中国工作日历跳过节假日
- **内置 CLI**：主程序首参数传入子命令(git/wiki/sem/project/report)即进入 CLI 模式,JSON 输出;配套 4 个内置技能(Skill)可一键导入资源库并部署到各 agent,详见 [CLI.md](CLI.md)

### 桌面体验

- **系统托盘**：托盘迷你弹窗速览项目、搜索并执行常用命令；关闭行为可配置（最小化到托盘/退出）；支持开机自启（静默驻留托盘）
- **多方式打开**：资源管理器 / VSCode（自动检测）/ 终端，编辑器图标自动缓存
- **自动更新**：内置更新检查与升级提示
- **个性化**：亮/暗主题 + island 皮肤、Markdown 主题、中/英文界面，设置持久化

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | Vue 3（`<script setup>`）、Vite、TypeScript、Pinia、Vue Router |
| UI | shadcn-vue（reka-ui）、Tailwind CSS v4、lucide 图标、vue-sonner |
| 编辑/图表 | CodeMirror 6（文件预览）、d3（Git 图谱）、ECharts（统计图） |
| 渲染 | vue-stream-markdown（Shiki 高亮） |
| 后端 | Tauri 2（Rust）、rusqlite（bundled SQLite）、tokio、clap（内置 CLI） |
| Sidecar | sem（代码语义分析，externalBin 内置） |
| 国际化 | vue-i18n（zh-CN 默认 / en-US 回退） |
| 工具链 | oxlint（静态检查）、oxfmt（代码格式化）、Vitest（单元测试） |

## 开发环境

- [Node.js](https://nodejs.org/) 18+ 与 [pnpm](https://pnpm.io/)（勿用 npm）
- [Rust](https://rustup.rs/) 工具链
- Tauri 2 系统依赖（Windows 需 WebView2，见 [Tauri 官方文档](https://v2.tauri.app/start/prerequisites/)）

## 常用命令

```bash
pnpm install        # 安装依赖
pnpm start          # 准备 sem sidecar 并 tauri dev:完整桌面端开发(前端 + Rust 热更新)
pnpm dev            # 仅 Vite 前端(端口 1420)
pnpm build          # 类型检查(vue-tsc)+ 前端构建(唯一的类型检查手段)
pnpm build:desktop  # 打包桌面安装包(Windows NSIS)
pnpm lint           # oxlint 静态检查
pnpm lint:fix       # 自动修复可修 lint 问题
pnpm format         # oxfmt 格式化 src/
pnpm format:check   # 仅检查格式(CI 用)
pnpm test:unit      # Vitest 单元测试(src/**/*.test.ts)
pnpm cli:dev -- git status -d .   # 以 CLI 模式运行主程序(不开窗口)
pnpm sem:prepare    # 下载/校验 sem sidecar 二进制(首次运行或 cargo 构建前需要)
pnpm release:local  # 本地发布流程调试(另有 check/build/sign/latest/all 子命令)
```

> 改动后验证：`pnpm lint` + `pnpm test:unit` + `pnpm build`；Rust 侧 `src-tauri/` 下 `cargo check` / `cargo test`（先 `pnpm sem:prepare`）。

## 目录结构

```
src/                    Vue 前端
  views/                页面:ProjectsHome / ProjectDetail / ProjectFiles / GitGraph
                        / ProjectWiki / ReportHistory / ResourceSkillPreview
                        / Settings / TrayPopup(托盘迷你弹窗)
  components/           ui/(shadcn-vue 生成,勿手改)+ 按域划分的业务组件
                        (ai-elements / chat / files / git / markdown / report / wiki 等)
  composables/          组合式函数,部分按子域分目录(files/ git/ wiki/)
  stores/               Pinia:projects / settings / tags / wiki / chat / ai-config
                        / background-tasks / batch-report / pins / project-assets 等
  i18n/locales/         zh-CN.ts、en-US.ts(键需对齐,locales.test.ts 校验)
  lib/                  cmd() Tauri 桥、路径/格式化等工具(单测与源码同目录)
src-tauri/
  src/commands/         Tauri 命令域(project / git / script / docker / files / wiki
                        / chat / ai / report / semantic / java / open / tag 等)
  src/agent/            内置 Agent(llm 适配、会话、压缩、工具)
  src/ai/               AI 接入(厂商/模型目录、SDK、Wiki 大纲、提示词)
  src/cli/              内置 CLI(首参数子命令进入,JSON 输出;技能文本在仓库根 skills/)
  src/scheduler/        定时任务调度引擎(日历 / 配置 / 执行 / 运行时)
  src/db/               SQLite 连接与迁移执行器(rusqlite)
  migrations/           SQL 迁移 NNN_name.sql(按 user_version 幂等应用,001~011)
scripts/                sem/(sidecar 下载与清单)、release/(发布流程)
skills/repomeow/        内置技能(SKILL.md 入口 + references/ 细分,随二进制打包)
```

## 数据存储

应用数据位于 `~/.repomeow/`（Windows：`C:\Users\<用户名>\.repomeow\`）：

- `projects.db` — SQLite 数据库（项目、标签、自定义命令、报告历史、AI 用量等）
- `settings.json` — 界面与功能设置（主题、语言等）
- `prompts/*.md` — AI 提示词
- `ai-config.json` — AI 接入配置
- `wiki/<项目名>-<hash>/` — 生成的项目 Wiki

运行期缓存（编辑器图标、中国节假日）位于安装目录 `data/`，不可写时静默降级。

## 推荐 IDE 配置

[VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) + [Oxc](https://marketplace.visualstudio.com/items?itemName=oxc.oxc-vscode)（oxlint + oxfmt 一体）
