# RepoMeow · 喵库

基于 **Tauri 2 + Vue 3 + TypeScript** 的本地开发项目管理中心(桌面端)。把散落在各处的项目集中登记,在一个窗口里完成日常高频操作:跑脚本、管 Docker Compose、看 Git 状态并提交/推送、读 README、预览文件、生成项目 Wiki、向 AI 提问项目细节、定时产出日报/周报——还能通过内置 MCP Server 把这些能力开放给 Codex、Claude Desktop 等 AI 客户端。

## 功能特性

### 项目与脚本

- **项目管理**:添加/归档/删除本地项目,支持重新指定或移动项目目录;卡片与表格两种视图,按名称/描述搜索、标签筛选、收藏
- **脚本执行**:解析 `package.json` scripts 与自定义命令,分组折叠展示,一键在系统终端运行(Windows 优先 Windows Terminal,回退 cmd);命令可标记为常用,在托盘弹窗中快速执行
- **Docker Compose**:自动扫描 compose 文件,解析服务与端口,`compose ps` 运行状态指示(绿/黄/灰),浏览器直达服务端口
- **Java 支持**:JDK 检测与安装引导、Spring Boot 项目识别

### Git

- **状态与写操作**:状态总览、分支切换(本地+远程,自动建跟踪分支)、提交/拉取/推送;拉取冲突弹窗引导解决,push 被拒提示先拉取;未跟踪文件提交前显式勾选;支持自动拉取
- **提交图谱**:d3 渲染的分支/提交拓扑图(GitGraph)
- **Stash 与 Worktree**:Stash 列表、弹出/清理与差异查看;Worktree 管理
- **冲突解决**:合并冲突文件逐个处理,可交给内置 Agent 自动修改并暂存文本冲突
- **账号绑定**:绑定 GitHub / Gitee / GitLab 账号,浏览账号下仓库并一键克隆添加

### 阅读与文档

- **文件预览**:项目文件树浏览,CodeMirror 6 语法高亮预览,vscode-icons 文件图标
- **Markdown 预览**:README 抽屉式渲染(Shiki 代码高亮、代码复制/折叠、表格复制与导出 CSV/TSV/MD),四套 MD 主题
- **AI Wiki**:为项目自动生成结构化 Wiki,支持增量更新与整本重生成,可通过 ACP Agent 驱动

### AI 能力

- **项目问答**:基于项目上下文与 AI 对话(流式输出、思考过程展示、上下文占用指示),多轮会话与历史记录
- **AI 接入**:内置多厂商/模型目录,自定义接入配置;提示词管理;AI 用量统计(含缓存 token)
- **AI 资源库**:技能(Skill)市场浏览与安装、技能分组、通用 MCP 服务器定义管理、一键生成 AGENTS.md(并自动对齐 CLAUDE.md 引用)、从 CC Switch 导入;资源库可作为 git 仓库备份同步到远端,支持口令加密与安全扫描
- **语义分析**:内置 sem sidecar,提供代码实体语义搜索、上下文与调用关系分析、未提交变更汇总

### 报告与自动化

- **报告生成**:按项目与日期生成日报/周报(AI 汇总提交与变更),报告历史归档查看
- **定时任务**:内置调度引擎,支持定时报告计划,结合中国工作日历跳过节假日
- **内置 MCP Server**:主程序附加 `--mcp` 即以 stdio MCP Server 运行,工具按组开关(Git 提交 / Wiki 查询 / 语义分析 / 项目数据 / 报告生成),默认全部关闭,详见 [MCP.md](MCP.md)

### 桌面体验

- **系统托盘**:托盘迷你弹窗速览项目、搜索并执行常用命令;关闭按钮行为可配置(最小化到托盘/退出);支持开机自启(静默驻留托盘)
- **多方式打开**:资源管理器 / VSCode(自动检测)/ 终端,编辑器图标自动缓存
- **自动更新**:内置更新检查与升级提示
- **个性化**:亮/暗主题 + island 皮肤、Markdown 主题、中/英文界面,设置持久化

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | Vue 3(`<script setup>`)、Vite、TypeScript、Pinia、Vue Router |
| UI | shadcn-vue(reka-ui)、Tailwind CSS v4、lucide 图标、vue-sonner |
| 编辑/图表 | CodeMirror 6(文件预览)、d3(Git 图谱)、ECharts(统计图) |
| 渲染 | vue-stream-markdown(Shiki 高亮) |
| 后端 | Tauri 2(Rust)、rusqlite(bundled SQLite)、tokio、rmcp(内置 MCP Server) |
| Sidecar | sem(代码语义分析,externalBin 内置) |
| 国际化 | vue-i18n(zh-CN 默认 / en-US 回退) |
| 工具链 | oxlint(静态检查)、oxfmt(代码格式化)、Vitest(单元测试) |

## 开发环境

- [Node.js](https://nodejs.org/) 18+ 与 [pnpm](https://pnpm.io/)(勿用 npm)
- [Rust](https://rustup.rs/) 工具链
- Tauri 2 系统依赖(Windows 需 WebView2,见 [Tauri 官方文档](https://v2.tauri.app/start/prerequisites/))

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
pnpm mcp:dev        # 以 --mcp 模式运行(stdio MCP Server,不开窗口)
pnpm sem:prepare    # 下载/校验 sem sidecar 二进制(首次运行或 cargo 构建前需要)
pnpm release:local  # 本地发布流程调试(另有 check/build/sign/latest/all 子命令)
```

> 改动后建议验证:`pnpm lint` + `pnpm test:unit` + `pnpm build`;Rust 侧在 `src-tauri/` 下跑 `cargo check` / `cargo test`(需先 `pnpm sem:prepare`)。

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
  lib/                  cmd() Tauri 桥、路径/格式化等工具函数(单测与源码同目录)
src-tauri/
  src/commands/         Tauri 命令域(project / git / script / docker / files / wiki
                        / chat / ai / report / semantic / java / open / tag 等)
  src/agent/            内置 Agent(llm 适配、会话、压缩、工具)
  src/ai/               AI 接入(厂商/模型目录、SDK、Wiki 大纲、提示词)
  src/mcp/              内置 stdio MCP Server(--mcp 模式,工具按域拆分)
  src/scheduler/        定时任务调度引擎(日历 / 配置 / 执行 / 运行时)
  src/db/               SQLite 连接与迁移执行器(rusqlite)
  migrations/           SQL 迁移 NNN_name.sql(按 user_version 幂等应用,001~011)
scripts/                sem/(sidecar 下载与清单)、release/(发布流程)
```

## 数据存储

应用数据统一存放在用户主目录下的 `~/.repomeow/`(Windows: `C:\Users\<用户名>\.repomeow\`):

- `projects.db` — SQLite 数据库(项目、标签、自定义命令、报告历史、AI 用量等)
- `settings.json` — 界面与功能设置(主题、语言、MCP 工具组开关等)
- `prompts/*.md` — AI 提示词
- `ai-config.json` — AI 接入配置
- `wiki/<项目名>-<hash>/` — 生成的项目 Wiki

运行期缓存(编辑器图标、中国节假日数据)存放在安装目录 `data/` 下,不可写时静默降级。

## 推荐 IDE 配置

[VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) + [Oxc](https://marketplace.visualstudio.com/items?itemName=oxc.oxc-vscode)(oxlint + oxfmt 一体)
