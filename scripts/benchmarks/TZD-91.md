# TZD-91 Windows 开发构建实测

这次配置将默认开发构建的业务调试信息改为行表，第三方依赖关闭调试信息，并保留 `dev-debug` 完整调试入口。主要可见收益是 target 逻辑大小下降；本机小改动样本耗时下降，完全缓存命中的重建未改善。release profile、Tauri 发布配置、依赖及业务源码均未改动。

## 版本与环境

- 基线：dev `958f9dece4018403ad4ed47df71c2207e32c61cf`。
- 优化配置实测版本：`53db68d5b66d784596aecba3aeae2c84f7e6c2f0`；后续补充报告及测量工具输出位置保护，Cargo 配置与 Rust 业务源码不再变动。
- 2026-11-13，同机 Windows `10.0.26320` x64，i7-12700K（20 逻辑 CPU），内存 68,448,337,920 bytes；同一磁盘、默认 Cargo 并行度。
- rustc `1.99.1 (58cf8f9c3 2026-10-13)`，Cargo `1.99.1 (5f94df478 2026-08-27)`，host `x86_64-pc-windows-msvc`；Visual Studio 18 Community 安装的最新 MSVC 目录 `14.51.36231`，Windows SDK `10.0.26100.0`。
- Node `24.21.0`、pnpm `11.10.0`；Cargo.lock SHA-256：`82c06ce9874377792771d864a28759aab1e4c63c686deca1b95b3485abde388a`，两轮相同。
- 两轮未设置 profile、Rust 编译参数或编译包装器的环境覆盖；使用相同 Cargo registry 缓存，分别创建空 target。顺序为基线后优化，未清理系统文件缓存，计时期间没有并行运行本任务的其他构建或测试。

## 命令与结果

测量脚本、命令与统计定义见 [开发构建与测量](../README-dev-build.md)。两轮均在 `src-tauri/` 使用相同命令，仅 target 输出路径不同：

```powershell
cargo build --locked --offline --bin repomeow --target-dir <独立空目录>/target --timings --message-format=json-render-diagnostics
```

每次计时后再统计目录，GiB = bytes / 2^30。这里的“大小”是逐文件逻辑字节之和，包含硬链接路径的重复计数，不是去重后的物理分配空间；不包含 Cargo registry、pnpm、sem 下载缓存等。所有构建退出码为 0。

| 场景 | 基线耗时（秒） | 优化耗时（秒） | 基线 target（GiB） | 优化 target（GiB） | 编译 / fresh artifact 数（两轮相同） |
| --- | ---: | ---: | ---: | ---: | --- |
| 首次构建（空 target、依赖已准备） | 691.48 | 667.14 | 10.532 | 4.459 | 604 / 0 |
| 无改动重建 1 | 35.32 | 25.17 | 12.243 | 5.088 | 2 / 602 |
| 无改动重建 2 | 2.09 | 2.34 | 12.243 | 5.088 | 0 / 604 |
| 无改动重建 3 | 1.91 | 2.44 | 12.243 | 5.088 | 0 / 604 |
| 小改动增量构建 | 54.04 | 35.99 | 12.233 | 5.069 | 2 / 602 |

无改动重建 1 两轮都重新编译了 `repomeow_lib` 和 `repomeow`，不能称为完全命中缓存；本轮未确定该首次额外重编译的根因。三次无改动重建中位数为 2.09 → 2.44 秒，完全 fresh 的两次也未改善。小改动为公共模块 `time_util.rs` 的 `now_utc().timestamp()` → `now_utc().timestamp() + 0`，触发业务库和桌面入口重编译/链接，结束后恢复了原始字节。

首次构建耗时仅少 3.5%，小改动耗时少 33.4%；二者各只有一个样本，未交错重复实验，不能据此声称统计显著或跨平台收益。首次 target 逻辑大小少 57.7%，小改动后少 58.6%，后者相差 7,691,939,047 bytes（约 7.16 GiB）。

| 小改动后的组成 | 基线 bytes | 优化 bytes | 基线 GiB | 优化 GiB |
| --- | ---: | ---: | ---: | ---: |
| target 总量 | 13,134,692,899 | 5,442,753,852 | 12.233 | 5.069 |
| incremental（总量的子集） | 3,566,094,297 | 1,354,271,434 | 3.321 | 1.261 |
| PDB（总量的子集） | 2,188,615,680 | 772,472,832 | 2.038 | 0.719 |

Cargo JSON 记录证实业务库 debuginfo 从 `2` 变为 `line-tables-only`、普通依赖（例如 serde）从 `2` 变为 `0`，两轮 `opt_level = 0`、`debug_assertions = true`、`overflow_checks = true`。保持增量编译，仍以缓存空间换取小改动速度；没有添加自动清理、sccache 或新链接器。

## 下载、缓存与实际构建的边界

每轮先单独执行 `cargo fetch --locked --target x86_64-pc-windows-msvc`，分别为 4.04 / 0.92 秒，日志均无下载，属于已有 registry 的缓存准备检查，不是首次联网下载对照，也不是优化收益。随后构建使用 `--offline`，上述构建耗时不含下载，但包含 Cargo 检查、构建脚本、rustc、原生编译、链接及日志输出。各 crate 的并行时间线保存在 Cargo timing HTML 中，不能简单相加当作墙钟时间。

前端依赖安装（pnpm 报告 166.5 秒）、sem 下载/解压/校验在测量前完成，未将这些耗时算入 Rust 编译，也没有对其开展 A/B。两轮的首次构建均为 0 fresh artifact；系统文件缓存、杀毒扫描和其他机器活动无法完全隔离。测量不含 Vite、Tauri 启动到窗口可交互的时间、release 编译或打包；未测 macOS/Linux、完整变量调试器交互及 `dev-debug` 的额外占用。

默认构建不再提供完整 Rust 局部变量/类型和依赖源码调试信息。需要时使用 `dev-debug`，会重新编译并在独立目录保存较大的缓存；回退方式和命令见开发构建文档。旧 profile 缓存不会自动释放，新目录的缩小不等于原有工作区会立即腾出同等空间。

## 独立复现

使用此 PR 的固定 SHA 创建干净、独立的检出，先安装依赖并准备 sem，确认没有运行代码监听或其他构建。在 PR 版本保留测量脚本，仅将 Cargo.toml 恢复为基线内容，即可测量原始 profile（其余 Rust 业务源码和 lockfile 与基线相同）：

```powershell
node -e "const f=require('node:fs'),c=require('node:child_process'); f.writeFileSync('src-tauri/Cargo.toml',c.execFileSync('git',['show','958f9dece4018403ad4ed47df71c2207e32c61cf:src-tauri/Cargo.toml']));"
node scripts/bench-dev.mjs --output src-tauri/bench-dev-baseline.local
git restore --source=HEAD -- src-tauri/Cargo.toml
node scripts/bench-dev.mjs --output src-tauri/bench-dev-optimized.local
git diff --exit-code -- src-tauri/src/time_util.rs src-tauri/Cargo.toml
```

输出目录必须不存在，已有目录请保留并另取名称。基线 manifest SHA-256 为 `1b61dacc379867d6a8742c382768f7e15b66295bb6cac0700a8ba0b28f12f8e9`；完整精度数值、源文件哈希、命令和工具链记录保存在交付附件中的两份 `results.json`，同时附 `.log` 和 Cargo timing HTML。

本轮计时使用的独立 `.local` 目录当时位于仓库根目录（原始 JSON 保留了实际路径），测量期间关闭了 Vite。后续桌面冒烟发现：若继续把该目录作为 Tauri target，Vite 会监听其中的可执行文件并在链接时触发 `EBUSY`。最终脚本因此拒绝仓库内 `src-tauri/` 以外的输出位置，文档复现命令使用已有 Vite 排除规则覆盖的 Rust 目录。该保护及路径变更不冒充已经重测的 A/B 数据；两轮历史数据仍对应上述实际路径，独立验收请用最终脚本重新采样。测量证据归档后已清理本次额外生成的基线 target，未清理共享依赖缓存。

## 回归

- `pnpm lint`、`pnpm format:check`：通过；`pnpm test:unit`：76 文件、601 测试通过；`pnpm build`：类型检查与 Vite 构建通过，入口 148.1 KiB / 500 KiB。Vite 仍有原有的大 chunk 提示。
- `cargo test --locked --offline --manifest-path src-tauri/Cargo.toml --target-dir <优化 target>`：系统用户上下文下 1124 通过、0 失败、7 按既有标记忽略；bin/doc test 无额外用例。保留既有 Rust 编译警告。首轮沙箱执行出现 Git 失败及无法正常结束超时测试的限制，已取消并确认该轮进程不再运行，不计作通过；结果以系统用户上下文的完整重跑为准。
- `cargo build --locked --offline --bins --manifest-path src-tauri/Cargo.toml --target-dir <优化 target>`：开发版 GUI/console 构建通过。`node scripts/test-cli.mjs --exe <优化 target>/debug/repomeow-cli.exe`：59 次调用通过；`node scripts/test-cli-shells.mjs --exe <同一程序>`：PowerShell 5.1/7 的成功、断言失败、清理失败六组退出码传播通过。测试使用隔离数据。
- 开发 test profile 下直接运行既有 `scheduler::runtime::hardening_tests::panic_is_isolated_from_following_tasks` 并设置 `RUST_BACKTRACE=1`：通过，回溯含 `src-tauri/src/scheduler/runtime.rs` 源码行号。未进行完整变量调试器交互。
- 测量工具：Node 语法、帮助、拒绝复用已有目录、拒绝仓库内 Rust 目录以外的输出、拒绝时不改源码均验证通过。两轮实际测量均恢复了探针源文件原始字节。
- Tauri dev + Vite / WebView2 `158.0.4370.16`（Playwright CLI `0.1.24`）：隔离 `REPOMEOW_DATA_DIR`、WebView 数据目录，通过首页加载、设置导航、`get_app_data_dir` IPC 及 CLI 登记临时项目后桌面可见性断言，并保存截图。
- Rust watcher 确认检测到公共模块修改并重编译、重新启动；第一次热更新后调试端口未恢复，未确认主界面，根因未确定。源码恢复触发的第二次热更新后，确认新的浏览器端点并再次通过界面/IPC/项目断言。两次完整 Tauri 重建日志分别为 2m18s、26.24s，与上表 Cargo 探针的路径/特性/启动条件不同，不纳入性能 A/B；首次热更新后 WebView 恢复需质量工程师重复验证。

桌面冒烟先准备 sem，再执行 `tauri dev --config src-tauri/tauri.dev.conf.json`，设置 `CARGO_TARGET_DIR=<src-tauri 下的优化 target>`、`REPOMEOW_DATA_DIR=<独立测试目录>/data`、`WEBVIEW2_USER_DATA_FOLDER=<独立测试目录>/webview`、`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=19291`。用 Playwright CLI 连接该端口；常规开发仍用 `pnpm start`，调试端口只是本次回归的环境设置。测试结束后关闭测试进程并恢复源码，不保留开发服务。

未运行本地 release 构建/打包、发布、合并或跨平台回归；发布 profile 与配置保持原样。独立验收请针对 PR 固定 SHA 重跑最终脚本，关注新输出路径的 A/B、首次热更新后的 WebView 恢复及需要完整变量调试时的 `dev-debug`。
