#!/usr/bin/env bash
# 发布流程中 macOS 产物的签名 / 公证策略，供 release.yml 与 preview.yml 共用。
#
# 为什么需要它：Tauri 只有在拿到签名身份时才会重签 app bundle。没有 APPLE_CERTIFICATE /
# APPLE_SIGNING_IDENTITY 时，bundle 里只剩 rustc 链接期写下的 linker-signed ad-hoc 签名，
# `codesign --verify --strict` 会报 “code has no resources but signature indicates they
# must be present”，用户从浏览器下载（带 com.apple.quarantine）后系统直接判定产物损坏，
# 提示“已损坏，无法打开”。因此无论凭据是否齐全，都必须保证发布产物的 bundle 签名自洽。
#
# 子命令：
#   plan              解析凭据组合、校验凭据有效性、导出可用凭据并决定签名模式
#                     （结果写入 $GITHUB_ENV），stdout 只输出模式
#   verify <bundle>   校验 <bundle>/macos/*.app、<bundle>/macos/*.app.tar.gz 与 <bundle>/dmg/*.dmg；
#                     signed 模式还会挂载 dmg 检查里面的 .app。加 --partial 时允许缺少 tar.gz / dmg
#                     （仅供本机只产出部分产物时使用；CI 一律要求完整）
#   notes [file]      输出追加到 release notes 的 macOS 说明（已签名时为空）；目标文件（默认
#                     $RUNNER_TEMP/release-notes.md）已含该段落时不再输出（重跑 publish 不重复追加）
#   verify --partial  允许缺少 tar.gz / dmg（仅供本机只产出部分产物时使用；CI 不带此参数）
#
# 模式（只接受这两个值，其它值一律报错退出）：
#   signed  Developer ID 证书 + 公证凭据齐全：正常签名 + 公证 + 装订票据
#   adhoc   没有任何 Apple 凭据：退化为 ad-hoc 重签，签名自洽可启动，但不满足 Gatekeeper
#
# 凭据只允许「全无」或「齐全」两种状态；只给一半会直接失败，避免静默产出半残产物。
# 特别注意：CI 里 `secrets.X` 缺失时会展开成空字符串，因此 plan 只把非空凭据写进
# $GITHUB_ENV——否则 tauri 会认为 APPLE_CERTIFICATE="" 是「有证书」而去导入空证书。
set -euo pipefail

NOTES_MARKER="## macOS 安装包说明（未公证）"
VALID_MODES="signed adhoc"

# 挂载中的 dmg 挂载点；在 EXIT trap 里卸载，保证任何失败路径都不会留下挂载。
MOUNT_POINT=""

die() {
  printf '::error::%s\n' "$*" >&2
  exit 1
}

cleanup_mount() {
  if [[ -n "$MOUNT_POINT" ]]; then
    local mp="$MOUNT_POINT"
    MOUNT_POINT=""
    hdiutil detach "$mp" >/dev/null 2>&1 || hdiutil detach -force "$mp" >/dev/null 2>&1 || true
    rmdir "$mp" 2>/dev/null || true
  fi
}
trap cleanup_mount EXIT

# 公证凭据状态：absent（一个都没有）/ partial（只给了几个）/ complete（某一组成齐全）。
# 支持两种组合：Apple ID（APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID）与
# App Store Connect API Key（APPLE_API_KEY + APPLE_API_ISSUER + 私钥：CI 用 base64
# APPLE_API_KEY_P8_BASE64，本机可直接给 APPLE_API_KEY_PATH）。
notarization_state() {
  local any=""
  if [[ -n "${APPLE_ID:-}${APPLE_PASSWORD:-}${APPLE_TEAM_ID:-}${APPLE_API_KEY:-}${APPLE_API_ISSUER:-}${APPLE_API_KEY_PATH:-}${APPLE_API_KEY_P8_BASE64:-}" ]]; then
    any=1
  fi
  if [[ -z "$any" ]]; then
    echo absent
    return 0
  fi
  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    echo complete
    return 0
  fi
  if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" ]] &&
    [[ -n "${APPLE_API_KEY_PATH:-}" || -n "${APPLE_API_KEY_P8_BASE64:-}" ]]; then
    echo complete
    return 0
  fi
  echo partial
}

certificate_state() {
  if [[ -z "${APPLE_CERTIFICATE:-}" && -z "${APPLE_CERTIFICATE_PASSWORD:-}" ]]; then
    echo absent
  elif [[ -n "${APPLE_CERTIFICATE:-}" && -n "${APPLE_CERTIFICATE_PASSWORD:-}" ]]; then
    echo complete
  else
    echo partial
  fi
}

resolve_mode() {
  local cert notary
  cert=$(certificate_state)
  notary=$(notarization_state)

  if [[ "$cert" == partial ]]; then
    die "APPLE_CERTIFICATE 与 APPLE_CERTIFICATE_PASSWORD 必须同时提供（只给了其中一个）"
  fi

  if [[ "$cert" == absent ]]; then
    if [[ "$notary" == partial ]]; then
      die "公证凭据不完整：APPLE_ID 需配套 APPLE_PASSWORD 与 APPLE_TEAM_ID；API Key 需配套 APPLE_API_ISSUER 与私钥（CI 传 APPLE_API_KEY_P8_BASE64，本机可给 APPLE_API_KEY_PATH）"
    fi
    if [[ "$notary" == complete ]]; then
      die "提供了公证凭据但缺 APPLE_CERTIFICATE / APPLE_CERTIFICATE_PASSWORD：没有 Developer ID 证书无法公证，请补齐证书或清空公证凭据走 ad-hoc 降级"
    fi
    if [[ -n "${APPLE_SIGNING_IDENTITY:-}" && "${APPLE_SIGNING_IDENTITY}" != "-" ]]; then
      die "只设置 APPLE_SIGNING_IDENTITY 无法签名：CI runner 的 keychain 里没有该身份，必须同时提供 APPLE_CERTIFICATE / APPLE_CERTIFICATE_PASSWORD"
    fi
    echo adhoc
    return 0
  fi

  if [[ "$notary" != complete ]]; then
    die "已提供 Developer ID 证书但缺少公证凭据（APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID 或 APPLE_API_KEY/APPLE_API_ISSUER/私钥）：Developer ID 签名的产物必须公证，否则 Gatekeeper 仍会拦截"
  fi
  echo signed
}

# 解析当前要使用的模式：显式给了 MACOS_SIGNING_MODE 时必须是白名单内的值，否则重新推导。
# 白名单是必要的：verify 在 signed 之外的任何值上都会走 adhoc 宽松分支，拼错就会静默放过坏产物。
resolve_mode_checked() {
  local mode="${MACOS_SIGNING_MODE:-}"
  if [[ -z "$mode" ]]; then
    resolve_mode
    return 0
  fi
  case " $VALID_MODES " in
    *" $mode "*) echo "$mode" ;;
    *) die "MACOS_SIGNING_MODE 只接受 $VALID_MODES，收到 \"$mode\"" ;;
  esac
}

# 写 $GITHUB_ENV（本地直接运行时变量为空，忽略即可）；GITHUB_ENV 不支持多行值，
# 因此调用方必须先把它压成单行（见 single_line / export_credentials）。
write_env() {
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    printf '%s=%s\n' "$1" "$2" >> "$GITHUB_ENV"
  fi
}

step_summary() {
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n' "$1" >> "$GITHUB_STEP_SUMMARY"
  fi
}

# GITHUB_ENV 是逐行文件格式，多行值只会写进第一行、其余行变成裸行（值被静默截断）。
# 证书 / p8 的 base64 从终端复制时常带换行与缩进，这里统一去掉所有空白再使用。
single_line() {
  printf '%s' "$1" | tr -d '\r\n\t ' | tr -d '\v\f'
}

require_openssl() {
  command -v openssl >/dev/null 2>&1 || die "需要 openssl 来校验 base64 凭据，但当前环境里没有"
}

# 解出的字节数；非法 base64 或解出 0 字节都视为无效（提前报错，把问题留在 plan 而不是 notarytool 阶段）。
base64_bytes() {
  require_openssl
  local decoded
  if ! decoded=$(printf '%s' "$1" | openssl base64 -d -A 2>/dev/null | wc -c); then
    die "凭据不是合法的 base64 内容"
  fi
  printf '%s' "$decoded" | tr -d ' \n'
}

# 把非空凭据透传给后续步骤（tauri 读的是环境变量，不认 GitHub secrets）。
export_credentials() {
  local name value
  for name in APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID \
    APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_PROVIDER_SHORT_NAME; do
    value="${!name:-}"
    if [[ -n "$value" ]]; then write_env "$name" "$value"; fi
  done

  # base64 类凭据先压成单行，否则多行值会把 $GITHUB_ENV 写坏、证书被截断。
  if [[ -n "${APPLE_CERTIFICATE:-}" ]]; then
    write_env APPLE_CERTIFICATE "$(single_line "$APPLE_CERTIFICATE")"
  fi

  # API Key 只以 base64 形式给出时落盘成文件（p8 私钥下载后只能看到一次）。
  if [[ -n "${APPLE_API_KEY_P8_BASE64:-}" ]]; then
    [[ -n "${RUNNER_TEMP:-}" ]] || die "APPLE_API_KEY_P8_BASE64 需要配合 RUNNER_TEMP 使用（本机可直接给 APPLE_API_KEY_PATH）"
    mkdir -p "$RUNNER_TEMP"
    local key_path="$RUNNER_TEMP/AuthKey_${APPLE_API_KEY:-key}.p8"
    if ! printf '%s' "$(single_line "$APPLE_API_KEY_P8_BASE64")" | openssl base64 -d -A > "$key_path" 2>/dev/null; then
      rm -f "$key_path"
      die "APPLE_API_KEY_P8_BASE64 不是合法的 base64 内容，无法生成 p8 私钥文件"
    fi
    # LibreSSL 的 `openssl base64 -d` 遇到非法字符不报错、直接输出空内容，所以还要看解出的字节数。
    local p8_bytes
    p8_bytes=$(wc -c < "$key_path" | tr -d ' \n')
    if [[ "$p8_bytes" -le 0 ]]; then
      rm -f "$key_path"
      die "APPLE_API_KEY_P8_BASE64 解出 0 字节：不是合法的 base64（应为 AuthKey_*.p8 的内容 base64 化）"
    fi
    chmod 600 "$key_path"
    write_env APPLE_API_KEY_PATH "$key_path"
  fi
}

# plan 阶段的凭据有效性校验：只做「能不能用」的判定，失败即报错退出，不做重试或降级。
validate_credentials() {
  local mode="$1" bytes

  if [[ "$mode" == signed ]]; then
    bytes=$(base64_bytes "$(single_line "${APPLE_CERTIFICATE:-}")")
    if [[ "$bytes" -le 0 ]]; then
      die "APPLE_CERTIFICATE 解出 0 字节：不是有效的 .p12 base64（导出方式应为 openssl base64 -A -in certificate.p12）"
    fi
    echo "APPLE_CERTIFICATE: base64 解析得到 $bytes 字节" >&2
  fi

  # P8_BASE64 落盘后这个文件刚创建；本机直接给 PATH 时文件必须已存在且非空。
  if [[ -n "${APPLE_API_KEY_PATH:-}" ]]; then
    if [[ ! -f "$APPLE_API_KEY_PATH" ]]; then
      die "APPLE_API_KEY_PATH 指向的文件不存在：$APPLE_API_KEY_PATH"
    fi
    bytes=$(wc -c < "$APPLE_API_KEY_PATH" | tr -d ' \n')
    if [[ "$bytes" -le 0 ]]; then
      die "APPLE_API_KEY_PATH 指向的文件为空：$APPLE_API_KEY_PATH"
    fi
  fi
}

cmd_plan() {
  local mode
  mode=$(resolve_mode)
  export_credentials
  validate_credentials "$mode"
  write_env MACOS_SIGNING_MODE "$mode"
  echo "$mode"

  if [[ "$mode" == signed ]]; then
    {
      echo "签名模式：Developer ID + 公证（tauri 会导入 APPLE_CERTIFICATE 并自动公证、装订票据）"
    } >&2
    step_summary "- macOS 产物：Developer ID 签名 + 公证（凭据齐全）"
    return 0
  fi

  # tauri 约定的 ad-hoc 伪身份：仍然会深层重签 app bundle，让签名自洽。
  write_env APPLE_SIGNING_IDENTITY "-"
  {
    echo "签名模式：ad-hoc 降级（未配置 Apple Developer ID 证书 / 公证凭据）"
    echo "  - bundle 会被重新签名，签名自洽、可启动，不再出现「code has no resources」坏签名"
    echo "  - 未公证：Gatekeeper 仍会拦截浏览器下载后的首次打开，release notes 会附用户处理说明"
  } >&2
  step_summary "- macOS 产物：**ad-hoc 签名降级**（未配置 Apple Developer ID 证书 / 公证凭据），未公证，release notes 已附用户处理说明"
}

# macOS runner 的 /bin/bash 是 3.2，`set -u` 下展开空数组会报 unbound variable，统一按下标循环。
verify_one_app() {
  local app="$1" strict="$2" failed=0

  echo "== 校验 $app =="
  codesign -dv --verbose=4 "$app" 2>&1 |
    grep -E '^(Identifier|Format|CodeDirectory|Authority|Signature|TeamIdentifier|Sealed Resources|Info\.plist|Internal requirements)' || true

  # 针对本次缺陷的回归守卫：没有 CodeResources 说明 bundle 从未被重签，只剩 linker-signed 签名。
  if [[ -f "$app/Contents/_CodeSignature/CodeResources" ]]; then
    echo "seal: 存在 Contents/_CodeSignature/CodeResources"
  else
    echo "::error::$app 缺少 Contents/_CodeSignature/CodeResources，bundle 未被重新签名（只有 linker-signed 签名）"
    failed=1
  fi

  if codesign --verify --deep --strict --verbose=2 "$app"; then
    echo "codesign --verify --deep --strict: 通过（签名自洽）"
  else
    echo "::error::$app 签名校验失败（codesign --verify --deep --strict 未通过）"
    failed=1
  fi

  if [[ "$strict" == signed ]]; then
    if xcrun stapler validate "$app"; then
      echo "stapler validate: 通过（公证票据已装订）"
    else
      echo "::error::$app 未装订公证票据（stapler validate 失败）"
      failed=1
    fi
    if spctl --assess --type execute -vv "$app"; then
      echo "spctl --assess: 通过（Gatekeeper 放行）"
    else
      echo "::error::$app 未通过 Gatekeeper 评估（spctl --assess 拒绝）"
      failed=1
    fi
  else
    # ad-hoc 只能保证签名自洽，Gatekeeper 按设计拒绝；记录结果但不作为失败。
    if xcrun stapler validate "$app"; then
      echo "stapler validate: 通过（公证票据已装订）"
    else
      echo "stapler validate: 无公证票据（ad-hoc 模式预期如此）"
    fi
    if spctl --assess --type execute -vv "$app"; then
      echo "spctl --assess: 通过"
    else
      echo "spctl --assess: 拒绝（ad-hoc 无 Developer ID 时的预期结果；不作为 CI 失败）"
    fi
  fi

  return "$failed"
}

# signed 模式对 dmg 的要求：不只是「有签名」（ad-hoc 也满足 codesign --verify），必须是 Developer ID 签名。
#
# 这里**只**把 `codesign -dv` 的 Authority / TeamIdentifier 当硬门槛，不要求 dmg 通过 Gatekeeper 的
# 磁盘镜像评估（`spctl --assess --type open --context context:primary-signature`）：Tauri（
# tauri-bundler 2.10.1）的公证与装订只对 `.app` 做（`src/bundle/macos/app.rs:137-150`），dmg 那一步
# 只签名、全程没有 notarize / staple（`src/bundle/macos/dmg/mod.rs:194-207`），所以 Tauri 产出的 dmg
# 必然是「Developer ID 签名 + 无公证票据」；拿它去要求 primary-signature 评估会把**正常产物**拒掉，
# 让 signed 分支整块卡死。dmg 的 Gatekeeper 评估结果只记录、不作门槛；公证与装订由 `.app` 一侧的
# stapler validate / spctl --assess --type execute 保证。
verify_dmg_signature() {
  local dmg="$1" mode="$2" failed=0 dv

  dv=$(codesign -dv --verbose=4 "$dmg" 2>&1) || true
  if [[ "$mode" == signed ]]; then
    if printf '%s\n' "$dv" | grep -q '^Authority=Developer ID Application'; then
      echo "codesign -dv: dmg 由 Developer ID Application 签名"
    else
      echo "::error::$dmg 在 signed 模式下不是 Developer ID 签名（ad-hoc 签名也满足 codesign --verify，因此这里必须看 Authority）"
      failed=1
    fi
    if printf '%s\n' "$dv" | grep -q '^TeamIdentifier='; then
      echo "codesign -dv: dmg 带 TeamIdentifier"
    else
      echo "::error::$dmg 在 signed 模式下没有 TeamIdentifier"
      failed=1
    fi
    # 记录性输出：Tauri 不给 dmg 装订公证票据（上游行为），被拒是预期结果，无论通过与否都不置 failed。
    if spctl --assess --type open --context context:primary-signature -vv "$dmg"; then
      echo "spctl --assess（primary-signature）: 通过（仅记录，不作门槛）"
    else
      echo "spctl --assess（primary-signature）: 拒绝（仅记录，不作门槛；Tauri 只给 .app 装订票据，dmg 无票据属上游行为）"
    fi
    return "$failed"
  fi

  # adhoc：tauri 对 "-" 身份故意不给 dmg 签名（上游 issue #12288），这里只记录，不失败。
  if codesign --verify --verbose=2 "$dmg" 2>&1; then
    echo "codesign --verify: 通过（dmg 有签名）"
  else
    echo "codesign --verify: dmg 自身未签名（ad-hoc 模式预期如此；决定能否打开的是 dmg 内 app 的签名）"
  fi
  return 0
}

# signed 模式挂载 dmg，检查用户双击时真正会启动的那份 .app。
# 只用 -nobrowse -readonly -noautoopen：不弹 Finder 窗口、不等待任何 GUI，挂载与卸载都能立即返回。
verify_dmg_contents() {
  local dmg="$1" mode="$2"
  [[ "$mode" == signed ]] || return 0

  local mp
  mp="$(mktemp -d "${RUNNER_TEMP:-/tmp}/repomeow-dmg-mount.XXXXXX")"
  echo "== 挂载 $dmg 检查内层 app =="
  if ! hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mp" "$dmg" >/dev/null; then
    rmdir "$mp" 2>/dev/null || true
    echo "::error::$dmg 无法挂载，无法确认用户双击时启动的 .app"
    return 1
  fi
  MOUNT_POINT="$mp"

  local failed=0 inner=()
  shopt -s nullglob
  inner=("$mp"/*.app)
  shopt -u nullglob
  if [[ ${#inner[@]} -eq 0 ]]; then
    echo "::error::$dmg 挂载后没有找到 .app"
    failed=1
  else
    local idx
    for ((idx = 0; idx < ${#inner[@]}; idx++)); do
      verify_one_app "${inner[idx]}" "$mode" || failed=1
    done
  fi

  cleanup_mount
  return "$failed"
}

cmd_verify() {
  local bundle_root="${1:-}" allow_partial=0 arg
  shift || true
  for arg in "$@"; do
    case "$arg" in
      --partial) allow_partial=1 ;;
      *) die "未知参数：$arg（用法：macos-signing.sh verify <bundle 目录> [--partial]）" ;;
    esac
  done
  [[ -n "$bundle_root" ]] || die "用法：macos-signing.sh verify <bundle 目录>，例如 src-tauri/target/release/bundle"

  # 先校验模式白名单再碰文件系统：非法值会在下面被当成 adhoc 宽松分支处理，必须先拦住。
  local mode
  mode=$(resolve_mode_checked)

  [[ -d "$bundle_root" ]] || die "找不到产物目录：$bundle_root"

  local failed=0 app dmg tarball idx app_count dmg_count tar_count
  local apps=() dmgs=() tarballs=()
  shopt -s nullglob
  apps=("$bundle_root"/macos/*.app)
  tarballs=("$bundle_root"/macos/*.app.tar.gz)
  dmgs=("$bundle_root"/dmg/*.dmg)
  shopt -u nullglob
  app_count=${#apps[@]}
  dmg_count=${#dmgs[@]}
  tar_count=${#tarballs[@]}

  [[ "$app_count" -gt 0 ]] || die "在 $bundle_root/macos 下没有找到 .app 产物"

  # 缺失产物本身是硬错误：发布矩阵固定产出 app/tar.gz/dmg，缺任何一个都说明打包没完成。
  # 本机只产出部分产物时用 --partial 显式放宽，CI 不带这个参数。
  if [[ "$allow_partial" != 1 ]]; then
    [[ "$tar_count" -gt 0 ]] || die "$bundle_root/macos 下没有 .app.tar.gz（自动更新包缺失）；本机只验证部分产物时加 --partial"
    [[ "$dmg_count" -gt 0 ]] || die "$bundle_root/dmg 下没有 .dmg（安装包缺失）；本机只验证部分产物时加 --partial"
  fi

  for ((idx = 0; idx < app_count; idx++)); do
    app="${apps[idx]}"
    verify_one_app "$app" "$mode" || failed=1
  done

  # 自动更新包必须是同一个已签名 app（tauri 先签 app 再打 tar.gz），否则升级安装后照样坏签名。
  local unpack_dir unpacked_app
  for ((idx = 0; idx < tar_count; idx++)); do
    tarball="${tarballs[idx]}"
    echo "== 校验自动更新包 $tarball =="
    unpack_dir="$(mktemp -d "${RUNNER_TEMP:-/tmp}/repomeow-macos-update.XXXXXX")"
    if tar xzf "$tarball" -C "$unpack_dir" --strip-components=0 2>/dev/null; then
      shopt -s nullglob
      unpacked_app=("$unpack_dir"/*.app)
      shopt -u nullglob
      if [[ ${#unpacked_app[@]} -gt 0 ]]; then
        verify_one_app "${unpacked_app[0]}" "$mode" || failed=1
      else
        echo "::error::$tarball 里没有 .app，自动更新包内容异常"
        failed=1
      fi
    else
      echo "::error::$tarball 无法解包，自动更新包内容异常"
      failed=1
    fi
    rm -rf "$unpack_dir"
  done

  for ((idx = 0; idx < dmg_count; idx++)); do
    dmg="${dmgs[idx]}"
    echo "== 校验 $dmg =="
    verify_dmg_signature "$dmg" "$mode" || failed=1
    verify_dmg_contents "$dmg" "$mode" || failed=1
  done

  if [[ "$failed" != 0 ]]; then
    die "macOS 产物签名校验未通过（模式 $mode），见上方 ::error:: 行"
  fi
  echo "macOS 产物签名校验完成（模式 $mode）"
}

notes_section() {
  cat <<'EOF'

## macOS 安装包说明（未公证）

本次 macOS 产物未使用 Apple Developer ID 签名、未公证（构建时只做了 ad-hoc 重签，签名自洽）。浏览器下载的 dmg 会带上系统隔离标记（`com.apple.quarantine`），因此首次打开 RepoMeow 时 macOS 可能提示“无法验证开发者”，需要手动放行一次。这不是安装包损坏，而是未公证产物常见的拦截。

安装后可任选一种方式放行：

1. 在 Finder 中右键 RepoMeow.app →「打开」，再在弹窗里选「打开」；
2. 或到「系统设置 → 隐私与安全性」，在“已阻止使用”处点「仍要打开」；
3. 或用命令移除隔离标记后重新打开：

   ```bash
   xattr -dr com.apple.quarantine /Applications/RepoMeow.app
   ```

第 3 种只移除本地隔离标记，不改变签名状态；下次从网络下载的安装包仍会带上该标记。
EOF
}

# file（可选）是要追加到的 release notes 文件：已经含有该段落时什么都不输出，
# 使 publish 作业重跑（同一 tag 重试）不会把说明追加两遍。不传时默认看
# $RUNNER_TEMP/release-notes.md——两个 workflow 的 publish 都把说明写进这个文件，
# 因此无需改 workflow 也能幂等。
cmd_notes() {
  local target="${1:-}" mode
  if [[ -z "$target" && -n "${RUNNER_TEMP:-}" && -f "$RUNNER_TEMP/release-notes.md" ]]; then
    target="$RUNNER_TEMP/release-notes.md"
  fi
  mode=$(resolve_mode_checked)
  [[ "$mode" == adhoc ]] || return 0
  if [[ -n "$target" && -f "$target" ]] && grep -qF "$NOTES_MARKER" "$target"; then
    # 提示必须走 stderr：stdout 会被 publish 作业追加到 release notes 正文。
    echo "release notes 已有 macOS 未公证说明，跳过追加" >&2
    return 0
  fi
  notes_section
}

main() {
  local cmd="${1:-}"
  case "$cmd" in
    plan) cmd_plan ;;
    verify)
      shift
      cmd_verify "$@"
      ;;
    notes)
      shift
      cmd_notes "${1:-}"
      ;;
    *)
      die "用法：macos-signing.sh <plan|verify|notes> [参数]"
      ;;
  esac
}

main "$@"
