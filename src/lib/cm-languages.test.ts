import { describe, expect, it } from "vitest";
import { LanguageSupport, StreamLanguage } from "@codemirror/language";
import { resolveCmLanguage } from "@/lib/cm-languages";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 覆盖 CodeMirror 语言解析映射的关键路径:
// 1. 主流扩展名 → 官方 lezer(LanguageSupport);
// 2. legacy 语言(yaml/go/kotlin 等)与文件名映射(Dockerfile/.env)→ StreamLanguage;
// 3. 近似映射(vue→HTML)与无语法回退(.gitignore/bat/makefile)→ null;
// 4. 同语言命中缓存:同一扩展名/文件名返回同一个 Promise 实例。

describe("resolveCmLanguage", () => {
  it("主流扩展名映射到官方 lezer 语言", async () => {
    expect(await resolveCmLanguage("src/app/main.ts")).toBeInstanceOf(LanguageSupport);
    expect(await resolveCmLanguage("src/lib/diff.ts")).toBeInstanceOf(LanguageSupport);
    expect(await resolveCmLanguage("a/b.py")).toBeInstanceOf(LanguageSupport);
    expect(await resolveCmLanguage("main.rs")).toBeInstanceOf(LanguageSupport);
    // ts 变体与 c 系走同一官方包
    expect(await resolveCmLanguage("util.mts")).toBeInstanceOf(LanguageSupport);
    expect(await resolveCmLanguage("legacy.h")).toBeInstanceOf(LanguageSupport);
  });

  it("legacy 语言返回 StreamLanguage", async () => {
    expect(await resolveCmLanguage("conf/app.yml")).toBeInstanceOf(StreamLanguage);
    expect(await resolveCmLanguage("main.go")).toBeInstanceOf(StreamLanguage);
    expect(await resolveCmLanguage("src/Main.kt")).toBeInstanceOf(StreamLanguage);
    expect(await resolveCmLanguage("deploy.ps1")).toBeInstanceOf(StreamLanguage);
  });

  it("文件名映射(小写匹配)优先并覆盖扩展名语义", async () => {
    expect(await resolveCmLanguage("Dockerfile")).toBeInstanceOf(StreamLanguage);
    // ".env" 无扩展名语义(dot 在首位),靠完整文件名命中 properties
    expect(await resolveCmLanguage(".env")).toBeInstanceOf(StreamLanguage);
    expect(await resolveCmLanguage("build/.env")).toBeInstanceOf(StreamLanguage);
  });

  it("vue 近似为 HTML,无对应语法的文件回退 null", async () => {
    expect(await resolveCmLanguage("src/App.vue")).toBeInstanceOf(LanguageSupport);
    expect(await resolveCmLanguage(".gitignore")).toBeNull();
    expect(await resolveCmLanguage("scripts/run.bat")).toBeNull();
    expect(await resolveCmLanguage("Makefile")).toBeNull();
    expect(await resolveCmLanguage("some/file.xyzunknown")).toBeNull();
  });

  it("同语言命中缓存:相同 key 返回同一 Promise", async () => {
    const a = resolveCmLanguage("x/one.ts");
    const b = resolveCmLanguage("y/two.ts");
    expect(b).toBe(a);
    // 命中不同 loader 表(文件名 vs 扩展名)的 key 相互隔离
    const d1 = resolveCmLanguage("Dockerfile");
    const d2 = resolveCmLanguage("sub/Dockerfile");
    expect(d2).toBe(d1);
  });
});
