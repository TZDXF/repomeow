# macOS 发布签名与公证

本文说明 RepoMeow 的 macOS 发布产物（`.dmg` / 自动更新用的 `.app.tar.gz`）是怎么签名、公证的，凭据缺失时如何降级，以及「已损坏，无法打开」这类报错怎么判断和临时解决。

相关文件：

- `scripts/macos-signing.sh`：签名模式判定 + 打包后产物校验 + release notes 的 macOS 说明。
- `.github/workflows/release.yml`：macOS 两个矩阵 job 的 `Resolve macOS signing mode` / `Verify macOS bundle signatures` 步骤。
- `docs/build-and-test.md`：CI 与 macOS 本地复跑范围。

## 为什么会出现「已损坏，无法打开」

Tauri 只有在拿到签名身份时才会重签整个 `.app`。如果构建环境没有任何 Apple 凭据，产物里只剩 rustc 链接时写下的 linker-signed ad-hoc 签名，`Contents/_CodeSignature/CodeResources` 不存在，`Info.plist` 也没有被签名绑定。这类 bundle 的签名不自洽：

```bash
codesign -dv --verbose=4 RepoMeow.app   # flags=0x20002(adhoc,linker-signed)、Sealed Resources=none、Info.plist=not bound
codesign --verify --deep --strict RepoMeow.app
# RepoMeow.app: code has no resources but signature indicates they must be present
```

从浏览器下载的 dmg 会被打上 `com.apple.quarantine`。系统在首次打开时先按 Gatekeeper 评估，遇到这种不自洽的 bundle 就直接判定为损坏，弹「"RepoMeow.app"已损坏，无法打开。你应该将它移到废纸篓」。也就是说：

| 产物状态                     | 无 quarantine | 有 quarantine                                       |
| ---------------------------- | ------------- | --------------------------------------------------- |
| 只有 linker-signed 签名      | 能启动        | 打不开，提示「已损坏」                              |
| bundle 被重签（ad-hoc 自洽） | 能启动        | Gatekeeper 拒绝，提示「无法验证开发者」/ 需手动放行 |
| Developer ID 签名 + 公证     | 能启动        | 正常打开                                            |

因此发布链路的最低要求是 **bundle 签名自洽**，彻底解决需要 Developer ID 证书 + 公证。

v0.2.22 及更早版本的实测正好落在第一行：`RepoMeow_aarch64.app.tar.gz` 里的 `repomeow` / `sem` 只有 `LC_CODE_SIGNATURE` 指向的 ad-hoc CodeDirectory（`flags=0x20002`、无 CMS 证书、无 TeamIdentifier），整个 `Contents/_CodeSignature/` 目录都不存在；`x64` 包的可执行文件连 `LC_CODE_SIGNATURE` 都没有。这也是本地 `pnpm build:desktop` 出来的包在本机能跑、从 GitHub 下载后却报「已损坏」的原因——差别只在隔离标记。

## 签名模式

`bash scripts/macos-signing.sh plan` 依据环境变量判定模式，只接受「凭据全无」或「凭据齐全」，半套配置直接失败（`::error::` + 非零退出）：

- `signed`：`APPLE_CERTIFICATE` + `APPLE_CERTIFICATE_PASSWORD`（Developer ID Application 证书，base64 的 `.p12`）且公证凭据成组齐全。二选一：
  - Apple ID：`APPLE_ID` + `APPLE_PASSWORD`（App 专用密码）+ `APPLE_TEAM_ID`
  - App Store Connect API Key：`APPLE_API_KEY` + `APPLE_API_ISSUER` + 私钥。CI 走 `APPLE_API_KEY_P8_BASE64`（脚本落盘成 `APPLE_API_KEY_PATH`），本机可直接给 `APPLE_API_KEY_PATH` 指向已有的 `.p8`。
  - 可选 `APPLE_PROVIDER_SHORT_NAME`。

  此时 Tauri 正常签名、公证并装订票据。
- `adhoc`：没有任何 Apple 凭据。脚本把 `APPLE_SIGNING_IDENTITY` 设为 `-`，Tauri 仍会深层重签 bundle（自洽、可启动），但不公证：Gatekeeper 仍然拦，release notes 会自动追加用户处理说明。

之所以必须在没有凭据时显式给 `-`：Tauri 只在「有签名身份」时才执行签名步骤，默认（无身份）什么也不做，于是发布出去的仍是只有 linker-signed 签名的坏 bundle——这正是 v0.2.22 的成因。

`secrets.X` 缺失时 GitHub 会展开成空字符串，所以脚本只把**非空**凭据写进 `$GITHUB_ENV`；否则 Tauri 会把空的 `APPLE_CERTIFICATE` 当成「有证书」去导入空文件而失败。

`MACOS_SIGNING_MODE` 只接受 `signed` / `adhoc` 两个字面值；其它值（包括拼写成 `Signed`）一律 `::error::` 退出，不会被当成宽松的 `adhoc` 分支处理。

## 需要配置的 GitHub Secrets

签名/公证凭据放在 `update` Environment 下（`release.yml` 已声明 `environment: update`，仓库级 secrets 与它不互通）：

| Secret                                                           | 说明                                                                                  |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `APPLE_CERTIFICATE`                                              | Developer ID Application 证书 `.p12` 的 base64                                        |
| `APPLE_CERTIFICATE_PASSWORD`                                     | 导出 `.p12` 时设置的口令                                                              |
| `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`                  | Apple ID 公证方式（三者成组）                                                         |
| `APPLE_API_KEY` / `APPLE_API_ISSUER` / `APPLE_API_KEY_P8_BASE64` | API Key 公证方式（三者成组，与上一行二选一；workflow 只传 `P8_BASE64`，`APPLE_API_KEY_PATH` 仅供本机直接使用） |
| `APPLE_PROVIDER_SHORT_NAME`                                      | 可选，多团队账号公证时使用                                                            |
| `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`                           | 应用内更新包的 minisign 私钥，**与 Apple 代码签名/公证无关，必须保留**                |

任何一项 Apple 凭据都不存在时即为 `adhoc` 模式：发布照旧进行，但会在日志、`GITHUB_STEP_SUMMARY` 和 release notes 中明确标注未公证。

## 打包后自检

`bash scripts/macos-signing.sh verify <bundle 目录>`逐个检查。CI 里传的是 `src-tauri/target/<矩阵 target>/release/bundle`——两个 darwin job 都显式带了 `--target`，tauri 会把产物写进带三元组的子目录，不是 `target/release/bundle`：

1. **产物完整性（硬门槛）**：`macos/*.app`、`macos/*.app.tar.gz`、`dmg/*.dmg` 三者都必须至少存在一个，缺任何一个即 `::error::` + 非零退出（说明打包没完成）。本机只构建了部分产物时加 `--partial` 显式放宽，CI 不带此参数。
2. `macos/*.app`：`Contents/_CodeSignature/CodeResources` 必须存在（本次缺陷的回归守卫），`codesign --verify --deep --strict` 必须通过。
3. `macos/*.app.tar.gz`（自动更新包）：解包后对里面的 `.app` 做同样检查，避免升级安装后又是坏签名。
4. `dmg/*.dmg` 自身的签名：`adhoc` 模式只记录「dmg 自身未签名」而不失败（tauri 对 `-` 身份故意不给 dmg 签名，上游 issue #12288；决定能否打开的是 dmg 内 app 的签名）。`signed` 模式要求 dmg 由 Developer ID 签名（`codesign -dv` 里有 `Authority=Developer ID Application` 与 `TeamIdentifier`）——**只有 `codesign --verify` 不够**，因为 ad-hoc 签名的 dmg 在那一项也返回 0。Gatekeeper 的磁盘镜像评估结果只记录、不作门槛：Tauri 的公证与装订只对 `.app` 做，dmg 仅签名，因此 Tauri 产出的 dmg 必然没有公证票据。
5. **`signed` 模式还会挂载 dmg 检查里面的 `.app`**（`hdiutil attach -nobrowse -readonly -noautoopen -mountpoint <临时目录>`，`EXIT` trap 保证失败路径也会卸载）。用户双击启动的就是这份 app，外层 app 通过不代表它通过。
6. `signed` 模式额外要求对每个 `.app`（含 tar.gz 内层、dmg 内层）`xcrun stapler validate` 与 `spctl --assess --type execute` 通过；`adhoc` 模式记录这两项结果（必然被 Gatekeeper 拒绝）但不失败。

任一项失败即 `::error::` + 非零退出，该 macOS job 失败。tauri-action 在这之前已经把资产传上了 draft Release，所以「job 变红」是唯一的拦截点：转正前必须确认两个 macOS job 都是绿的，红着转正就等于把坏签名发了出去。

本机复现同样的检查（需要已有产物）：

```bash
# 无凭据时的 ad-hoc 模式；产物不完整时加 --partial
MACOS_SIGNING_MODE=adhoc bash scripts/macos-signing.sh verify "src-tauri/target/$(rustc -vV | sed -n 's/^host: //p')/release/bundle"
```

自动化回归测试：`src/lib/macos-signing.test.ts` 覆盖 `plan` 的状态转移、`notes` 输出与幂等，以及 `verify` 在任何 `codesign` 之前就返回的预检路径（跨平台可跑，不依赖 macOS）。需要真实产物与 macOS 专有工具的分支不在单测里，属于发布时验证。

## 发布说明

CI 创建的是 draft Release，正文由人工按 `release-tagger` 技能写入（见 `.agents/skills/release-tagger/SKILL.md` 第 8 步）。把正文写进文件后、`gh release edit` 之前，先追加 macOS 说明（`signed` 模式下该命令无输出）：

```bash
cp <草稿正文> "$RUNNER_TEMP/release-notes.md"     # 或直接编辑正文文件
bash scripts/macos-signing.sh notes "$RUNNER_TEMP/release-notes.md" >> "$RUNNER_TEMP/release-notes.md"
```

脚本先检查目标文件是否已含该段落，`gh release edit` 重跑不会追加两遍。

## 用户侧临时处理

未公证的产物装好后可任选一种方式放行：

1. Finder 中右键 RepoMeow.app →「打开」，弹窗里再点「打开」；
2. 「系统设置 → 隐私与安全性」→ 在「已阻止使用」处点「仍要打开」；
3. 命令移除隔离标记：

   ```bash
   xattr -cr /Applications/RepoMeow.app
   ```

「已损坏，无法打开」这种不自洽的坏签名（v0.2.22 及更早版本）第 1、2 种经常不生效，第 3 种有效。重签自洽之后，第 1、2 种才会变成常规可行的路径。

## 只有凭据持有者才能验证的部分

以下结论只能在配置了 Developer ID 证书 + 公证凭据、且真的跑一次发布时验证，代码审查与无凭据的 ad-hoc 路径都覆盖不到：

- `signed` 模式下 Tauri 是否真的导入了证书、notarytool 是否通过、票据是否装订成功（`xcrun stapler validate` / `spctl --assess --type execute` 的实际输出）。
- dmg 是否带上了 Developer ID 的 `Authority` 与 `TeamIdentifier`。
- 公证完成后从浏览器下载仍能直接双击打开（无 quarantine 提示）。

在此之前，能证实的是：无凭据路径（`adhoc`）的 `plan` 状态机、`verify` 的门槛行为（缺 `CodeResources` / `codesign --verify` 不通过会让 job 失败），以及这些行为有跨平台单测回归（`src/lib/macos-signing.test.ts`）。
