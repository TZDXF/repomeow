import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, expect, test } from "vitest";

// scripts/macos-signing.sh 的状态机回归:plan / notes 只依赖 shell + openssl,任何平台可复现,
// 因此放进跨平台也能跑的 vitest(frontend job 与 macos job 都跑 pnpm test:unit)。
// verify 只覆盖在任何 `codesign` 之前就返回的预检/门槛路径——需要真实产物与 macOS 专有
// 工具的分支属于发布时验证,不在这里重复。
const scriptPath = resolve(import.meta.dirname, "..", "..", "scripts", "macos-signing.sh");

const roots: string[] = [];

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "repomeow-macos-signing-"));
  roots.push(root);
  return { root, githubEnv: join(root, "github_env"), runnerTemp: join(root, "runner_temp") };
}

function cleanup() {
  for (const root of roots.splice(0)) {
    if (!root.startsWith(join(tmpdir(), "repomeow-macos-signing-"))) {
      throw new Error("unsafe cleanup");
    }
    rmSync(root, { recursive: true, force: true });
  }
}

/**
 * 按 GitHub Actions 的语义构造环境:每个 secret 对应的变量都存在、未配置时是空字符串,
 * 这样脚本里 `${VAR:-}` 与 `-n "$VAR"` 的行为与 CI 完全一致。
 */
function run(
  args: string[],
  secrets: Record<string, string> = {},
  opts: { withGithubEnv?: boolean; runnerTemp?: string } = {},
) {
  const fx = fixture();
  const env: Record<string, string> = {
    PATH: process.env.PATH ?? "",
    HOME: process.env.HOME ?? fx.root,
  };
  for (const name of [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_PATH",
    "APPLE_API_KEY_P8_BASE64",
    "APPLE_PROVIDER_SHORT_NAME",
    "APPLE_SIGNING_IDENTITY",
    "MACOS_SIGNING_MODE",
  ]) {
    env[name] = "";
  }
  Object.assign(env, secrets);
  env.RUNNER_TEMP = opts.runnerTemp ?? fx.runnerTemp;
  if (opts.withGithubEnv !== false) {
    env.GITHUB_ENV = fx.githubEnv;
  }
  rmSync(fx.runnerTemp, { recursive: true, force: true });

  const result = spawnSync("bash", [scriptPath, ...args], {
    env,
    encoding: "utf8",
    cwd: fx.root,
    timeout: 30000,
  });
  const envFile = existsSync(fx.githubEnv) ? readFileSync(fx.githubEnv, "utf8") : "";
  return { ...result, envFile, root: fx.root, runnerTemp: env.RUNNER_TEMP };
}

function envValue(envFile: string, name: string) {
  const line = envFile.split("\n").find((l) => l.startsWith(`${name}=`));
  return line ? line.slice(name.length + 1) : undefined;
}

/** 取出脚本里某个 shell 函数的函数体(`name() {` 到行首 `}`),用于纯静态断言。 */
function functionBody(name: string) {
  const source = readFileSync(scriptPath, "utf8");
  const start = source.indexOf(`\n${name}() {`);
  expect(start, `脚本里找不到函数 ${name}`).toBeGreaterThan(-1);
  const end = source.indexOf("\n}\n", start);
  expect(end, `函数 ${name} 没有闭合`).toBeGreaterThan(start);
  return source.slice(start, end);
}

afterEach(cleanup);

test("没有任何 Apple 凭据时降级为 ad-hoc,并通过 APPLE_SIGNING_IDENTITY=- 让 tauri 重签 bundle", () => {
  const r = run(["plan"]);
  expect(r.status).toBe(0);
  expect(r.stdout.trim()).toBe("adhoc");
  expect(envValue(r.envFile, "MACOS_SIGNING_MODE")).toBe("adhoc");
  // 关键:没有这一步,tarui 不会重签,产物就只剩 linker-signed 签名(用户看到「已损坏」)。
  expect(envValue(r.envFile, "APPLE_SIGNING_IDENTITY")).toBe("-");
  // 空字符串 secret 不能被写进 $GITHUB_ENV,否则 tauri 会去导入空证书。
  expect(r.envFile.split("\n").filter((l) => l.endsWith("=")).length).toBe(0);
});

test("只给证书或只给口令都直接失败,不写入任何模式", () => {
  const halfCredentials: Record<string, string>[] = [
    { APPLE_CERTIFICATE: "Zm9v" },
    { APPLE_CERTIFICATE_PASSWORD: "pp" },
  ];
  for (const secrets of halfCredentials) {
    const r = run(["plan"], secrets);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("::error::");
    expect(r.stderr).toContain("必须同时提供");
    expect(envValue(r.envFile, "MACOS_SIGNING_MODE")).toBeUndefined();
  }
});

test("有证书但缺公证凭据时失败(Developer ID 产物必须公证)", () => {
  const r = run(["plan"], { APPLE_CERTIFICATE: "Zm9v", APPLE_CERTIFICATE_PASSWORD: "pp" });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("缺少公证凭据");
});

test("只有公证凭据、没有证书时失败", () => {
  const r = run(["plan"], { APPLE_ID: "a@b.c", APPLE_PASSWORD: "p", APPLE_TEAM_ID: "t" });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("没有 Developer ID 证书无法公证");
});

test("只给 APPLE_SIGNING_IDENTITY(非 -)时失败:runner keychain 里没有该身份", () => {
  const r = run(["plan"], {
    APPLE_SIGNING_IDENTITY: "Developer ID Application: Example (ABCD123456)",
  });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("必须同时提供 APPLE_CERTIFICATE");
});

test("证书 + 口令 + Apple ID 三件套齐全 → signed,并把凭据透传给后续步骤", () => {
  const r = run(["plan"], {
    APPLE_CERTIFICATE: "Zm9v",
    APPLE_CERTIFICATE_PASSWORD: "pp",
    APPLE_ID: "a@b.c",
    APPLE_PASSWORD: "p",
    APPLE_TEAM_ID: "t",
  });
  expect(r.status).toBe(0);
  expect(r.stdout.trim()).toBe("signed");
  expect(envValue(r.envFile, "MACOS_SIGNING_MODE")).toBe("signed");
  expect(envValue(r.envFile, "APPLE_CERTIFICATE")).toBe("Zm9v");
  expect(envValue(r.envFile, "APPLE_TEAM_ID")).toBe("t");
  // signed 模式绝不能同时写 ad-hoc 伪身份,否则会把 Developer ID 签名覆盖掉。
  expect(envValue(r.envFile, "APPLE_SIGNING_IDENTITY")).toBeUndefined();
});

test("多行 base64 证书被压成单行写进 $GITHUB_ENV(否则值会被截断)", () => {
  const r = run(["plan"], {
    APPLE_CERTIFICATE: "Zm9v\nYmFy\n YmQ=\n",
    APPLE_CERTIFICATE_PASSWORD: "pp",
    APPLE_ID: "a@b.c",
    APPLE_PASSWORD: "p",
    APPLE_TEAM_ID: "t",
  });
  expect(r.status).toBe(0);
  expect(envValue(r.envFile, "APPLE_CERTIFICATE")).toBe("Zm9vYmFyYmQ=");
  expect(r.envFile.split("\n").filter((l) => l.trim() !== "" && !l.includes("=")).length).toBe(0);
});

test("非法 base64 证书在 plan 阶段就报错(不留到 notarytool)", () => {
  // 断言的是「解出 0 字节 → 报错」,不依赖 openssl 自身的退出码:LibreSSL 遇非法字符返回 0
  // 但输出空内容,Linux 的 OpenSSL 3 行为更严格;两种实现下字节数判定都成立。
  const r = run(["plan"], {
    APPLE_CERTIFICATE: "!!!",
    APPLE_CERTIFICATE_PASSWORD: "pp",
    APPLE_ID: "a@b.c",
    APPLE_PASSWORD: "p",
    APPLE_TEAM_ID: "t",
  });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("解出 0 字节");
});

test("证书 + API Key + P8_BASE64 → signed,p8 落盘 0600 且内容等于解出的明文", () => {
  const fx = fixture();
  const r = run(
    ["plan"],
    {
      APPLE_CERTIFICATE: "Zm9v",
      APPLE_CERTIFICATE_PASSWORD: "pp",
      APPLE_API_KEY: "K1",
      APPLE_API_ISSUER: "issuer",
      APPLE_API_KEY_P8_BASE64: "aGVsbG8=",
    },
    { runnerTemp: fx.runnerTemp },
  );
  expect(r.status).toBe(0);
  expect(r.stdout.trim()).toBe("signed");
  const keyPath = envValue(r.envFile, "APPLE_API_KEY_PATH")!;
  expect(keyPath).toBe(join(fx.runnerTemp, "AuthKey_K1.p8"));
  expect(existsSync(keyPath)).toBe(true);
  expect(readFileSync(keyPath, "utf8")).toBe("hello");
  expect(statSync(keyPath).mode & 0o777).toBe(0o600);
});

test("非法 P8_BASE64 在 plan 阶段报错且不留下空文件", () => {
  const fx = fixture();
  const r = run(
    ["plan"],
    {
      APPLE_CERTIFICATE: "Zm9v",
      APPLE_CERTIFICATE_PASSWORD: "pp",
      APPLE_API_KEY: "K1",
      APPLE_API_ISSUER: "issuer",
      APPLE_API_KEY_P8_BASE64: "!!!",
    },
    { runnerTemp: fx.runnerTemp },
  );
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("P8_BASE64");
  expect(existsSync(join(fx.runnerTemp, "AuthKey_K1.p8"))).toBe(false);
});

test("APPLE_API_KEY_PATH 指向不存在的文件时在 plan 阶段报错", () => {
  const r = run(["plan"], {
    APPLE_CERTIFICATE: "Zm9v",
    APPLE_CERTIFICATE_PASSWORD: "pp",
    APPLE_API_KEY: "K1",
    APPLE_API_ISSUER: "issuer",
    APPLE_API_KEY_PATH: "/nonexistent/AuthKey_K1.p8",
  });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("文件不存在");
});

test("notes:adhoc 模式输出 RepoMeow 的用户处理说明,signed 模式什么都不输出", () => {
  const adhoc = run(["notes"]);
  expect(adhoc.status).toBe(0);
  expect(adhoc.stdout).toContain("## macOS 安装包说明（未公证）");
  expect(adhoc.stdout).toContain("xattr -dr com.apple.quarantine /Applications/RepoMeow.app");
  // 说明里不能残留别的产品名(本脚本从 Pi-X 移植而来,路径必须指向 RepoMeow.app)。
  expect(adhoc.stdout).not.toMatch(/PiX/);

  const signed = run(["notes"], {
    APPLE_CERTIFICATE: "Zm9v",
    APPLE_CERTIFICATE_PASSWORD: "pp",
    APPLE_ID: "a@b.c",
    APPLE_PASSWORD: "p",
    APPLE_TEAM_ID: "t",
  });
  expect(signed.status).toBe(0);
  expect(signed.stdout).toBe("");
});

test("notes 幂等:目标文件已有该段落时不再输出(publish 重跑不会追加两份)", () => {
  const fx = fixture();
  const notesFile = join(fx.root, "release-notes.md");
  writeFileSync(notesFile, "release body\n");

  const first = run(["notes"], {}, { runnerTemp: fx.root });
  writeFileSync(notesFile, `release body\n${first.stdout}`);
  expect(readFileSync(notesFile, "utf8").split("## macOS 安装包说明").length - 1).toBe(1);

  const second = run(["notes", notesFile]);
  expect(second.status).toBe(0);
  expect(second.stdout).toBe("");
  expect(readFileSync(notesFile, "utf8").split("## macOS 安装包说明").length - 1).toBe(1);
});

test("非法 MACOS_SIGNING_MODE 直接报错,不会被 verify 当成 adhoc 静默放过", () => {
  const r = run(["verify", join(fixture().root, "bundle")], { MACOS_SIGNING_MODE: "Signed" });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain("只接受");
});

test("verify 的三条预检 die 路径:无参 / 目录不存在 / 目录里没有 .app", () => {
  const noArg = run(["verify"]);
  expect(noArg.status).toBe(1);
  expect(noArg.stderr).toContain("用法");

  const fx = fixture();
  const missing = run(["verify", join(fx.root, "nope")], { MACOS_SIGNING_MODE: "adhoc" });
  expect(missing.status).toBe(1);
  expect(missing.stderr).toContain("找不到产物目录");

  // 目录存在但没有 .app:应报「没有找到 .app 产物」,而不是去跑 codesign。
  const emptyBundle = join(fx.root, "empty-bundle");
  spawnSync("mkdir", ["-p", join(emptyBundle, "macos")]);
  const empty = run(["verify", emptyBundle], { MACOS_SIGNING_MODE: "adhoc" });
  expect(empty.status).toBe(1);
  expect(empty.stderr).toContain("没有找到 .app 产物");
});

test("verify 在无 --partial 时把缺失的 tar.gz / dmg 当硬错误", () => {
  const fx = fixture();
  const bundle = join(fx.root, "bundle");
  // 只要 .app 目录存在就会越过「没有 .app」预检,缺失的 tar.gz 应在 codesign 之前就报错。
  spawnSync("mkdir", ["-p", join(bundle, "macos", "RepoMeow.app", "Contents")]);
  const r = run(["verify", bundle], { MACOS_SIGNING_MODE: "adhoc" });
  expect(r.status).toBe(1);
  expect(r.stderr).toContain(".app.tar.gz");
});

test("dmg 的 Gatekeeper 磁盘镜像评估只记录、不作门槛(否则 Tauri 的正常产物会被拒)", () => {
  // 纯静态断言,不构造 dmg:signed 分支只把 Authority / TeamIdentifier 当硬门槛。
  // Tauri 只给 .app 公证装订, dmg 仅签名、没有票据,
  // 要求 dmg 通过 primary-signature 评估会拒掉正常产物。
  const body = functionBody("verify_dmg_signature");
  const spctlBlock = body.slice(body.indexOf("spctl --assess --type open"));
  expect(spctlBlock, "spctl 段落之后不应再有 failed=1").not.toMatch(/failed=1/);
  expect(spctlBlock).toContain("仅记录，不作门槛");
  // 两条硬校验仍在:它们是 ad-hoc 与 Developer ID 的分界。
  expect(body).toMatch(/grep -q '\^Authority=Developer ID Application'[^]*?failed=1/);
  expect(body).toMatch(/grep -q '\^TeamIdentifier='[^]*?failed=1/);
});

test("脚本里的提示路径都指向 RepoMeow.app,没有残留移植来的产品名", () => {
  const source = readFileSync(scriptPath, "utf8");
  expect(source).not.toMatch(/PiX/);
  expect(source).toMatch(/com\.apple\.quarantine \/Applications\/RepoMeow\.app/);
});
