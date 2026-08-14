import { StreamLanguage, type LanguageSupport } from "@codemirror/language";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// CodeMirror 6 的语言解析映射(替代原 Shiki 的 LANG_BY_EXT/LANG_BY_NAME):
// - 主流语言用官方 lezer 包(精确解析),每语言一个惰性 import() 才进 chunk;
// - 其余走 @codemirror/legacy-modes 的 StreamParser(识别稍弱但覆盖面广);
// - vue 近似为 HTML(SFC 模板可读,script 内高亮退化为 HTML 容错),
//   less 近似为 CSS,makefile/.gitignore/bat 等无对应语法回退 null(纯文本)。
// 加载结果按 key(命中的文件名或扩展名)缓存 Promise,重复打开同语言零开销。

type CmLanguage = LanguageSupport | StreamLanguage<unknown>;
type LangLoader = () => Promise<CmLanguage>;

const EXT_LOADERS: Record<string, LangLoader> = {
  // 官方 lezer 语法
  ts: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true })),
  mts: () =>
    import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true })),
  cts: () =>
    import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true })),
  tsx: () =>
    import("@codemirror/lang-javascript").then((m) => m.javascript({ typescript: true, jsx: true })),
  js: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  mjs: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  cjs: () => import("@codemirror/lang-javascript").then((m) => m.javascript()),
  jsx: () => import("@codemirror/lang-javascript").then((m) => m.javascript({ jsx: true })),
  json: () => import("@codemirror/lang-json").then((m) => m.json()),
  jsonc: () => import("@codemirror/lang-json").then((m) => m.json()),
  html: () => import("@codemirror/lang-html").then((m) => m.html()),
  htm: () => import("@codemirror/lang-html").then((m) => m.html()),
  vue: () => import("@codemirror/lang-html").then((m) => m.html()),
  css: () => import("@codemirror/lang-css").then((m) => m.css()),
  less: () => import("@codemirror/lang-css").then((m) => m.css()),
  md: () => import("@codemirror/lang-markdown").then((m) => m.markdown()),
  markdown: () => import("@codemirror/lang-markdown").then((m) => m.markdown()),
  py: () => import("@codemirror/lang-python").then((m) => m.python()),
  rs: () => import("@codemirror/lang-rust").then((m) => m.rust()),
  c: () => import("@codemirror/lang-cpp").then((m) => m.cpp()),
  h: () => import("@codemirror/lang-cpp").then((m) => m.cpp()),
  cpp: () => import("@codemirror/lang-cpp").then((m) => m.cpp()),
  hpp: () => import("@codemirror/lang-cpp").then((m) => m.cpp()),
  sql: () => import("@codemirror/lang-sql").then((m) => m.sql()),
  xml: () => import("@codemirror/lang-xml").then((m) => m.xml()),
  svg: () => import("@codemirror/lang-xml").then((m) => m.xml()),
  php: () => import("@codemirror/lang-php").then((m) => m.php()),
  // legacy StreamParser
  yml: () => import("@codemirror/legacy-modes/mode/yaml").then((m) => StreamLanguage.define(m.yaml)),
  yaml: () =>
    import("@codemirror/legacy-modes/mode/yaml").then((m) => StreamLanguage.define(m.yaml)),
  toml: () =>
    import("@codemirror/legacy-modes/mode/toml").then((m) => StreamLanguage.define(m.toml)),
  ini: () =>
    import("@codemirror/legacy-modes/mode/properties").then((m) =>
      StreamLanguage.define(m.properties),
    ),
  sh: () =>
    import("@codemirror/legacy-modes/mode/shell").then((m) => StreamLanguage.define(m.shell)),
  bash: () =>
    import("@codemirror/legacy-modes/mode/shell").then((m) => StreamLanguage.define(m.shell)),
  zsh: () =>
    import("@codemirror/legacy-modes/mode/shell").then((m) => StreamLanguage.define(m.shell)),
  ps1: () =>
    import("@codemirror/legacy-modes/mode/powershell").then((m) =>
      StreamLanguage.define(m.powerShell),
    ),
  go: () => import("@codemirror/legacy-modes/mode/go").then((m) => StreamLanguage.define(m.go)),
  rb: () =>
    import("@codemirror/legacy-modes/mode/ruby").then((m) => StreamLanguage.define(m.ruby)),
  lua: () =>
    import("@codemirror/legacy-modes/mode/lua").then((m) => StreamLanguage.define(m.lua)),
  swift: () =>
    import("@codemirror/legacy-modes/mode/swift").then((m) => StreamLanguage.define(m.swift)),
  scss: () =>
    import("@codemirror/legacy-modes/mode/sass").then((m) => StreamLanguage.define(m.sass)),
  java: () =>
    import("@codemirror/legacy-modes/mode/clike").then((m) => StreamLanguage.define(m.java)),
  kt: () =>
    import("@codemirror/legacy-modes/mode/clike").then((m) => StreamLanguage.define(m.kotlin)),
  cs: () =>
    import("@codemirror/legacy-modes/mode/clike").then((m) => StreamLanguage.define(m.csharp)),
};

// 按完整文件名匹配(小写),优先于扩展名
const NAME_LOADERS: Record<string, LangLoader> = {
  dockerfile: () =>
    import("@codemirror/legacy-modes/mode/dockerfile").then((m) =>
      StreamLanguage.define(m.dockerFile),
    ),
  ".env": () =>
    import("@codemirror/legacy-modes/mode/properties").then((m) =>
      StreamLanguage.define(m.properties),
    ),
};

const cache = new Map<string, Promise<CmLanguage>>();

/** 解析仓库内文件路径对应的 CM 语言扩展;无对应语法(含回退)返回 null */
export function resolveCmLanguage(path: string): Promise<CmLanguage | null> {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1) : "";
  const loader = NAME_LOADERS[name] ?? EXT_LOADERS[ext];
  if (!loader) return Promise.resolve(null);
  const key = name in NAME_LOADERS ? `n:${name}` : `e:${ext}`;
  let cached = cache.get(key);
  if (!cached) {
    cached = loader();
    cache.set(key, cached);
  }
  return cached;
}
