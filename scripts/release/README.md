# 本地 release 流水线(备用)

> **自 v0.1.9 起,默认发版流程走 GitHub Actions CI**——推送 `v*` tag 后,`.github/workflows/release.yml` 用 `tauri-action` 在 CI 上构建、签名、上传并创建 draft Release,本机不需要打包、也不需要私钥。详细流程见 `.zcode/skills/release-tagger/SKILL.md`。
>
> 本脚本是上述 CI 流程的**本地等价物**,保留下来仅用于:
> - CI runner 临时不可用时,在本机出包
> - 排查签名(`pubkey` 交叉校验)或 updater 行为
> - 单独重签 / 重新生成 `latest.json`
>
> **不纳入默认发版流程**——主流程已不再调用 `pnpm release:all` / `pnpm release:build` / `pnpm release:sign` / `pnpm release:latest` / `pnpm release:local`。仅 `pnpm release:check` 仍被主流程用于版本号一致性自检。

## 前置条件

- Node 18+(脚本用原生 `node:fs` / `node:child_process`,无外部依赖)
- pnpm 11+(项目根 `package.json` 的 `packageManager` 已锁版本,通过 `corepack pnpm` 调度)
- **私钥在 `~/.tauri/` 下**(脚本会自动找到唯一的 `*.key` 文件);如果那里有多个,需要用 `--key` 指定
- 可选:`gh` CLI 已登录(本脚本**不**上传,见下方"上传")

## 私钥如何被发现

脚本默认**不**硬编码私钥文件名,而是按下面顺序解析:

1. **`--key <path>`**:直接用你给的路径,并交叉校验同目录下的 `<path>.pub` 与 `src-tauri/tauri.conf.json#plugins.updater.pubkey` 一致
2. **自动发现**:`~/.tauri/` 下扫描:
   - 唯一 1 个 `*.key` → 用它
   - 0 个 → 报错,提示用 `--key`
   - 2 个以上 → 报错,列出全部让你选
3. 自动发现时,如果同名 `*.key.pub` 与 `tauri.conf.json` 的 `pubkey` 不一致 → **直接报错并打印两边原文**,防止你用错私钥签了一个装不上 updater 的包

成功路径会打印:

```
✓ discovered key: C:\Users\TZDXF\.tauri\project-manger.key (matches tauri.conf.json pubkey)
```

## 快速开始

```sh
# 完整流程:check → build → sign → latest
pnpm release:all

# 已经构建过,只想重签 + 重写 latest.json
pnpm release:sign
pnpm release:latest
```

## 子命令

| 命令 | 作用 |
| --- | --- |
| `pnpm release:check` | 校验 `tauri.conf.json` / `package.json` / `Cargo.toml` 三处版本号一致 |
| `pnpm release:build` | 跑 `pnpm install --frozen-lockfile` + `pnpm build:desktop` 生成 NSIS 安装包 |
| `pnpm release:sign`  | 用私钥签名,产出 `.sig`,并交叉验证 .sig 内的 pubkey 与 tauri.conf.json 一致 |
| `pnpm release:latest`| 根据 .sig + tauri.conf.json 的 pubkey 写 `latest.json` |
| `pnpm release:all`   | 默认入口,串行执行 check → build → sign → latest |
| `pnpm release:local`  | 等价于 `release:all --skip-build`,只跑 sign + latest |

## 产物位置

`src-tauri/target/release/bundle/nsis/`(均在 gitignore 内):

- `RepoMeow_<ver>_x64-setup.exe` — NSIS 安装包
- `RepoMeow_<ver>_x64-setup.exe.sig` — 签名
- `latest.json` — updater 元数据

## 重要:Tauri 2.11.4 signer 的一个静默 bug

当你用 **没有密码** 的私钥时,如果 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 这个环境变量**未定义**(完全没设置),`tauri signer sign` 会:

1. 打印一行 `Signing without password.`
2. **退出码 0**,**没有任何错误信息**
3. **不写出 .sig 文件**

`cmdSign` 里强制把环境变量设为空串 `""`,绕开这个 bug。

`release.yml` 的 CI 里也做了同样处理(见 "Pin empty updater signing password" step)。

## 上传(本脚本不做)

`release:all` 完成后,产物在 `src-tauri/target/release/bundle/nsis/`。上传仍走手工:

```sh
gh release create v0.1.0 \
  --repo TZDXF/repomeow \
  --title "v0.1.0" \
  --notes "..." \
  --draft \
  src-tauri/target/release/bundle/nsis/RepoMeow_0.1.0_x64-setup.exe \
  src-tauri/target/release/bundle/nsis/RepoMeow_0.1.0_x64-setup.exe.sig \
  src-tauri/target/release/bundle/nsis/latest.json
```

后续打 tag / 触发 CI 见 `.zcode/skills/release-tagger/SKILL.md`。

## 与 CI 的关系

`.github/workflows/release.yml` 的 `release-windows` job 通过 `tauri-apps/tauri-action@v0` 在 CI 里跑 `build → sign → release create` 同一套事。**自 v0.1.9 起,CI 已成为默认主流程**——本脚本是其**本地等价物**,仅在 CI 不可用或排查时使用。两个流程产物期望一致(都签同一把私钥,pubkey 都来自 `tauri.conf.json`)。

## 故障排查

| 现象 | 原因 |
| --- | --- |
| `signer exited with code 1` + `Invalid symbol 32` | 私钥文件被加过换行 / 多余空白,让 `base64 -d` 解不开。原始私钥是**单行 base64** |
| 打印了 `Signing without password.` 但没 .sig | Tauri signer bug;检查 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 是否真的传到了子进程 |
| `Public key mismatch: ...` | `~/.tauri/*.key.pub` 与 `tauri.conf.json` 的 `pubkey` 不一致(私钥与配置错配)。要么把 tauri.conf.json 的 pubkey 改成跟 `.pub` 一样,要么用 `--key` 指向真正对应的私钥 |
| `No .key file found in ~/.tauri` | 没私钥;`cargo tauri signer generate` 生成一对,把 `.key` + `.key.pub` 放到 `~/.tauri/` 下,并把 `.pub` 内容(去掉换行后单行 base64)写进 `tauri.conf.json` 的 `plugins.updater.pubkey` |
| `Multiple .key files in ~/.tauri` | 目录里有多个私钥;用 `--key <path>` 指定 |
| `Cannot read github remote` | 没配 `github` remote;用 `--repo owner/name` 覆盖,或 `git remote add github git@github.com:TZDXF/repomeow.git` |
| `installer not found` | `--skip-build` 跳过了构建但产物不在;先跑 `pnpm release:build` |