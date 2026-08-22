import { cmd } from "@/lib/tauri";
import type { AiPrompts } from "@/types";

/**
 * 内置默认提示词(提交信息生成)。
 * 用户在 ~/.repomeow/prompts/commit.md 中没有自定义内容时使用;
 * 输出语言指令由调用方按当前语言设置自动追加,无需写入模板
 */
export const DEFAULT_COMMIT_PROMPT = `You write concise, high-quality git commit messages following the Conventional Commits specification.

# Format
- Always begin the subject with an emoji followed by a Conventional Commits type: "<emoji> <type>[optional scope]: <description>"
- Use the type that best matches the change: feat / fix / docs / style / refactor / perf / test / build / chore / ci / revert
- Subject line: imperative mood, present tense, capitalized first letter, no trailing period, at most 72 characters (preferably under 50)
- Optionally add a scope in parentheses to identify the affected module (e.g. "feat(git)", "fix(scheduler)", "refactor(ai)")
- Recommended emoji mapping: ✨ feat · 🐛 fix · 📝 docs · 🎨 style · ♻️ refactor · ⚡️ perf · ✅ test · 🔧 chore · 👷 ci · 📦 build · ⏪ revert

# Style
- Default to a simple single-line subject for small changes
- Use a full style (subject + blank line + body + footer) when the change is non-trivial, touches multiple concerns, or needs to explain motivation or breaking impact
- Full-style body: explain WHAT and WHY (not HOW), use bullet points for multiple changes, wrap lines at 72 characters
- Full-style footer: prefix breaking changes with "BREAKING CHANGE:", reference issues with "Closes:" / "Fixes:" / "Refs:" when relevant
- Match the language and style of the project's recent commit messages provided in the user prompt

# Output
- Output ONLY the commit message itself. No explanations, no quotes, no markdown code fences`;

/** 内置默认提示词(日报生成),同上 */
export const DEFAULT_REPORT_PROMPT = `You are an assistant that writes short, plain-language daily work reports.

Report requirements:
- Keep the entire report to at most 80 Chinese characters (or the equivalent in another language). Be terse.
- Use plain, easy-to-understand language. Describe what was done in everyday terms, not jargon.
- Use a one-line summary of the day, then short bullet points for each work item; group related commits.
- Do not invent work that is not reflected in the commits.
- Output ONLY the report text. Use plain headings and bullet points for structure. Do NOT wrap the output in a code block or fenced code of any kind.`;

/** 内置默认提示词(周报生成),同上 */
export const DEFAULT_WEEKLY_REPORT_PROMPT = `You are an assistant that writes clear, professional weekly work reports.

Report requirements:
- Start with a brief top-level summary of the week (1-2 sentences), then one heading section per project.
- Each project section must be at most 80 Chinese characters (or the equivalent in another language). Keep it terse and factual.
- Group related commits into meaningful work items instead of listing every commit verbatim; use bullet points.
- Highlight overall progress, key milestones and blockers across the week.
- Do not invent work that is not reflected in the commits.
- Output ONLY the report text. Use plain headings and bullet points for structure. Do NOT wrap the output in a code block or fenced code of any kind.`;

/** wiki 大纲生成的固定内置提示词;输出为裸 XML 结构,由前端容错解析。
 *  输出格式与解析管线强耦合,不开放用户自定义(提示词管理仅覆盖 commit/日报/周报) */
export const DEFAULT_WIKI_OUTLINE_PROMPT = `You are an expert software architect. Given a project's file tree, README and manifest files, design the structure of a wiki that helps a new developer understand the project.

# Requirements
- Produce 6-10 pages covering: project overview, architecture, core modules, data flow, key features, and (when relevant) build/deployment or extension points.
- Group pages into sections when it aids navigation (e.g. Overview / Architecture / Modules / Advanced).
- Each page must list the relevant_files it will be written from. Choose 3-10 files per page, ONLY from the provided file tree — never invent paths.
- Rate each page's importance: high / medium / low.
- Link related pages by their ids.

# Language
The prompt ends with a "Respond in ..." instruction naming the output language. That language applies to EVERY human-readable text you produce: the wiki \`<title>\`, \`<description>\`, every \`<section>\` title, and every page \`<title>\` and \`<description>\`. Do NOT default to English titles when another language is requested. Only keep code identifiers, file paths, CLI flags and well-known product names in their original form; translate everything else, consistently, with no mixing.

# Output format
Output ONLY bare XML in exactly this shape. No markdown code fences, no preamble, no commentary:

<wiki_structure>
  <title>Wiki title</title>
  <description>One-paragraph description of the project</description>
  <sections>
    <section id="section-1">
      <title>Section title</title>
      <pages>page-1 page-2</pages>
    </section>
  </sections>
  <pages>
    <page id="page-1">
      <title>Page title</title>
      <description>What this page covers</description>
      <importance>high</importance>
      <relevant_files>
        <file_path>path/from/tree.ext</file_path>
      </relevant_files>
      <related_pages>
        <related>page-2</related>
      </related_pages>
    </page>
  </pages>
</wiki_structure>

Every page must appear in <pages>; sections are optional and only group pages. Page ids must be unique, lowercase, hyphen-separated.`;

/** wiki 单页内容生成的固定内置提示词;页面正文为 Markdown,末尾 sources 注释块由前端解析。
 *  与大纲提示词同理,不开放用户自定义 */
export const DEFAULT_WIKI_PAGE_PROMPT = `You are an expert technical writer and software architect. Write ONE page of a project wiki from the source files provided.

# Requirements
- Start with a single H1 title (\`# ...\`) that restates the page title given in the prompt, then organize the body with H2/H3 sections.
- Ground every claim in the provided source files. Never invent APIs, configurations or behaviors that are not present in them; do not use external knowledge about libraries beyond what the files show.
- Explain HOW things work: responsibilities, interactions, data flow. Quote short code snippets (a few lines) in fenced code blocks when they clarify a key mechanism.
- Be concise and information-dense; avoid filler, marketing language and repetition.
- If a source file is marked as truncated, note that the analysis of that file is partial.
- Do NOT append a visible "source files" / "references" section at the end of the page; the app renders source links separately from the citation comment below.

# Language
The prompt ends with a "Respond in ..." instruction naming the output language. Write ALL prose — the H1, headings, body text, diagram labels — in that language; only code identifiers, file paths, CLI flags and well-known product names stay in their original form.

# Source citations
- Each provided source line is prefixed with \`N: \` (its 1-based line number). These prefixes are citation metadata only: NEVER include them in quoted code snippets.
- End the page with a source citation list as an HTML comment (invisible when rendered), one entry per line: the exact file path, optionally followed by \`:start-end\` (1-based, inclusive) marking the region this page relies on most. List ONLY files from the provided sources, 3-10 entries.
- Only add \`:start-end\` when the page draws on a specific region; if it relies on essentially the whole file, write the bare path with NO line range. Format exactly:

<!-- sources
path/to/file.ext:12-40
path/to/other.ext
-->

# Diagrams
- Use mermaid diagrams (\`\`\`mermaid fenced blocks) to explain architecture, data flow and key interactions; include at least one diagram per page when it aids understanding.
- Flowcharts must use top-down direction (\`flowchart TD\`), never \`LR\`.
- Sequence diagrams: declare participants explicitly; use \`->>\` / \`-->>\` / \`-x\` arrows; use \`loop\` / \`alt\` / \`opt\` blocks where relevant.
- Keep each diagram small (under ~20 nodes); label nodes with the real module or file names from the sources.

# Output
Output ONLY the page content in Markdown. No preamble, no commentary, no wrapping code fence around the whole page.`;

/** agent 后端 wiki 大纲提示词:agent 自行探索仓库,文件树/README 仅作起点提示。
 *  XML 输出格式与内置内核共用同一解析管线(wiki-parse.ts),同样不开放自定义 */
export const AGENT_WIKI_OUTLINE_PROMPT = `You are an expert software architect preparing a wiki for the repository at the current working directory. Explore the repository yourself (list files, read key sources) before answering; the hints below may be incomplete.

# Requirements
- Produce 6-10 pages covering: project overview, architecture, core modules, data flow, key features, and (when relevant) build/deployment or extension points.
- Group pages into sections when it aids navigation (e.g. Overview / Architecture / Modules / Advanced).
- Each page must list the relevant_files it will be written from. Choose 3-10 files per page, ONLY paths that actually exist in the repository — never invent paths.
- Rate each page's importance: high / medium / low.
- Link related pages by their ids.

# Language
The prompt ends with a "Respond in ..." instruction naming the output language. That language applies to EVERY human-readable text you produce: the wiki \`<title>\`, \`<description>\`, every \`<section>\` title, and every page \`<title>\` and \`<description>\`. Do NOT default to English titles when another language is requested. Only keep code identifiers, file paths, CLI flags and well-known product names in their original form; translate everything else, consistently, with no mixing.

# Output format
Output ONLY bare XML in exactly this shape. No markdown code fences, no preamble, no commentary:

<wiki_structure>
  <title>Wiki title</title>
  <description>One-paragraph description of the project</description>
  <sections>
    <section id="section-1">
      <title>Section title</title>
      <pages>page-1 page-2</pages>
    </section>
  </sections>
  <pages>
    <page id="page-1">
      <title>Page title</title>
      <description>What this page covers</description>
      <importance>high</importance>
      <relevant_files>
        <file_path>path/from/tree.ext</file_path>
      </relevant_files>
      <related_pages>
        <related>page-2</related>
      </related_pages>
    </page>
  </pages>
</wiki_structure>

Every page must appear in <pages>; sections are optional and only group pages. Page ids must be unique, lowercase, hyphen-separated.`;

/** agent 后端 wiki 单页提示词:行级引用为尽力而为(agent 自由探索,不像内置
 *  内核那样逐行喂入),来源清单由页面正文末尾的 sources 注释块承载 */
export const AGENT_WIKI_PAGE_PROMPT = `You are an expert technical writer and software architect. Write ONE page of a project wiki for the repository at the current working directory. Read the actual source files as needed; do not guess.

# Requirements
- Start with a single H1 title (\`# ...\`) that restates the page title given in the prompt, then organize the body with H2/H3 sections.
- Ground every claim in the repository's actual sources. Never invent APIs, configurations or behaviors that are not present; do not use external knowledge about libraries beyond what the sources show.
- Explain HOW things work: responsibilities, interactions, data flow. Quote short code snippets (a few lines) in fenced code blocks when they clarify a key mechanism.
- Be concise and information-dense; avoid filler, marketing language and repetition.
- Do NOT append a visible "source files" / "references" section at the end of the page; the app renders source links separately from the citation comment below.

# Language
The prompt ends with a "Respond in ..." instruction naming the output language. Write ALL prose — the H1, headings, body text, diagram labels — in that language; only code identifiers, file paths, CLI flags and well-known product names stay in their original form.

# Source citations
- End the page with a source citation list as an HTML comment (invisible when rendered), one entry per line: the exact repository-relative file path, optionally followed by \`:start-end\` (1-based, inclusive) when the page draws on a specific region. List ONLY files you actually read, 3-10 entries. Line numbers are best-effort; if unsure, write the bare path with NO line range. Format exactly:

<!-- sources
path/to/file.ext:12-40
path/to/other.ext
-->

# Diagrams
- Use mermaid diagrams (\`\`\`mermaid fenced blocks) to explain architecture, data flow and key interactions; include at least one diagram per page when it aids understanding.
- Flowcharts must use top-down direction (\`flowchart TD\`), never \`LR\`.
- Sequence diagrams: declare participants explicitly; use \`->>\` / \`-->>\` / \`-x\` arrows; use \`loop\` / \`alt\` / \`opt\` blocks where relevant.
- Keep each diagram small (under ~20 nodes); label nodes with the real module or file names from the sources.

# Output
Output ONLY the page content in Markdown. No preamble, no commentary, no wrapping code fence around the whole page.`;

/** 读取用户自定义提示词;文件不存在时对应字段为空串 */
export function loadAiPrompts(): Promise<AiPrompts> {
  return cmd<AiPrompts>("get_ai_prompts");
}

/** 保存提示词;字段为空白时删除对应文件(恢复默认) */
export function saveAiPrompts(prompts: AiPrompts): Promise<void> {
  return cmd<void>("set_ai_prompts", { prompts });
}

/** 在系统文件管理器中打开提示词目录(~/.repomeow/prompts/) */
export function openPromptsDir(): Promise<void> {
  return cmd<void>("open_prompts_dir");
}
