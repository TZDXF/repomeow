import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const cwd = join(root, "src-tauri");
const source = join(cwd, "src/time_util.rs");
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== "--output") {
  console.log("Usage: node scripts/bench-dev.mjs --output src-tauri/<new-directory.local>");
  process.exit(args.includes("--help") ? 0 : 2);
}
const output = resolve(args[1]);
const within = (parent, child) => {
  const path = relative(parent, child);
  return path === "" || (path !== ".." && !path.startsWith(`..${sep}`) && !isAbsolute(path));
};
if (within(root, output) && !within(cwd, output)) {
  throw new Error("Put benchmark output inside src-tauri/ or outside the repository so Vite does not watch build artifacts");
}
const target = join(output, "target");
const hash = (value) => createHash("sha256").update(value).digest("hex");
const capture = (command, argv) => {
  const result = spawnSync(command, argv, { cwd: root, encoding: "utf8" });
  if (result.status !== 0) throw new Error(`${command}: ${result.stderr || result.error}`);
  return result.stdout.trim();
};

async function size(path) {
  const result = { bytes: 0, incrementalBytes: 0, pdbBytes: 0, files: 0 };
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`Unexpected symlink: ${child}`);
    if (entry.isDirectory()) {
      const nested = await size(child);
      result.bytes += nested.bytes;
      result.files += nested.files;
      result.pdbBytes += nested.pdbBytes;
      result.incrementalBytes += entry.name === "incremental" ? nested.bytes : nested.incrementalBytes;
    } else {
      const bytes = (await stat(child)).size;
      result.bytes += bytes;
      result.files += 1;
      if (entry.name.endsWith(".pdb")) result.pdbBytes += bytes;
    }
  }
  return result;
}

async function run(name, argv) {
  console.log(`${new Date().toISOString()} ${name}: cargo ${argv.join(" ")}`);
  const log = createWriteStream(join(output, `${name}.log`));
  const started = performance.now();
  let stderr = "";
  let stdout = "";
  let exitCode;
  try {
    exitCode = await new Promise((res, rej) => {
      const child = spawn("cargo", argv, { cwd, windowsHide: true });
      child.on("error", rej);
      child.stdout.on("data", (chunk) => { log.write(chunk); stdout += chunk; });
      child.stderr.on("data", (chunk) => { log.write(chunk); stderr += chunk; });
      child.on("close", res);
    });
  } finally {
    await new Promise((res) => log.end(res));
  }
  const seconds = (performance.now() - started) / 1000;
  const artifacts = stdout.split(/\r?\n/).flatMap((line) => {
    try { const value = JSON.parse(line); return value.reason === "compiler-artifact" ? [value] : []; }
    catch { return []; }
  });
  const result = { name, argv, seconds, exitCode,
    cargoFinished: stderr.match(/Finished[^\r\n]+/g)?.at(-1) ?? null,
    compiledArtifacts: artifacts.filter((item) => !item.fresh).length,
    freshArtifacts: artifacts.filter((item) => item.fresh).length };
  if (name !== "fetch") result.target = await size(target);
  report.runs.push(result);
  await writeFile(join(output, "results.json"), JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify(result));
  if (exitCode !== 0) throw new Error(`${name} failed; see ${join(output, `${name}.log`)}`);
}

// mkdir is deliberately non-recursive: never reuse or clean an existing build cache.
await mkdir(output);
const original = await readFile(source);
const modified = Buffer.from(original.toString("utf8").replace(
  "now_utc().timestamp()", "now_utc().timestamp() + 0",
));
if (original.equals(modified)) throw new Error("Incremental edit anchor missing");
const rustc = capture("rustc", ["-Vv"]);
const host = rustc.match(/^host: (.+)$/m)?.[1];
if (!host) throw new Error("Missing rustc host target");
const report = {
  startedAt: new Date().toISOString(),
  commit: capture("git", ["rev-parse", "HEAD"]),
  dirty: capture("git", ["status", "--short"]),
  rustc, cargo: capture("cargo", ["-V"]), node: process.version,
  os: `${os.platform()} ${os.release()} ${os.arch()}`,
  cpu: os.cpus()[0]?.model, logicalCpus: os.cpus().length, memoryBytes: os.totalmem(),
  environment: Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    /^(CARGO_PROFILE_|CARGO_BUILD_|RUSTFLAGS$|RUSTC_WRAPPER$|RUSTC_WORKSPACE_WRAPPER$|CARGO_INCREMENTAL$|TAURI_CONFIG$)/.test(key))),
  manifestSha256: hash(await readFile(join(cwd, "Cargo.toml"))),
  lockSha256: hash(await readFile(join(cwd, "Cargo.lock"))),
  sourceSha256: hash(original), edit: "now_utc().timestamp() -> now_utc().timestamp() + 0",
  sizeDefinition: "Sum of logical file sizes, not allocated filesystem blocks; target only, excludes Cargo registry and pnpm/sem caches",
  runs: [],
};
const build = ["build", "--locked", "--offline", "--bin", "repomeow", "--target-dir", target,
  "--timings", "--message-format=json-render-diagnostics"];
await run("fetch", ["fetch", "--locked", "--target", host]);
await run("cold", build);
for (let i = 1; i <= 3; i += 1) await run(`no-change-${i}`, build);
if (!(await readFile(source)).equals(original)) throw new Error("Source changed during benchmark; refusing to edit");
await writeFile(source, modified);
try {
  await run("incremental", build);
} finally {
  if (!(await readFile(source)).equals(modified)) {
    throw new Error("Concurrent source edit detected; refusing to overwrite it");
  }
  await writeFile(source, original);
}
console.log(`Results: ${join(output, "results.json")}`);
