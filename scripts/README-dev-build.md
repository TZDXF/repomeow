# 开发构建与测量

本次 Windows 数据与验证边界见 [TZD-91 实测报告](benchmarks/TZD-91.md)。

开发构建使用 `src-tauri/Cargo.toml` 的 `dev` profile。`pnpm start` 负责准备 sem、启动 Vite 和 Tauri 的 Rust 热更新；`pnpm cli:dev -- <参数>` 使用相同开发 profile。先安装依赖并准备 sidecar：

```powershell
pnpm install --frozen-lockfile
pnpm sem:prepare
```

## 调试信息与缓存取舍

默认开发 profile 使用 `debug = "line-tables-only"`，业务代码保留用于 panic 回溯的行表，第三方依赖 `debug = false`。这样减少符号生成、链接和磁盘占用，但默认构建不提供完整的局部变量/类型和依赖源码调试信息。WebView 前端开发者工具不受这一 Rust 调试信息设置影响。

需要完整 Rust 断点/变量及依赖调试时，使用 `dev-debug`。它显式恢复业务代码及依赖的 `debug = true`，避免继承默认开发 profile 的依赖通配覆盖。已有 Vite 开发服务时，可由调试器启动以下 Cargo 构建的程序：

```powershell
cargo build --locked --manifest-path src-tauri/Cargo.toml --bin repomeow --profile dev-debug
cargo run --locked --manifest-path src-tauri/Cargo.toml --bin repomeow-cli --profile dev-debug -- --help
```

Tauri 的 runner 参数通过 `--` 传给 Cargo；完整桌面开发调试可运行：

```powershell
pnpm sem:prepare
pnpm tauri dev --config src-tauri/tauri.dev.conf.json -- --profile dev-debug
```

`dev-debug` 首次需要重新编译，并在 `target/dev-debug` 单独保存产物，同时保留两套 profile 会增加磁盘占用。默认 `dev` 仍保留 Cargo 的增量缓存，优先保证日常小改动编译速度；`cargo test` 继承 `dev` 的调试信息设置，断言与溢出检查仍开启。未改变 crate 类型、依赖特性、优化级别、panic 策略或 release profile。

完整回退只需删除 Cargo.toml 最后的四个 profile 表；切换后首次构建会重新生成产物。清理缓存应按需进行（先停止占用可执行文件的开发服务）：

```powershell
cargo clean --manifest-path src-tauri/Cargo.toml --profile dev-debug
# 需要节省空间且接受下一次开发全量编译时，才清理默认开发产物
cargo clean --manifest-path src-tauri/Cargo.toml --profile dev
```

不要把 `cargo clean` 加到日常启动或编译命令，也不默认引入 sccache/新链接器或关闭增量缓存；它们需要额外环境与独立测量。更改 profile 后的旧缓存不会自动消失，配置带来的新目录大小下降不等于现有工作目录立即释放了同等磁盘空间。

配置依据：[Cargo 构建性能指南](https://doc.rust-lang.org/cargo/guide/build-performance.html)、[Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)。

## 可复现测量

在仓库根目录执行：

```powershell
node scripts/bench-dev.mjs --output src-tauri/bench-dev-baseline.local
```

切换优化配置后，使用另一个全新目录：

```powershell
node scripts/bench-dev.mjs --output src-tauri/bench-dev-optimized.local
```

测量期间关闭正在监听代码的 `pnpm start`，不要编辑 Rust 源码或同时运行其他构建。脚本拒绝复用已有输出目录，不清理已有缓存；输出目录后缀 `.local` 受仓库 gitignore 保护。父目录必须存在。仓库内的输出必须放在 `src-tauri/` 中（或选择仓库外的目录），避免 Vite 监听大量构建产物，在 Windows 链接期间出现 `EBUSY`。脚本依次执行：

1. `cargo fetch --locked --target <rustc host>`，单独记录依赖准备时间。已有 Cargo registry 缓存时，这是缓存检查，不能称为首次下载耗时。
2. 在 `src-tauri/` 执行 `cargo build --locked --offline --bin repomeow --target-dir <输出目录>/target --timings --message-format=json-render-diagnostics`。首次使用空 target，排除网络、保留系统文件缓存。
3. 完全相同的命令执行三次，测量无改动重建；用中位数比较，日志中的 `fresh` 表示 Cargo 复用了产物。
4. 将 `src/time_util.rs` 的 `now_utc().timestamp()` 临时改成 `now_utc().timestamp() + 0`，再执行相同构建命令。改动会触发业务库及桌面入口的编译和链接，不只是重新编译很小的 `main.rs`。

脚本在正常结束或构建失败时恢复源码字节；检测到并发源码改动时拒绝覆盖。强制结束脚本或断电会跳过恢复，需自行检查 `git diff -- src-tauri/src/time_util.rs`，仅撤回脚本的 ` + 0`。完成后源码时间戳改变，下一次构建可能再次编译，这是测量后的恢复开销。

每一步写入 `results.json` 和独立 `.log`，Cargo 的逐 crate 时间线位于 `target/cargo-timings/`。JSON 包含提交、工作树状态、Cargo.toml/Cargo.lock/探针源文件 SHA-256、工具链、CPU/内存及影响 profile 的环境变量。`seconds` 是 Cargo 子进程墙钟时间，包括 Cargo 自身、构建脚本、编译、链接和日志写入；不包括事后的目录统计。它不是 CPU 编译时间；Cargo 时间线可定位各 crate，但不能简单相加当作墙钟时间。

占用为 target 下所有文件逻辑字节之和，另列 `incremental` 和 `.pdb` 大小（两者是总量的组成部分）。不等于 NTFS 实际分配块，也不包括 pnpm store、node_modules、Cargo registry、sem 下载缓存和 WebView2 用户数据。先后两次测试可能受系统文件缓存、杀毒扫描和机器其他进程影响；首次与小改动各测一次时只能作为本机样本，不代表统计显著收益。

这组有限时长测量聚焦 Rust 桌面目标的编译，不启动应用，不包含 Vite、sidecar 下载、Tauri 启动到窗口可交互的时间，也不覆盖打包或发布构建。桌面开发启动、热更新和 CLI 功能须另外回归。
