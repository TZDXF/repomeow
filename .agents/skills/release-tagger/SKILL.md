---
name: release-tagger
description: 在此仓库（tzdxf/repomeow，Tauri 2 + Vue 3）发布新版本。读取 `src-tauri/tauri.conf.json` 的 `version` 字段作为发布号，做一致性检查与构建验证后，走**CI 打包发布**：本机 bump → commit → push → push `v*` tag → `.github/workflows/release.yml` 在 CI 上用 tauri-action 构建 NSIS、安装包签名、生成 `latest.json` 并自动创建 draft Release；本机不需要打包、不需要私钥。CI 跑完后本机无需任何动作，draft Release 视为已发布。在用户提及"发版"、"发布新版本"、"CI 打包"、"cut a release"、"v0.X.0"或想发起 GitHub Release 流程时调用此 skill——即便用户只是说"准备发版"，也走这条流程。
---

# CI 打包发布工作流

自 v0.1.9 起，发布流程全部由 **GitHub Actions** 完成：推送 `v*` tag 触发 `.github/workflows/release.yml`，在 `windows-latest` runner 上用 [`tauri-apps/tauri-action`](https://github.com/tauri-apps/tauri-action) 构建 NSIS 安装包、用 `update` Environment 下的 `TAURI_SIGNING_PRIVATE_KEY` 签名、上传 `RepoMeow_<ver>_x64-setup.exe` / `.sig` / `latest.json`，并创建 **draft Release**。本机只需要 bump 版本号、push commit、push tag，**不需要 `~/.tauri/` 私钥，不需要跑 `pnpm release:all`**。

旧的本地打包脚本 `scripts/release/release.mjs`（`pnpm release:all` 等）保留为可选项：仅在 CI 暂时不可用、本地需要复现构建、或排查签名问题时手动调用，**不纳入默认发版流程**。

## 前置条件

- Node 18+、pnpm 11+（仓库 `packageManager` 已锁版本）
- `gh` CLI 已登录 `TZDXF` 账号（仅用于事后用 `gh release view` 验证；本机不再 `gh release create`）
- 仓库 GitHub `update` Environment 已配置 `TAURI_SIGNING_PRIVATE_KEY`（CI 私钥；本机不需私钥）
- `git push` 可达 `github.com/TZDXF/repomeow`（remotes `github` 或 `origin`，CI workflow 监听 `push: tags: v*`）

## 何时调用

- 用户说"发版"、"发布新版本"、"准备发版"、"CI 打包"、"cut a release"
- 用户给出形如"发 0.2.0"、"v0.2.0"的具体版本号时，若与仓库当前 `version` 不一致，先 bump（见第 1 步）再继续——用户明确说"发布新版 X.Y.Z"即视为授权 bump

## 何时不调用

- 用户只想 bump 版本号、不真的发版 → 不调用
- 用户只想查看现有 tag → 不调用

## 工作流（按顺序执行，每步失败立即中止）

### 1. 读出版本号，必要时 bump

```bash
VERSION=$(grep -m1 '"version"' src-tauri/tauri.conf.json | sed -E 's/.*"version":\s*"([^"]+)".*/\1/')
REMOTE=$(git remote get-url github 2>/dev/null || git remote get-url origin)
```

期望 `REMOTE` 含 `github.com/TZDXF/repomeow`，否则中止并提示"仓库远程异常，请确认 github remote 配置"。

若用户要求的版本号 ≠ `$VERSION`，同步修改四处：

- `src-tauri/tauri.conf.json` 的 `"version"`
- `package.json` 的 `"version"`
- `src-tauri/Cargo.toml` 的 `version = "..."`
- `src-tauri/Cargo.lock` 中 `name = "repomeow"` 紧随的 `version`（改完 Cargo.toml 后跑任意 cargo 命令也会自动更新）

改完用 `pnpm release:check` 校验三处一致（脚本来自 `scripts/release/release.mjs`，仅做版本号三处一致性检查，本流程默认使用，不触发构建）。

### 2. 工作区状态

```bash
git status --short
git rev-list --count github/main..HEAD   # 或 origin/main,看哪个是主分支
```

若 `git status` 非空（未提交改动或 untracked 文件）：
- 默认行为：**中止**并提示"工作区有未提交改动，请先 commit"
- 如果改动只有版本号 bump 相关文件，允许代提交后继续，提交信息沿用历史风格：`🔖 chore(release): bump 版本号至 X.Y.Z`，且必须明确告诉用户你做了什么

### 3. 构建验证

```bash
pnpm lint      # 0 errors 即可,style 告警不阻塞
pnpm build     # 含 vue-tsc --noEmit 类型检查,最高保真预发布闸门
```

任何一条失败 → 中止，把输出原样贴回给用户。CI 不会重新跑 lint/typecheck，**本机这两步是仅有的发布前闸门**。

### 4. 提交 bump 并推送 main

```bash
git add package.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json
git commit -m "🔖 chore(release): bump 版本号至 $VERSION"
git push github main
```

main 必须先于 tag 推送——`v*` tag 必须指向这次 bump 提交（CI 流程会 checkout 这个 tag，tauri-action 也会用 tag 推断 `__VERSION__`）。

### 5. 推送 `v*` tag 触发 CI

```bash
git tag -a "v$VERSION" -m "Release v$VERSION"
git push github "v$VERSION"
```

- 若 tag 已存在 → 中止并用 `git tag -l "v*"` 列出
- **推送 tag 后 CI 会自动启动**：触发 `.github/workflows/release.yml` 的 `release-windows` job，在 `windows-latest` 上拉取、打包、签名、上传资产、创建 draft Release
- 本流程不调用 `pnpm release:all`、不调用 `gh release create`——这些事 CI 全做了

### 6. 等待 CI 完成

```bash
gh run list --workflow=release.yml --limit 1
gh run watch <run-id>   # 同步等待,exit 0 即成功
```

CI 步骤概要（详见 `.github/workflows/release.yml`）：

1. checkout + 装 pnpm 11.10.0 + 装 Node 22
2. 设置 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""`（规避 Tauri 2.11.4 signer 静默 bug）
3. 装 Rust stable + x86_64-pc-windows-msvc target + cache
4. `pnpm install --frozen-lockfile`
5. `tauri-action@v0` 用 `secrets.TAURI_SIGNING_PRIVATE_KEY` 签名、生成 `latest.json`、上传 `RepoMeow_<ver>_x64-setup.exe` / `.sig` / `latest.json`、创建 **draft** Release

耗时主要在 Rust release 编译（约 1–3 分钟）。失败时 `gh run watch` 退出非 0，把日志贴回用户并建议查 `.github/workflows/release.yml` 排错。

### 7. 验证 draft Release

```bash
gh release view "v$VERSION" --json isDraft,isPrerelease,assets
```

期望：

- `isDraft: true`
- `assets` 含 `RepoMeow_<ver>_x64-setup.exe`、`.sig`、`latest.json` 三项
- `isPrerelease: false`

任何一项不符都先排查 CI 日志，不要把不完整的 Release 转正。

### 8. 发布验证（可选）

```bash
gh api repos/TZDXF/repomeow/releases/latest --jq '.tag_name'   # 可能仍是上一版,draft 不影响 latest
curl -sL https://github.com/TZDXF/repomeow/releases/latest/download/latest.json | jq .version
```

`gh api ... /releases/latest` 仍返回上一个**已发布**版（draft 不算 latest），这是预期。updater 端点 (`latest.json`) 的实际指向以 `releases/latest/download/latest.json` 资源为准——tauri-action 会上传 `latest.json` 资产，客户端 updater 读它即可。

完成后告诉用户：CI 已构建并以 draft 形式上传资产，旧版应用下次启动时若启用 updater，可自动检测到 `v$VERSION`。如需把 draft 转为正式 Release（让 `releases/latest` 立即指向新版本），用：

```bash
gh release edit "v$VERSION" --draft=false
```

默认 draft 状态即可——用户可手动在 GitHub UI 上点 Publish review。

## 失败模式速查

| 现象 | 原因 | 处置 |
|---|---|---|
| `git remote get-url github` 报错 | 该机器未配 `github` remote 名 | 中止，请用户用 `git remote -v` 检查 |
| `pnpm build` 失败 | TS 错误或前端构建错误 | 中止，把 build 输出原样贴回给用户 |
| tag 已存在 | 之前发过或本地残留 | 中止并附 `git tag -l "v*"` 输出 |
| 三处版本号不一致 | 用户或脚本只改了其一 | 中止并指出每个文件的当前值，等用户修齐再跑 |
| 工作区脏 | 还有未提交改动 | 默认中止；若是 bump 文件可代 commit 后继续，告知细节 |
| CI 报 "A public key has been found, but no private key" | 仓库 `update` Environment 缺 `TAURI_SIGNING_PRIVATE_KEY` | 中止；让用户去 GitHub Settings → Environments → update 配置密钥 |
| CI 报 "Signing without password." 但没 .sig | Tauri 2.11.4 signer 静默 bug | 已通过强制 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` 规避；如再现，检查 `.github/workflows/release.yml` 第 37–40 步是否被改掉 |
| CI 报 "Public key mismatch" | `tauri.conf.json` 里的 pubkey 与新 `TAURI_SIGNING_PRIVATE_KEY` 不匹配 | 中止；让用户确认密钥来源一致后再重发 tag（需先 `git tag -d vX.Y.Z && git push github :refs/tags/vX.Y.Z` 删旧 tag） |
| `gh run watch` 失败 / `assets` 缺文件 | CI 编译/上传失败 | 中止；贴 CI 日志给用户，按 `release.yml` 排错后重发 tag（先删旧 tag） |
| draft 已存在但本机没 tag | 之前发过又删了本地 tag | 中止；`gh release view vX.Y.Z` 看状态，必要时 `gh release delete` |

## 旧流程：本地打包（仅在 CI 不可用或排查时使用）

`scripts/release/release.mjs` 仍提供以下命令，**默认发版流程不调用它们**：

| 命令 | 用途 |
|---|---|
| `pnpm release:check` | 仅校验 `tauri.conf.json` / `package.json` / `Cargo.toml` 三处版本号一致——主流程仍用 |
| `pnpm release:build` | 本机 `pnpm build:desktop` 跑出 NSIS installer |
| `pnpm release:sign` | 用 `~/.tauri/*.key` 签 installer |
| `pnpm release:latest` | 写 `latest.json` |
| `pnpm release:all` | 上述四步串行 |
| `pnpm release:local` | `all --skip-build`，对已有 artifacts 重签 / 重写 latest |

使用场景示例：

- **CI runner 临时不可用** → 用 `pnpm release:all` 在本机出包，再用 `gh release upload vX.Y.Z <assets>` 上传到 draft Release
- **签名不匹配排查** → 在本机用 `pnpm release:sign` 验证 `~/.tauri/*.key.pub` 与 `tauri.conf.json` 一致性
- **本地手测 updater** → 用 `pnpm release:latest` 生成测试用 `latest.json`

详见 `scripts/release/README.md`。
