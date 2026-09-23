import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export function syncNotes(manifest, release) {
  const notes = release.body?.trim();
  if (!notes || notes === "See the assets to download and install this version.") {
    throw new Error("Release 正文为空或仍为占位提示，请先填写更新说明");
  }
  if (String(manifest.version).replace(/^v/, "") !== release.tag_name.replace(/^v/, "")) {
    throw new Error("latest.json 版本与 Release tag 不一致，拒绝覆盖");
  }
  return { ...manifest, notes };
}

function main() {
  const [tag, repo = "TZDXF/repomeow"] = process.argv.slice(2);
  if (!/^v\d+\.\d+\.\d+(?:[-+][\w.-]+)?$/.test(tag ?? "")) {
    throw new Error("用法: node scripts/release/sync-notes.mjs v<version> [owner/repo]");
  }
  const gh = (...args) => execFileSync("gh", args, { encoding: "utf8" });
  const release = JSON.parse(gh("api", `repos/${repo}/releases/tags/${tag}`));
  // 留存原文件用于审计/恢复；只改 notes，不重建安装包、不修改签名与下载地址。
  const dir = mkdtempSync(join(tmpdir(), "repomeow-release-notes-"));
  gh("release", "download", tag, "--repo", repo, "--pattern", "latest.json", "--dir", dir);
  const file = join(dir, "latest.json");
  const original = readFileSync(file, "utf8");
  const manifest = JSON.parse(original);
  const updated = syncNotes(manifest, release);
  if (manifest.notes === updated.notes) {
    console.log(`${tag}: 更新说明已同步`);
    return;
  }
  const backup = join(dir, "latest.original.json");
  writeFileSync(backup, original);
  console.log(`原始元数据备份: ${backup}`);
  writeFileSync(file, `${JSON.stringify(updated, null, 2)}\n`);
  gh("release", "upload", tag, file, "--repo", repo, "--clobber");
  const verifyDir = mkdtempSync(join(tmpdir(), "repomeow-release-verify-"));
  gh("release", "download", tag, "--repo", repo, "--pattern", "latest.json", "--dir", verifyDir);
  const actual = JSON.parse(readFileSync(join(verifyDir, "latest.json"), "utf8"));
  if (JSON.stringify(actual) !== JSON.stringify(updated)) {
    throw new Error("上传后元数据校验失败，请检查远端 latest.json");
  }
  console.log(`${tag}: latest.json.notes 已同步并验证，其他字段保持不变`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
