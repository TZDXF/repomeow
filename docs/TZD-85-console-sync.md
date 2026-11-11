## 基准与关键设计

基线：`dev` / `a2ea6edba167d9a4c6a25b39672c0c368f5cdab7`。

Windows GUI 子系统无法让 shell 可靠等待。新增 `repomeow-cli` console 二进制，直接调用现有 `cli::run_cli_blocking()`，与 GUI 共用命令实现但不启动 Tauri。Cargo `default-run=repomeow` 保持桌面开发入口；Tauri 自动构建、安装两个 Cargo binary。GUI 名称、快捷方式和更新入口保留。

同步采用数据根目录内的 `cli-write.lock` 与 `cli-revisions.json`。CLI 在解析成功、业务写入前取得 OS 文件锁，原子发布各数据域的单调递增版本，执行完成后释放锁。进程崩溃也由 OS 释放锁，无需清理“陈旧锁文件”。读命令和参数错误不发布版本（AI 配置读取可能播种文件，按写入处理）。GUI 后端每 500ms 尝试共享锁读取版本，写入进行中跳过；检测变化后先失效扫描/Git 缓存、唤醒调度，再广播 `data://cli-changed`。各窗口按域失效前端 LRU 并回读，保留原页面实例；前端另以 750ms 检查持久化快照兜底启动和漏收事件。事件没有内容、路径、凭据或业务数据。

正常刷新时限：后端在 CLI 完成后 500ms 内发现，正常约 1 秒加查询耗时；多次刷新顺序执行、合并中间变化，读取失败重试。系统休眠、WebView 暂停、长查询和持续写入时不能承诺可见刷新的硬实时上限。调度唤醒由后端负责，不依赖隐藏窗口的 JS 计时器。GUI 停止时无需通知，重开时正常从持久层加载。

## 写入与缓存清单

| CLI 路径 | 持久化 | GUI 刷新 |
| --- | --- | --- |
| project add/update/archive/unarchive/delete/set-* | projects.db；删除级联相关表 | 筛选列表、全量 ID 索引、归档列表、标签、详情、托盘、相关报告/Wiki |
| tag create/update/delete/set-project | tags/project_tags | 标签 store、项目索引及筛选结果 |
| script create/update/delete | custom_commands | 详情命令卡片、聚合 LRU、常用项 |
| pin set / hidden set | pinned_commands/hidden_items | pin store、脚本和 Compose 卡片隐藏状态 |
| report generate/history-delete/schedules-save/system-schedule-save | 报告/提交明细、报告调度及 system_schedules | 历史列表、展开报告、日历标注、调度设置；唤醒 scheduler/git monitor |
| ai config-save / 首次 config-get、models / test / usage-clear | ai-config.json、ai_usage_log | AI store、干净的设置草稿、模型选择器、用量统计 |
| prompt set | prompts/*.md | 提示词设置；未编辑字段自动更新，编辑中字段保留并提示 |
| account add/update/remove | git_accounts | 账号设置列表 |
| wiki generate/update/config-set/delete | wiki/<项目>/config.json、meta.json、pages、Wiki git、AI 用量 | 阅读 store、已打开的 Wiki 页、干净的配置对话框、用量统计 |
| file save | 项目内文本 | 后端 walk/Git 状态缓存、前端资产/聚合 LRU、文件树/当前预览/语义缓存、项目 AI 资产 |
| terminal input/create/start/stop/restart | GUI 内存会话与一次性 ipc 文件 | 原有桌面 IPC/terminal 事件；不走持久化版本 |
| sem 查询、project read-file/reports/commands、各 list/get（除播种） | 只读业务数据；可能有可再生工具缓存 | 不发布版本 |

每个数据域的版本均保留，连续两个 CLI 进程修改不同域不会覆盖失效信息。前端聚合缓存、项目列表与 AI 配置读取带请求代次保护，避免较早请求覆盖新值；Wiki 原有 loadVersions 与报告请求 token 继续生效。缓存项目页仍存在于原页面作用域中，未保存的命令、AI、提示词及 Wiki 配置草稿不因同步销毁。

## 一致性与失败语义

- 同数据目录的 CLI 写命令串行执行（包括需要等待 AI/网络的写命令）；不同数据目录互不阻塞。SQLite 开启 WAL、外键和 10 秒 busy timeout，与 GUI 的事务并存；集合替换沿用原有事务。
- JSON/项目文本以唯一临时文件原子替换，其他读者只看到完整旧版或新版。AI 配置的首次播种与保存共用进程间锁；全量配置保存按最后完成写入为准，不承诺合并两个全量草稿。
- 提示词的 CLI 读改写和 GUI 保存使用同一个文件锁，避免 CLI 只改一项时丢失其他并发修改。提示词集合仍是多个文件，进程异常时可能只完成部分文件；GUI 会回读实际结果。
- 同项目 Wiki 的 GUI/CLI 生成、增量更新、单页重生成、配置保存和删除共用操作锁；冲突操作返回错误，已有任务继续。锁放在 Wiki 目录旁，删除不会使锁失效。不同项目可独立生成。
- 参数错误在写入前返回 2，不发布版本。业务错误返回 1；若已进入写命令，则发布保守失效，即使没有实际改变。已有实现中的多文件/数据库与文件组合操作不升级为跨资源事务，失败或崩溃后 GUI 回读已提交的实际状态，不显示虚构的成功结果。
- 未保存的 GUI 配置继续保留，用户选择保存时可能覆盖 CLI 更新；干净草稿自动采用外部版本。此规则在界面提示，不静默丢弃用户编辑。
- GUI/CLI 的 SQLite、settings store、AI、提示词、Wiki、资源库及工作日业务路径统一使用 `REPOMEOW_DATA_DIR`。必须在各进程启动前设为同一个绝对路径。终端请求遇到已运行 GUI 的数据目录不同会拒绝执行，防止写入另一目录。

## 可复现验证入口

```powershell
pnpm install --frozen-lockfile
pnpm sem:prepare
pnpm lint
pnpm test:unit
pnpm build
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo build --release --locked --bins --manifest-path src-tauri/Cargo.toml
python scripts/test-cli.py --exe src-tauri/target/release/repomeow-cli.exe
powershell -NoProfile -File scripts/test-cli.ps1 -Exe src-tauri/target/release/repomeow-cli.exe
pwsh -NoProfile -File scripts/test-cli.ps1 -Exe src-tauri/target/release/repomeow-cli.exe
pnpm tauri build --bundles nsis
```

Python 回归校验 release PE console 标志、无 TTY 下 UTF-8/流分离/0/1/2、增删改/归档、失败写入、隔离目录、并发标签/提示词/文本写入。PowerShell 回归检查直接重定向、管道、变量接收和退出码。Rust 回归验证写锁、失败失效、并发域版本；前端回归验证重叠请求、重试、作用域销毁及旧响应不能复活缓存。`quality.yml` 对 `dev` PR 运行这些检查。

## 验证记录

2026-11-11 Windows 本机验证：

- `pnpm lint`、`pnpm format:check`、`pnpm build` 通过；前端 76 个文件、601 项测试通过，首屏入口 148.1 KiB。
- Rust：批量运行与定向复测分批覆盖 1124 项通过、7 项原有 ignored。新写锁、并发域版本、Wiki 冲突、失败失效及技能入口契约测试均通过。
- 两项既有 `agent::harness::env::tests::exec_timeout_kills_command` / `exec_abort_cancels_command` 在沙箱内未能终止循环子进程，首轮全量检查中止；在系统上下文单独复测两项均通过，遗留循环也已清理。首轮 Git/Wiki 测试失败在沙箱外均通过。现有 `cargo fmt --check` 在未改动的上游文件中也有差异，改动的 Rust 文件已单独格式化。
- 优化 release console PE subsystem=3；`scripts/test-cli.py` 59 次调用通过，含 cmd 原始 UTF-8 重定向、PATH 命中、账号本机 API 样本、增删改、并发写入和隔离目录。Windows PowerShell 5.1.26100.9836 与 PowerShell 7.6.9 的 `test-cli.ps1` 均通过。
- 使用独立 `com.repomeow.tzd85.test` identifier/productName、数据目录和 WebView profile 构建优化 release GUI，经 CDP 连接实际 WebView 验证：项目增改/归档/恢复/删除、标签、自定义命令增改删、pin 级联、脚本扫描与隐藏/恢复、README 当前预览、提示词外部更新与未保存草稿、AI 配置、账号增改删、系统调度、Wiki 删除、报告删除均自动刷新。三次测量分别为 845ms、119ms、331ms，未手动刷新或重启页面。
- Wiki/报告使用本地固定数据，未调用付费 AI。未实测托盘弹窗、交互式 ConPTY、macOS/Linux 包。本机已有用户 GUI 在运行，未执行会关闭同名进程的 NSIS 静默安装；安装包生成成功，脚本包含 console 的安装/卸载项，隔离 PATH 调用通过。正式升级安装、更新签名及托盘窗口由质量工程师补验。
- 仅重新 bundle 已打过类型标记的同一测试二进制时，Tauri 提示找不到原始 `__TAURI_BUNDLE_TYPE` 标记；最终 NSIS 生成退出码为 0。本次未发布或执行 updater 签名。

分批库测试命令（在系统上下文执行）：

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --skip agent::harness::env::tests::exec_timeout_kills_command --skip agent::harness::env::tests::exec_abort_cancels_command
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib cli::skills::tests::builtin_skills_use_path_command
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib agent::harness::env::tests::exec_ -- --test-threads=1
```

本次测试 GUI、重复握手实例、HTTP 桥接器及遗留测试循环均已关闭，未终止用户已安装的 RepoMeow。
