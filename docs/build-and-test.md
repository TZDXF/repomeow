本仓库是 Tauri 2 + Vue 3 + TypeScript 前端与 Rust 后端的桌面应用（本地开发项目管理中心）：前端由 Vite / vue-tsc / oxlint / oxfmt / Vitest 驱动，后端在 `src-tauri/` 下用 Cargo 构建，sem 以 externalBin sidecar 内置。包管理与所有入口一律用 **pnpm 11.10.0**，不用 npm。

开始改动前先记住几条禁止项：不要用 npm（用 pnpm）；不要 ad-hoc 写 `replace('\\', '/')`（路径归一化走 `src-tauri/src/path_util.rs` 与 `src/lib/path.ts`）；不要手改 `components/ui/` 下的 shadcn-vue 生成组件；不要修改已发布的 SQLite 迁移；不要提交密钥或个人配置；不要擅自合并、发布、部署或改权限。

## 环境前提

- Node `^20.19.0 || >=22.12.0`（`package.json` engines，与 Vite 8 一致；CI 使用 22）。
- pnpm 11.10.0（`packageManager` 字段；仓库存在 `pnpm-lock.yaml` / `pnpm-workspace.yaml`）。
- Rust 工具链与 Tauri 2 系统依赖（Windows 需 WebView2）。
- 首次安装依赖 `pnpm install --frozen-lockfile`；sem 二进制未准备时，任何 Cargo 构建前先跑 `pnpm sem:prepare`（下载到被 gitignore 的 `src-tauri/binaries/sem-<target>`，`pnpm sem:check` 只校验不下载）。

## 本地启动

| 命令 | 实际行为 |
| --- | --- |
| `pnpm start` | `sem:prepare && tauri dev --config src-tauri/tauri.dev.conf.json`，完整桌面端开发（前端 + Rust 热更新）。dev 经 `tauri.dev.conf.json` 覆盖 identifier 为 `com.repomeow.dev`，可与安装版同时运行 |
| `pnpm dev` | 仅 Vite 前端，端口 1420（见 `tauri.conf.json` 的 `devUrl`） |
| `pnpm cli:dev -- <args>` | `cargo run --manifest-path src-tauri/Cargo.toml --bin repomeow-cli --`，运行独立 console 目标，不开窗口 |

`package.json` 里还有两个辅助入口：`pnpm preview`（`vite preview`，预览已构建的 `dist`）和 `pnpm tauri`（Tauri CLI 直通，参数原样转发），日常上手不必使用。

## 构建

| 命令 | 实际行为 |
| --- | --- |
| `pnpm build` | `vue-tsc --noEmit && vite build && node scripts/check-entry-bundle.mjs`：类型检查 + 前端产物 + 首屏入口 500 KiB 预算检查。**这是唯一的类型检查手段**；`check-entry-bundle.mjs` 只是构建内部步骤，没有独立入口 |
| `pnpm build:desktop` | `sem:prepare && tauri build`，打出桌面安装包（NSIS） |

## 静态检查与格式化

| 命令 | 实际行为 |
| --- | --- |
| `pnpm lint` | `oxlint . --max-warnings=0`，零 warning 门禁（correctness / suspicious / perf 为 error，style 为 warn；忽略 `dist/`、`node_modules/`、`src-tauri/`、`scripts/`） |
| `pnpm lint:fix` | `oxlint . --fix`，自动修复可修问题 |
| `pnpm format` | `oxfmt --write src/` |
| `pnpm format:check` | `oxfmt --check src/`，CI 用 |

`components/ui/` 是 shadcn-vue 生成组件，不手改；相关规则已在 oxlint 配置中豁免。

## 测试

| 命令 | 实际行为 |
| --- | --- |
| `pnpm test:unit` | `vitest run`，node 环境、无 setup 文件，只收 `src/**/*.test.ts`（单测与被测源码同目录） |
| `pnpm test:cli --exe <程序>` | `node scripts/test-cli.mjs`，Node 内置模块驱动的 CLI 回归，校验 console 子系统、UTF-8、流分离、退出码 0/1/2、增删改/归档、隔离数据目录与并发写入 |
| `pnpm test:cli:shells --exe <程序>` | `node scripts/test-cli-shells.mjs`，按 Actions 包装方式跑 Windows PowerShell 5.1 + PowerShell 7 六组场景（正常完成 0、断言失败 1、清理失败 1），需同时安装两套 PowerShell |

两个 CLI 回归都需要已编译的二进制：本地先用 `cargo build --release --locked --bins --manifest-path src-tauri/Cargo.toml` 得到 `src-tauri/target/release/repomeow-cli.exe`。仅执行 `scripts/test-cli.ps1`（`powershell` / `pwsh` 两种宿主）不能覆盖 Actions 的退出码包装。

Rust 侧在 `src-tauri/` 目录下执行 `cargo check` / `cargo test`；`cargo check` 前若 sem 二进制未准备，先跑 `pnpm sem:prepare`。

## 发布脚本

`pnpm release:<子命令>` 调用 `scripts/release/release.mjs` 做本地发布流程调试，参数取 `check` / `build` / `sign` / `latest` / `all` / `local`（`local` 等于 `all --skip-build`）。这不是发布入口，正式发布由 tag 触发的 GitHub Actions 完成。

## CI 入口

`.github/workflows/quality.yml`（Quality）：`pull_request` 与 `push` 到 `main`、`dev` 触发，同组任务 `cancel-in-progress`。

- **frontend**（ubuntu-latest，pnpm 11.10.0 + Node 22）：`pnpm install --frozen-lockfile` → `pnpm lint` → `pnpm format:check` → `pnpm test:unit` → `pnpm build`。
- **rust**（windows-latest）：`pnpm sem:prepare --target x86_64-pc-windows-msvc` → `src-tauri/` 下 `cargo test --locked` → `pnpm install --frozen-lockfile` + `pnpm build` → `cargo build --release --locked --bins` → `pnpm test:cli --exe` → `pnpm test:cli:shells --exe` → `scripts/test-cli.ps1` 分别经 Windows PowerShell 5.1 与 PowerShell 7 直调。

`.github/workflows/release.yml`（Release）：`push` tag `v*` 或手动 `workflow_dispatch` 触发，单个 `release` job 按四目标矩阵并行（Windows NSIS、macOS arm64/x64 的 dmg + app、Ubuntu 22.04 的 deb + AppImage），统一由 `tauri-action` 写入同一个 draft Release。更新包签名私钥来自 `update` Environment 的 secrets；签名口令为空，workflow 会显式置为空字符串以避开 Tauri 2.11.4 signer 的静默失败。macOS 产物未做 Apple 公证，首次打开需右键 → 打开或 `xattr -cr`。

## 提交前建议

改动后至少组合验证 `pnpm lint` + `pnpm test:unit` + `pnpm build`；触及 Rust 则补 `cargo test`（先在 `src-tauri/` 就位、必要时 `pnpm sem:prepare`）；触及 CLI 输出、数据同步或退出码则补 `pnpm test:cli` 与 `pnpm test:cli:shells`。CI 在 `dev` 上也跑同一套质量门禁，本地跑通不等于 PR 通过，以 PR 上的 Quality 结果为准。
