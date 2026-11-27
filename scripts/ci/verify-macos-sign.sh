#!/usr/bin/env bash
# macOS 发布产物签名门禁:对三个会分发出去的载体逐一 `codesign --verify --strict`。
#
# 现阶段的签名是 ad-hoc(`APPLE_SIGNING_IDENTITY=-`,方案 A):它只保证产物自身
# 签名有效、资源密封成立,不保证 Gatekeeper 放行。所以本脚本**不**断言
# `spctl ... accepted`——没有 Developer ID 证书时 spctl 必然 rejected(exit 3),
# 断言它会让 job 永远红,等于把门禁变成噪声。
# 拿到 Developer ID 证书 + 公证凭据之后,在 workflow 里追加:
#   spctl -a -vvv -t exec <app>   期望 `accepted` 且 `source=Notarized Developer ID`
#   xcrun stapler validate <app>  期望 ticket 校验通过
#
# 用法:verify-macos-sign.sh <target-triple> [bundle 目录(仅本地联调用)]
set -euo pipefail

target="${1:?用法: verify-macos-sign.sh <target-triple> [bundle-dir]}"
bundle_dir="${2:-src-tauri/target/${target}/release/bundle}"

fail() {
  echo "::error::$*" >&2
  exit 1
}

# 逐文件签名校验:先 app 自身,再 Contents/MacOS 下的每个可执行文件。
# 不用 `--deep`:bundler 是按由内到外顺序签的,逐个点名校验比 --deep 的语义更明确。
verify_app() {
  local app="$1" carrier="$2"
  [ -d "$app" ] || fail "${carrier}: 找不到 .app($app)"
  # 资源密封是「已损坏」的直接对因:缺 _CodeSignature 的 bundle 会被 codesign 判为已改动
  [ -f "$app/Contents/_CodeSignature/CodeResources" ] ||
    fail "${carrier}: 缺少 Contents/_CodeSignature/CodeResources(资源未密封,签名必然无效)"
  codesign --verify --strict "$app" || fail "${carrier}: codesign --verify --strict 失败($app)"
  local exe
  for exe in "$app"/Contents/MacOS/*; do
    [ -f "$exe" ] || continue
    codesign --verify --strict "$exe" || fail "${carrier}: 子可执行文件签名无效($exe)"
  done
  echo "--- ${carrier}: $(basename "$app") 签名摘要 ---"
  codesign -dv --verbose=4 "$app" 2>&1 |
    grep -E '^(CodeDirectory|Signature=|TeamIdentifier=|Sealed Resources|Internal requirements)' ||
    true
}

tmp_dir="$(mktemp -d)"
mount_point=""
cleanup() {
  [ -n "$mount_point" ] && hdiutil detach "$mount_point" -quiet 2>/dev/null || true
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

# 载体 1:dmg 与 .app.tar.gz 的源 .app(打包阶段签好后被两个载体各自复制一份)
source_app="$(ls -d "$bundle_dir"/macos/*.app 2>/dev/null | head -1 || true)"
verify_app "${source_app:-$bundle_dir/macos/缺失.app}" "源 .app"

# 载体 2:updater 载荷 .app.tar.gz——用户升级时装的就是它,签名必须与源 .app 一致
tar_gz="$(ls "$bundle_dir"/macos/*.app.tar.gz 2>/dev/null | head -1 || true)"
[ -n "$tar_gz" ] || fail "找不到 updater 载荷 $bundle_dir/macos/*.app.tar.gz"
tar -xzf "$tar_gz" -C "$tmp_dir"
unpacked="$(ls -d "$tmp_dir"/*.app 2>/dev/null | head -1 || true)"
verify_app "${unpacked:-$tmp_dir/缺失.app}" "updater 载荷 $(basename "$tar_gz")"

# 载体 3:dmg——用户从 GitHub Releases 下载的就是它,里面那份 .app 也要单独校验
dmg="$(ls "$bundle_dir"/dmg/*.dmg 2>/dev/null | head -1 || true)"
[ -n "$dmg" ] || fail "找不到 $bundle_dir/dmg/*.dmg"
# -nobrowse 不弹 Finder;-readonly 只读挂载;挂载点取 hdiutil 输出的最后一列
mount_point="$(
  hdiutil attach -nobrowse -readonly "$dmg" 2>/dev/null |
    awk -F '\t' '/\/Volumes\//{print $NF}' | tail -1
)"
[ -n "$mount_point" ] || fail "dmg 挂载失败($dmg)"
app_in_dmg="$(ls -d "$mount_point"/*.app 2>/dev/null | head -1 || true)"
verify_app "${app_in_dmg:-$mount_point/缺失.app}" "dmg 内的 $(basename "$dmg")"

echo "macOS 签名门禁通过:源 .app / updater 载荷 / dmg 三个载体均为有效签名(ad-hoc 阶段)。"
