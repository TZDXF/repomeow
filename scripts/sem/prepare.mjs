import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { access, chmod, copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, extname, join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..", "..");
const MANIFEST_PATH = join(SCRIPT_DIR, "manifest.json");
const BIN_DIR = join(REPO_ROOT, "src-tauri", "binaries");
const CACHE_DIR = join(BIN_DIR, ".cache");

function parseArgs(argv) {
  let target = "";
  let checkOnly = false;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--target") {
      target = argv[++i] ?? "";
    } else if (arg.startsWith("--target=")) {
      target = arg.slice("--target=".length);
    } else if (arg === "--check") {
      checkOnly = true;
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return { target, checkOnly };
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

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function sha256(path) {
  const hash = createHash("sha256");
  await pipeline(createReadStream(path), hash);
  return hash.digest("hex");
}

async function download(url, destination) {
  const response = await fetch(url, {
    headers: { "User-Agent": "RepoMeow sem sidecar preparer" },
    redirect: "follow",
  });
  if (!response.ok || !response.body) {
    throw new Error(`下载失败: HTTP ${response.status} ${url}`);
  }
  const temporary = `${destination}.tmp-${process.pid}`;
  await pipeline(Readable.fromWeb(response.body), createWriteStream(temporary));
  await rename(temporary, destination);
}

function extractTarGz(archive, destination) {
  const result = spawnSync("tar", ["-xzf", archive, "-C", destination], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`解压 ${basename(archive)} 失败: ${result.stderr || result.error || "unknown error"}`);
  }
}

async function findFile(root, expectedName) {
  const { readdir } = await import("node:fs/promises");
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isFile() && entry.name === expectedName) return path;
    if (entry.isDirectory()) {
      const nested = await findFile(path, expectedName);
      if (nested) return nested;
    }
  }
  return null;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const manifest = JSON.parse(await readFile(MANIFEST_PATH, "utf8"));
  const target = args.target || process.env.TAURI_ENV_TARGET_TRIPLE || detectHostTarget();
  const spec = manifest.targets[target];
  if (!spec) {
    throw new Error(`sem ${manifest.version} 不支持构建目标 ${target}`);
  }

  const extension = target.includes("windows") ? ".exe" : "";
  const destination = join(BIN_DIR, `sem-${target}${extension}`);
  const metadataPath = `${destination}.json`;
  if ((await exists(destination)) && (await exists(metadataPath))) {
    const metadata = JSON.parse(await readFile(metadataPath, "utf8"));
    const binaryHash = await sha256(destination);
    if (
      metadata.version === manifest.version &&
      metadata.archiveSha256 === spec.sha256 &&
      metadata.binarySha256 === binaryHash
    ) {
      console.log(`✓ sem sidecar 已存在: ${destination}`);
      return;
    }
  }
  if (args.checkOnly) {
    throw new Error(`sem sidecar 不存在、版本不符或校验失败: ${destination}`);
  }

  await mkdir(CACHE_DIR, { recursive: true });
  const archive = join(CACHE_DIR, spec.asset);
  const expectedHash = spec.sha256.toLowerCase();
  let actualHash = (await exists(archive)) ? await sha256(archive) : "";
  if (actualHash !== expectedHash) {
    if (actualHash) await rm(archive, { force: true });
    const url = `${manifest.repository}/releases/download/v${manifest.version}/${spec.asset}`;
    console.log(`↓ 下载 sem ${manifest.version} (${target})`);
    await download(url, archive);
    actualHash = await sha256(archive);
  }
  if (actualHash !== expectedHash) {
    await rm(archive, { force: true });
    throw new Error(`sem 归档校验失败: expected=${expectedHash} actual=${actualHash}`);
  }

  const temporaryDir = await mkdtemp(join(tmpdir(), "repomeow-sem-"));
  try {
    if (!spec.asset.endsWith(".tar.gz")) {
      throw new Error(`尚不支持归档格式: ${extname(spec.asset)}`);
    }
    extractTarGz(archive, temporaryDir);
    const binary = await findFile(temporaryDir, spec.binary);
    if (!binary) throw new Error(`归档中未找到 ${spec.binary}`);
    const temporaryBinary = `${destination}.tmp-${process.pid}`;
    await copyFile(binary, temporaryBinary);
    if (!target.includes("windows")) await chmod(temporaryBinary, 0o755);
    const binarySha256 = await sha256(temporaryBinary);
    await rm(destination, { force: true });
    await rename(temporaryBinary, destination);
    const metadataTemporary = `${metadataPath}.tmp-${process.pid}`;
    await writeFile(
      metadataTemporary,
      JSON.stringify(
        {
          version: manifest.version,
          target,
          asset: spec.asset,
          archiveSha256: spec.sha256,
          binarySha256,
        },
        null,
        2,
      ) + "\n",
    );
    await rm(metadataPath, { force: true });
    await rename(metadataTemporary, metadataPath);
  } finally {
    await rm(temporaryDir, { recursive: true, force: true });
  }
  console.log(`✓ sem sidecar 已准备: ${destination}`);
}

main().catch((error) => {
  console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
