#!/usr/bin/env node
// sem sidecar 架构校验:`pnpm sem:prepare --target <triple>` 之后确认落盘二进制真的
// 与该三元组一致。判定逻辑在 scripts/sem/arch.mjs(纯函数、可单测),这里只负责读文件
// 与报错退出。
//
// 用法:node scripts/sem/verify.mjs --target aarch64-apple-darwin
// 退出码:0 = 架构与三元组一致;1 = 不一致 / 缺文件 / 元数据 target 不符。
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { assertArchMatches } from "./arch.mjs";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..", "..");
const BIN_DIR = join(REPO_ROOT, "src-tauri", "binaries");

function parseArgs(argv) {
  let target = "";
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--target") {
      target = argv[++i] ?? "";
    } else if (arg.startsWith("--target=")) {
      target = arg.slice("--target=".length);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return { target };
}

function detectHostTarget() {
  const result = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`无法执行 rustc -vV: ${result.stderr || result.error || "unknown error"}`);
  }
  const host = result.stdout.match(/^host:\s*(.+)$/m)?.[1]?.trim();
  if (!host) throw new Error("rustc -vV 未返回 host target");
  return host;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const target = args.target || process.env.TAURI_ENV_TARGET_TRIPLE || detectHostTarget();
  const extension = target.includes("windows") ? ".exe" : "";
  const binaryPath = join(BIN_DIR, `sem-${target}${extension}`);

  let buffer;
  try {
    buffer = await readFile(binaryPath);
  } catch (error) {
    throw new Error(
      `读取 ${binaryPath} 失败: ${error instanceof Error ? error.message : String(error)}` +
        ` (先运行 pnpm sem:prepare --target ${target})`,
    );
  }

  const actual = assertArchMatches(buffer, target);

  // 顺带核对 prepare 写下的元数据,防止旧 target 的 json 残留误导
  const metadataRaw = await readFile(`${binaryPath}.json`, "utf8").catch(() => null);
  if (metadataRaw === null) {
    throw new Error(`缺少 sidecar 元数据 ${binaryPath}.json(先运行 pnpm sem:prepare)`);
  }
  let metadata;
  try {
    metadata = JSON.parse(metadataRaw);
  } catch (error) {
    throw new Error(`${binaryPath}.json 不是合法 JSON: ${error.message}`);
  }
  if (metadata.target !== target) {
    throw new Error(`sidecar 元数据 target=${metadata.target} 与期望 ${target} 不一致`);
  }

  console.log(`✓ sem sidecar 架构校验通过: ${target} → ${actual} (${binaryPath})`);
}

main().catch((error) => {
  console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
