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
- **macos**（macos-latest，pnpm 11.10.0 + Node 22）：与上两个 job 并行且互不影响，不改动它们的门禁，也不影响 `release.yml`。`sw_vers` / `uname -m` 先记录真实系统与架构 → `pnpm install --frozen-lockfile` → `pnpm lint` → `pnpm format:check` → `pnpm test:unit` → `pnpm build` → 用 `rustc -vV` 的 host 三元组跑 `pnpm sem:prepare --target <host>`，再用 `pnpm sem:verify --target <host>` 读文件头**显式比较实际架构与三元组**（不一致退出 1），并跑 `pnpm sem:verify:test` 校验识别逻辑本身 → `src-tauri/` 下 `cargo test --locked -- --test-threads=1`（带一组 `--skip`，见下）。

macOS 侧的 `cargo test` 用 `--test-threads=1` 串行跑：`data_sync` / `wiki` 的文件锁用例以纳秒时间戳建临时目录，并行时偶发抢同一把锁。7 个用例暂时 `--skip`，原因都是断言本身写死 Windows 语义（Windows 路径字面量、`source == "brew"`、依赖 git 全局 `init.defaultBranch`），不是产品缺陷；逐个改成平台无关断言后应删除对应 `--skip`：

| `--skip` 的用例 | 原因 |
| --- | --- |
| `agent::harness::restricted_env::tests::unix_paths_stay_case_sensitive` | 默认 APFS 卷大小写不敏感，`README.MD` 能读到 `readme.md` |
| `commands::editor_icon::tests::exe_refs_expands_env_vars` / `exe_refs_jetbrains_toolbox_style` / `exe_refs_vscode_style_shim` | 用例输入是 `C:\` 路径，Unix 下 `Path::join` 不解析盘符；环境变量用例依赖 `SystemRoot` |
| `commands::toolchain::tests::go_detection_and_operations` | `can_update` / `can_uninstall` 要求 `source == "brew"`，用例传 `winget` |
| `terminal_ipc::tests::project_resolution_uses_registered_active_project` | 用例写死 `D:\projects\demo` |
| `commands::ai::resource_library::tests::sync_pull_push_status_and_remote_gone` | 本地 bare 仓库的默认分支名随 git 全局 `init.defaultBranch` 变化，删除当前分支被拒 |

`.github/workflows/release.yml`（Release）：`push` tag `v*` 或手动 `workflow_dispatch` 触发，单个 `release` job 按四目标矩阵并行（Windows NSIS、macOS arm64/x64 的 dmg + app、Ubuntu 22.04 的 deb + AppImage），统一由 `tauri-action` 写入同一个 draft Release。更新包签名私钥来自 `update` Environment 的 secrets；签名口令为空，workflow 会显式置为空字符串以避开 Tauri 2.11.4 signer 的静默失败。macOS 产物当前用 **ad-hoc 身份**（`APPLE_SIGNING_IDENTITY: "-"`，见 workflow 中该变量的注释）由 bundler 完成由内到外的 `codesign`，资源密封随之成立、`codesign --verify --deep --strict` 可通过；构建后由 `scripts/ci/verify-macos-sign.sh` 对源 `.app`、updater 载荷 `.app.tar.gz`、dmg 内的 `.app` 三个载体逐一校验签名，失败即 job 红。ad-hoc 不含 Developer ID 证书，**不做公证**，Gatekeeper 仍会拦：首次打开提示「Apple 无法验证“RepoMeow.app”是否包含可能危害 Mac 安全或泄漏隐私的恶意软件」，点「完成」后到「系统设置 → 隐私与安全性」点「仍要打开」即可（也可 `xattr -cr /Applications/RepoMeow.app` 一次性清属性）。要变成双击无提示，需换 Developer ID 证书 + 公证凭据（方案 C），届时门禁再追加 `spctl` / `stapler` 断言。

## macOS 本地复跑（Mac mini 真机）

CI 的 macos job 只是托管的 arm64 虚拟机；真机验收在同一套命令上复跑，先记录环境再逐条执行。整段可直接粘贴：开头 `set -e` 保证任一步失败即停（vim `env` / `bash` 都一样），不会再出现「sem 校验失败但仍继续跑后续命令」的假绿。

```bash
set -e   # 任一步非零立即中止；交互执行时请勿只用 `;` 串联
# rustup 装在 ~/.cargo 时，非登录 shell 可能没有 rustc：先确认 `rustc -V` 有输出，
# 否则下面的三元组解析会拿到空值，sem 与 Rust 步骤都会失败。
# 0. 环境记录（贴进验收结论）
sw_vers                       # macOS 版本
uname -m                      # 架构：arm64 / x86_64
xcode-select -p               # 命令行工具路径
node --version                # 需满足 ^20.19.0 || >=22.12.0
pnpm --version                # 11.10.0
rustc -vV | sed -n 's/^host: //p'   # host 三元组，决定 sem 资产

# 1. 依赖与前端门禁
pnpm install --frozen-lockfile
pnpm lint
pnpm format:check
pnpm test:unit                # 必须全绿：用例数随新增测试变化，以 vitest 汇总行的实际数字为准
pnpm build                    # 含 vue-tsc 类型检查与首屏 500 KiB 预算

# 2. 与架构匹配的 sem sidecar(每步都失败即停,set -e)
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
pnpm sem:prepare --target "$TRIPLE"
pnpm sem:verify --target "$TRIPLE"   # 文件头架构 ≠ $TRIPLE 时退出 1
pnpm sem:verify:test                 # 架构识别逻辑自身的回归

# 3. Rust 单测（与 CI 同一入口，同样串行 + 同一组 --skip）
cd src-tauri
cargo test --locked -- --test-threads=1 \
  --skip agent::harness::restricted_env::tests::unix_paths_stay_case_sensitive \
  --skip commands::editor_icon::tests::exe_refs_expands_env_vars \
  --skip commands::editor_icon::tests::exe_refs_jetbrains_toolbox_style \
  --skip commands::editor_icon::tests::exe_refs_vscode_style_shim \
  --skip commands::toolchain::tests::go_detection_and_operations \
  --skip terminal_ipc::tests::project_resolution_uses_registered_active_project \
  --skip commands::ai::resource_library::tests::sync_pull_push_status_and_remote_gone
```

- clippy 严格门禁（`cargo clippy --locked --all-targets -- -D warnings`）目前**在 macOS 上跑不过**：`#[cfg(windows)]` 分支在 Unix 上整体编译掉，触发 `unused import`（`commands/terminal.rs`、`open/shell.rs`）、`unused variable`（`commands/git/lifecycle.rs`）、`never constructed`（`open::ShellKind::PowerShell` / `GitBash`、`error::ErrorCode::MoveRobocopyFailed`、`env_refresh::apply_to_command`）以及 `docker.rs` 的 `large_enum_variant`，共 10 条 error。这些都需要按平台收窄 `cfg` 或单点 `#[allow]` 才能过，属于另一次改动，因此 macos job 只卡 `cargo test`，与 windows job 的 clippy 门禁互不影响。
- `pnpm test:cli` / `pnpm test:cli:shells` / `scripts/test-cli.ps1` 依赖 Windows PowerShell 与 `.exe` 产物，macOS 上不复跑。
- 真机若为 Intel（x86_64），`TRIPLE` 会解析成 `x86_64-apple-darwin`，`scripts/sem/manifest.json` 同样有对应资产与 SHA-256，无需改命令。

### 已知限制

- 未做 Apple 公证（只有 ad-hoc 签名）：`pnpm build:desktop` 的产物首次打开会被 Gatekeeper 拦下，提示「Apple 无法验证…是否包含可能危害 Mac 安全或泄漏隐私的恶意软件」，到「系统设置 → 隐私与安全性」点「仍要打开」即可，或 `xattr -cr /Applications/RepoMeow.app` 清掉 quarantine 属性。这不是「已损坏」——资源密封已有效，`codesign --verify --deep --strict` 能通过；「已损坏」只会出现在缺 `_CodeSignature` 的旧产物上。
- 上述 7 个 `--skip` 是尚未改写的用例断言；改成平台无关断言后 CI 与本地复跑命令需同步删掉对应行。
- `pnpm sem:verify` 只比对**文件头架构与三元组**，不校验 SHA-256（那是 `sem:prepare` 的职责），也不判断二进制能否运行。

## 提交前建议

改动后至少组合验证 `pnpm lint` + `pnpm test:unit` + `pnpm build`；触及 Rust 则补 `cargo test`（先在 `src-tauri/` 就位、必要时 `pnpm sem:prepare`）；触及 CLI 输出、数据同步或退出码则补 `pnpm test:cli` 与 `pnpm test:cli:shells`。CI 在 `dev` 上也跑同一套质量门禁，本地跑通不等于 PR 通过，以 PR 上的 Quality 结果为准。
