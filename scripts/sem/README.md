# sem Sidecar

RepoMeow 将官方 `sem` CLI 作为 Tauri Sidecar 随安装包分发，用户无需单独安装。

## 常用命令

```powershell
pnpm sem:prepare
pnpm sem:check
```

`prepare.mjs` 从 `manifest.json` 读取固定版本、平台资产和 SHA-256，下载归档后校验并生成 Tauri 要求的目标三元组文件名。生成的二进制与归档缓存位于 `src-tauri/binaries/`，均被 Git 忽略。

升级步骤：

1. 更新 `manifest.json` 的版本、资产和归档 SHA-256。
2. 同步 `src-tauri/third-party/sem/NOTICE` 的版本。
3. 删除本机 `src-tauri/binaries/sem-*` 与 `.cache/` 后运行 `pnpm sem:prepare`。
4. 运行 Rust 单测、前端验证和安装包冒烟测试。

应用不得调用 `sem update/login/mcp`；Sidecar 版本仅随 RepoMeow 发布更新。Rust 后端只暴露固定语义分析命令，不接受前端传入任意 sem 参数。
