# sem Sidecar

RepoMeow 将官方 `sem` CLI 作为 Tauri Sidecar 随安装包分发，用户无需单独安装。

## 常用命令

```powershell
pnpm sem:prepare                       # 下载 + SHA-256 校验,按 rustc host 决定 target
pnpm sem:check                         # 只校验已有二进制(版本 + 哈希),缺失即失败
pnpm sem:verify --target <triple>     # 读文件头比对架构与三元组,不一致退出 1
pnpm sem:verify:test                   # 架构识别逻辑(node --test)回归
```

`prepare.mjs` 从 `manifest.json` 读取固定版本、平台资产和 SHA-256，下载归档后校验并生成 Tauri 要求的目标三元组文件名。生成的二进制与归档缓存位于 `src-tauri/binaries/`，均被 Git 忽略。

`verify.mjs` 补的是 `prepare` 与 `check` 都不覆盖的一段：`sem:check` 校验版本与哈希，但哈希只对应 manifest 里登记的那份归档；一旦有人手工拷错架构的二进制、或缓存里残留旧 target 的产物，名字对而内容不对时 `sem:check` 看不出来。`verify` 直接读 Mach-O / PE / ELF 文件头判架构（`arch.mjs` 纯函数，`arch.test.mjs` 覆盖 arm64/x86_64、universal、损坏格式），与 `--target` 三元组不一致即非零退出；CI 的 macos job 与 `docs/build-and-test.md` 的本地复跑都用它当门禁。

升级步骤：

1. 更新 `manifest.json` 的版本、资产和归档 SHA-256。
2. 同步 `src-tauri/third-party/sem/NOTICE` 的版本。
3. 删除本机 `src-tauri/binaries/sem-*` 与 `.cache/` 后运行 `pnpm sem:prepare`。
4. 运行 Rust 单测、前端验证和安装包冒烟测试。

应用不得调用 `sem update/login/mcp`；Sidecar 版本仅随 RepoMeow 发布更新。Rust 后端只暴露固定语义分析命令，不接受前端传入任意 sem 参数。
