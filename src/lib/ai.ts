import { generateText, streamText } from "ai";
import { createOpenAI } from "@ai-sdk/openai";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { i18n, type SupportedLocale } from "@/i18n";
import {
  DEFAULT_COMMIT_PROMPT,
  DEFAULT_REPORT_PROMPT,
  DEFAULT_WEEKLY_REPORT_PROMPT,
  DEFAULT_WIKI_OUTLINE_PROMPT,
  DEFAULT_WIKI_PAGE_PROMPT,
  loadAiPrompts,
} from "@/lib/ai-prompts";
import { useSettingsStore } from "@/stores/settings";
import type {
  GitCommitContext,
  GitCommitInfo,
  ReportPeriodType,
  WikiContext,
  WikiFileContent,
  WikiOutlinePage,
} from "@/types";

/** 一个项目在给定时间范围内的提交记录(日报输入) */
export interface ProjectCommits {
  projectName: string;
  /** 项目描述,帮助模型理解业务语境;可能为空串 */
  projectDescription: string;
  commits: GitCommitInfo[];
}

/** 读取 AI 配置;任意一项(接口地址 / API Key / 模型)缺失时抛出带本地化文案的错误 */
function requireConfig() {
  const settings = useSettingsStore();
  const baseURL = settings.aiBaseUrl.trim();
  const apiKey = settings.aiApiKey.trim();
  const model = settings.aiModel.trim();
  if (!baseURL || !apiKey || !model) {
    throw new Error(i18n.global.t("ai.notConfigured"));
  }
  return { baseURL, apiKey, model };
}

/**
 * 按服务商/模型名给出"关闭思考模式"的请求参数。
 * 只对已知支持该参数的提供方注入(Qwen/GLM/豆包/阶跃 Step 系),避免严格 OpenAI 兼容网关因未知字段 400
 */
function thinkingOffParams(baseURL: string, model: string): Record<string, unknown> {
  const s = `${baseURL} ${model}`.toLowerCase();
  const m = model.toLowerCase();
  if (s.includes("qwen") || s.includes("dashscope") || s.includes("aliyuncs")) {
    if (s.includes("dashscope") || s.includes("aliyuncs")) {
      // 阿里云百炼 / DashScope 兼容模式
      return { enable_thinking: false };
    }
    // 自建 vLLM/SGLang 部署的 Qwen3 系
    return { enable_thinking: false, chat_template_kwargs: { enable_thinking: false } };
  }
  if (
    s.includes("glm") ||
    s.includes("zhipu") ||
    s.includes("bigmodel") ||
    s.includes("doubao") ||
    s.includes("volces")
  ) {
    // 智谱 GLM / 火山方舟(豆包)系
    return { thinking: { type: "disabled" } };
  }
  if (m.startsWith("step-3") || m.startsWith("step-r")) {
    // 阶跃星辰 Step 推理系(Step 3.5/3.7 Flash 等):官方接口无完全关闭思考的开关,
    // 用最低推理档尽量缩短思考;思考内容经独立 reasoning 字段返回,不混入正文
    return { reasoning_effort: "low" };
  }
  return {};
}

/**
 * 剥离输出开头的 <think>...</think> 思考块(推理模型或中转服务可能把思考过程混入正文)。
 * 只处理响应起始位置的思考块:正文中出现的 <think> 字样(如报告介绍该功能本身)必须保留
 */
function stripThinking(text: string): string {
  let out = text.trimStart();
  while (/^<think>/i.test(out)) {
    const close = out.search(/<\/think>/i);
    if (close === -1) return ""; // 未闭合的思考块:整段都是思考,没有正文
    out = out.slice(close + "</think>".length).trimStart();
  }
  return out.trim();
}

/**
 * 构造 OpenAI Chat Completions 兼容模型。
 * 显式使用 .chat()(而非默认的 Responses API),兼容 DeepSeek/Moonshot/各类中转服务;
 * fetch 走 Tauri HTTP 插件(Rust 侧发请求),规避 webview 的 CORS 限制
 *
 * 是否思考由应用按场景决定,无用户设置:
 * - false(默认;commit 信息/报告/测试连接) → 命中已知推理模型提供方时向请求体注入关闭思考的参数,
 *   避免思考带来的延迟副作用;
 * - true(仅 wiki 大纲与页面生成) → 不注入任何参数,模型按默认行为决定是否输出 <think> 块;
 *   stripThinking 兜底剥除仍然生效(只在响应起始位置的思考块会被清理,
 *   完整闭合后中途再出现的保留原文)。
 */
function getChatModel(thinkingEnabled = false) {
  const { baseURL, apiKey, model } = requireConfig();
  const baseFetch = tauriFetch as unknown as typeof globalThis.fetch;
  // 启用思考时直接跳过参数注入,模型按默认行为输出
  const noThink = thinkingEnabled ? {} : thinkingOffParams(baseURL, model);
  // 命中已知推理模型提供方时,包装 fetch 向请求体注入关闭思考的参数
  const fetchFn =
    Object.keys(noThink).length === 0
      ? baseFetch
      : ((async (input: RequestInfo | URL, init?: RequestInit) => {
          if (init?.body && typeof init.body === "string") {
            try {
              const body = JSON.parse(init.body);
              Object.assign(body, noThink);
              init = { ...init, body: JSON.stringify(body) };
            } catch {
              // 非 JSON 请求体,原样透传
            }
          }
          return baseFetch(input, init);
        }) as typeof globalThis.fetch);
  const openai = createOpenAI({
    baseURL,
    apiKey,
    fetch: fetchFn,
  });
  return openai.chat(model);
}

function languageName(language: SupportedLocale) {
  return language === "zh-CN" ? "中文" : "English";
}

/** 组装固定 system prompt(内置模板);输出语言指令统一追加 */
function fixedSystemPrompt(fallback: string, language: SupportedLocale) {
  return `${fallback}\n\nRespond in ${languageName(language)}.`;
}

/** 组装 system prompt:用户自定义(~/.repomeow/prompts/*.md)优先,空则回退内置默认;输出语言指令统一追加 */
function buildSystemPrompt(custom: string, fallback: string, language: SupportedLocale) {
  return fixedSystemPrompt(custom.trim() || fallback, language);
}

/** 根据当前变更上下文生成 git 提交信息;user 提示词携带项目名称与描述帮助模型理解业务语境 */
export async function generateCommitMessage(
  ctx: GitCommitContext,
  project: { name: string; description: string },
  language: SupportedLocale,
): Promise<string> {
  const prompts = await loadAiPrompts();
  const description = project.description.trim();
  const projectSection = `Project: ${project.name}${description ? `\nDescription: ${description}` : ""}`;
  // 风格锚定:真实提交示例比 system 提示词的抽象规则更能让模型对齐仓库惯例
  const recentSection = ctx.recent_commits.length
    ? `\n\nRecent commit messages (match their style and language):\n${ctx.recent_commits
        .map((s) => `- ${s}`)
        .join("\n")}`
    : "";
  const truncatedNote = ctx.truncated ? "\n(Note: the diff was truncated due to length.)" : "";
  // 有内容的未跟踪文件单独成段给出全文;无内容的(二进制/超限)仅列文件名
  const withContent = new Set(ctx.untracked_files.map((f) => f.path));
  const namesOnly = ctx.untracked.filter((n) => !withContent.has(n));
  const untrackedNamesSection = namesOnly.length
    ? `\n\nUntracked new files (no diff content available):\n${namesOnly.join("\n")}`
    : "";
  const untrackedContentsSection = ctx.untracked_files.length
    ? `\n\nNew file contents (untracked):\n${ctx.untracked_files
        .map((f) => `=== ${f.path}${f.truncated ? " (truncated)" : ""} ===\n${f.content}`)
        .join("\n\n")}`
    : "";
  const { text } = await generateText({
    model: getChatModel(),
    system: buildSystemPrompt(prompts.commit, DEFAULT_COMMIT_PROMPT, language),
    prompt: `${projectSection}${recentSection}

Change summary (git diff --stat):
${ctx.stat || "(none)"}

Diff:${truncatedNote}
${ctx.diff || "(empty)"}${untrackedNamesSection}${untrackedContentsSection}`,
  });
  return stripThinking(text);
}

/** 汇总多个项目的提交记录,生成 Markdown 报告(日报/周报按 periodType 选择提示词);
 *  signal 用于批量生成的取消:中止进行中的 AI 请求 */
export async function generateReport(
  data: ProjectCommits[],
  rangeLabel: string,
  language: SupportedLocale,
  periodType: ReportPeriodType,
  signal?: AbortSignal,
): Promise<string> {
  const prompts = await loadAiPrompts();
  const sections = data
    .map((p) => {
      const lines = p.commits
        .map((c) => `- [${c.date}] ${c.subject} (${c.hash}, ${c.author})`)
        .join("\n");
      const description = p.projectDescription.trim();
      const heading = description ? `${p.projectName} — ${description}` : p.projectName;
      return `### ${heading}\n${lines || "(no commits)"}`;
    })
    .join("\n\n");
  const custom = periodType === "weekly" ? prompts.reportWeekly : prompts.report;
  const fallback = periodType === "weekly" ? DEFAULT_WEEKLY_REPORT_PROMPT : DEFAULT_REPORT_PROMPT;
  const { text } = await generateText({
    model: getChatModel(),
    system: buildSystemPrompt(custom, fallback, language),
    prompt: `Time range: ${rangeLabel}.

Commit records:
${sections}`,
    abortSignal: signal,
  });
  return stripThinking(text);
}

/**
 * 生成 wiki 大纲:输入过滤后的文件树 + README + 清单文件,输出裸 XML 结构文本
 * (由 wiki-parse.ts 容错解析);signal 用于取消生成;思考模式固定启用
 */
export async function generateWikiOutline(
  context: WikiContext,
  projectName: string,
  language: SupportedLocale,
  signal: AbortSignal | undefined,
): Promise<string> {
  const manifestSection = context.manifests.length
    ? `\n\nManifest files:\n${context.manifests
        .map((m) => `=== ${m.path} ===\n${m.content}`)
        .join("\n\n")}`
    : "";
  const readmeSection = context.readme ? `\n\nREADME:\n${context.readme}` : "";
  const truncatedNote = context.treeTruncated
    ? "\n(Note: the file tree was truncated; directory entries like `dir/ (N files)` summarize folded subtrees.)"
    : "";
  const { text } = await generateText({
    model: getChatModel(true),
    system: fixedSystemPrompt(DEFAULT_WIKI_OUTLINE_PROMPT, language),
    prompt: `Project: ${projectName}

File tree (${context.fileCount} files):${truncatedNote}
${context.fileTree}${readmeSection}${manifestSection}`,
    abortSignal: signal,
  });
  return stripThinking(text);
}

/** wiki 单页生成的 user prompt(流式/非流式共用) */
function buildWikiPageUserPrompt(page: WikiOutlinePage, files: WikiFileContent[]): string {
  const filesSection = files
    .map((f) => {
      // 行号前缀供末尾 sources 注释块标注 path:start-end 引用(提示词要求引用时剥除前缀)
      const numbered = f.content
        .split("\n")
        .map((line, i) => `${i + 1}: ${line}`)
        .join("\n");
      return `=== ${f.path}${f.truncated ? " (truncated)" : ""} ===\n${numbered}`;
    })
    .join("\n\n");
  return `Wiki page: ${page.title}
Coverage: ${page.description}

Source files:
${filesSection || "(no source files available)"}`;
}

/**
 * 生成 wiki 单个页面:输入大纲条目与其相关文件全文,输出 Markdown 正文;
 * signal 用于取消生成与重试中止;思考模式固定启用
 */
export async function generateWikiPage(
  page: WikiOutlinePage,
  files: WikiFileContent[],
  language: SupportedLocale,
  signal?: AbortSignal,
): Promise<string> {
  const { text } = await generateText({
    model: getChatModel(true),
    system: fixedSystemPrompt(DEFAULT_WIKI_PAGE_PROMPT, language),
    prompt: buildWikiPageUserPrompt(page, files),
    abortSignal: signal,
  });
  return stripThinking(text);
}

/**
 * 流式生成 wiki 页面:onChunk 收到逐步累积的正文(已剥离开头思考块;
 * 思考块未闭合期间回调为空串)。Tauri http 插件按 pull 逐块读响应体,支持流式;
 * 思考模式固定启用
 */
export async function streamWikiPage(
  page: WikiOutlinePage,
  files: WikiFileContent[],
  language: SupportedLocale,
  signal: AbortSignal | undefined,
  onChunk: (partial: string) => void,
): Promise<string> {
  const result = streamText({
    model: getChatModel(true),
    system: fixedSystemPrompt(DEFAULT_WIKI_PAGE_PROMPT, language),
    prompt: buildWikiPageUserPrompt(page, files),
    abortSignal: signal,
  });
  let acc = "";
  for await (const chunk of result.textStream) {
    acc += chunk;
    onChunk(stripThinking(acc));
  }
  return stripThinking(acc);
}

/**
 * 拉取 OpenAI 兼容接口的模型列表(GET {baseURL}/models),供设置页模型下拉使用。
 * 直接收表单值而非读 store,便于用户未保存时也能先拉取;走 Tauri HTTP 插件规避 CORS
 */
export async function fetchAiModels(baseURL: string, apiKey: string): Promise<string[]> {
  const url = `${baseURL.trim().replace(/\/+$/, "")}/models`;
  const res = await tauriFetch(url, {
    method: "GET",
    headers: { Authorization: `Bearer ${apiKey.trim()}` },
  });
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }
  const json = (await res.json()) as { data?: Array<{ id?: unknown }> };
  const ids = (json.data ?? [])
    .map((m) => (typeof m?.id === "string" ? m.id.trim() : ""))
    .filter(Boolean);
  return [...new Set(ids)].sort((a, b) => a.localeCompare(b));
}

/** 测试连接:发一条极短请求验证 baseURL / apiKey / model 可用 */
export async function testAiConnection(): Promise<void> {
  await generateText({
    model: getChatModel(),
    prompt: "Reply with the single word: ok",
    maxOutputTokens: 8,
  });
}
