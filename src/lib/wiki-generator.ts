import { generateWikiOutline, languageName, streamWikiPage } from "@/lib/ai";
import { mapAcpPromptUsage, recordAiUsage } from "@/lib/ai-usage";
import { AGENT_WIKI_OUTLINE_PROMPT, AGENT_WIKI_PAGE_PROMPT } from "@/lib/ai-prompts";
import { acpCancel, acpPrompt, acpStart } from "@/lib/agent";
import { runPool } from "@/lib/async-pool";
import { parseWikiOutline } from "@/lib/wiki-parse";
import {
  beginWiki,
  collectWikiContext,
  readWikiFiles,
  saveWikiMeta,
  saveWikiPage,
} from "@/lib/wiki";
import type { SupportedLocale } from "@/i18n";
import type { WikiContext, WikiOutlinePage } from "@/types";

/**
 * wiki 生成编排(参照 batch-report 的模式:执行层不持有响应式状态,
 * 阶段/页面状态变更统一经回调上报,由调用方 store 写回)。
 *
 * 生成内核可插拔(WikiGenKernel):「内置」走 OpenAI 兼容 API(lib/ai.ts,
 * 前端直接调 LLM,页级并发可控),「agent」走本地 coding agent CLI(lib/agent.ts,
 * ACP 会话,单会话顺序生成)。两种内核产出相同的数据契约——大纲页列表与页 Markdown
 * 正文——落盘/状态机/UI 层完全共用。
 *
 * 流水线:collect(文件树+README+清单) → outline(内核产 XML 大纲,容错解析)
 * → begin(清空旧 wiki) → 逐页生成(并发池,单页重试 PAGE_RETRIES 次)
 * → 写 meta.json(取消时不写,整本视为无效)。
 */

export type WikiGenPhase =
  | "collecting"
  | "outlining"
  | "generating"
  | "done"
  | "failed"
  | "cancelled";

export type WikiPageStatus = "pending" | "running" | "done" | "failed" | "cancelled";

export interface WikiGenCallbacks {
  onPhase(phase: WikiGenPhase): void;
  onPage(page: WikiOutlinePage, status: WikiPageStatus, error?: string): void;
  /** 页面流式生成的增量内容(partial 为当前已生成的全部正文,非增量片段) */
  onPageProgress?(page: WikiOutlinePage, partial: string): void;
}

/** 生成后端选择:内置 API 或本地 agent(经 ACP 会话) */
export type WikiGenBackend =
  | { kind: "builtin" }
  | {
      kind: "agent";
      agentId?: string;
      customCommand?: string;
      /** 模型/思考强度 id(设置页从 agent 上报的选项列表选择;空 = agent 默认) */
      model?: string;
      thinking?: string;
    };

export interface WikiGenOptions {
  language: SupportedLocale;
  /** 并发生成的页数(复用设置的 aiConcurrency;agent 内核固定为单会话顺序,忽略此值) */
  concurrency: number;
  /** 记录进 meta 的模型名(仅内置内核使用;agent 内核以 agent 名称覆写) */
  model: string;
  backend: WikiGenBackend;
}

/** 页面生成的附加上下文(增量更新时传入变更文件清单,帮助内核聚焦) */
export interface WikiPageHints {
  changedFiles?: string[];
}

/**
 * 生成内核:产出「大纲页列表」与「页 Markdown 正文」两个纯数据契约,
 * 落盘与状态上报由编排层统一承担。
 */
export interface WikiGenKernel {
  /** 后端标识("builtin" / "acp:<agentId>" / "acp:custom"),写进 meta.generator */
  backendId: string;
  /** 页面生成并发(builtin = aiConcurrency;agent = 1,单会话顺序) */
  concurrency: number;
  /** 记录进 meta 的模型名(builtin = 配置模型;agent = agent 名称·所选模型) */
  model: string;
  /** 生成大纲(内核内部负责解析失败后的重试) */
  generateOutline(
    context: WikiContext,
    projectName: string,
    language: SupportedLocale,
    signal: AbortSignal,
  ): Promise<WikiOutlinePage[]>;
  /** 生成单个页面正文(单次尝试;重试由编排层统一处理),onPartial 为累积全文 */
  generatePage(
    projectPath: string,
    page: WikiOutlinePage,
    language: SupportedLocale,
    signal: AbortSignal,
    onPartial?: (partial: string) => void,
    hints?: WikiPageHints,
  ): Promise<string>;
  /** 收尾(终止 agent 会话并杀进程;builtin 无操作)。幂等,总是被调用 */
  dispose(): Promise<void>;
}

/** 后端标识(meta.generator 的取值):跨后端增量更新时退化为整本重生成 */
export function backendIdOf(backend: WikiGenBackend): string {
  return backend.kind === "builtin" ? "builtin" : `acp:${backend.agentId ?? "custom"}`;
}

/** 单页生成失败重试次数(与 deepwiki-open 默认一致) */
const PAGE_RETRIES = 2;
/** 大纲解析失败的重试次数(重新请求生成大纲) */
const OUTLINE_RETRIES = 1;

// ── 内置内核(OpenAI 兼容 API,lib/ai.ts) ───────────────────────────────────

function createBuiltinKernel(options: WikiGenOptions): WikiGenKernel {
  return {
    backendId: "builtin",
    concurrency: options.concurrency,
    model: options.model,
    async generateOutline(context, projectName, language, signal) {
      const validFiles = new Set(context.paths);
      let lastError: unknown = null;
      for (let attempt = 0; attempt <= OUTLINE_RETRIES; attempt++) {
        if (signal.aborted) break;
        try {
          // 重试语义:必须串行等待上一次失败后再请求,不能并发
          // eslint-disable-next-line no-await-in-loop
          const raw = await generateWikiOutline(context, projectName, language, signal);
          return parseWikiOutline(raw, validFiles).pages;
        } catch (e) {
          lastError = e;
        }
      }
      throw lastError instanceof Error ? lastError : new Error(String(lastError));
    },
    async generatePage(projectPath, page, language, signal, onPartial) {
      const files = await readWikiFiles(projectPath, page.relevantFiles);
      const content = await streamWikiPage(page, files, language, signal, (partial) =>
        onPartial?.(partial),
      );
      return content;
    },
    async dispose() {},
  };
}

// ── agent 内核(本地 coding agent CLI,ACP 会话) ────────────────────────────

/** agent 大纲任务提示词:规则 + 探索指引 + 文件树/README 起点提示 */
function buildAgentOutlinePrompt(
  context: WikiContext,
  projectName: string,
  language: SupportedLocale,
): string {
  const manifestSection = context.manifests.length
    ? `\n\nManifest files:\n${context.manifests
        .map((m) => `=== ${m.path} ===\n${m.content}`)
        .join("\n\n")}`
    : "";
  const readmeSection = context.readme ? `\n\nREADME:\n${context.readme}` : "";
  const truncatedNote = context.treeTruncated
    ? "\n(Note: the file tree was truncated; directory entries like `dir/ (N files)` summarize folded subtrees.)"
    : "";
  return `${AGENT_WIKI_OUTLINE_PROMPT}

Respond in ${languageName(language)}.

Project: ${projectName}

Preliminary hints (may be incomplete — verify by exploring the repository yourself):
File tree (${context.fileCount} files):${truncatedNote}
${context.fileTree}${readmeSection}${manifestSection}`;
}

/** agent 单页任务提示词:规则 + 页面主题 + 相关文件建议(增量时附变更清单) */
function buildAgentPagePrompt(
  page: WikiOutlinePage,
  hints: WikiPageHints | undefined,
  language: SupportedLocale,
): string {
  const relevantSection = page.relevantFiles.length
    ? `\n\nSuggested source files (verify they still exist):\n${page.relevantFiles
        .map((f) => `- ${f}`)
        .join("\n")}`
    : "";
  const changedSection = hints?.changedFiles?.length
    ? `\n\nRecently changed files (this page is being refreshed after these changes):\n${hints.changedFiles
        .map((f) => `- ${f}`)
        .join("\n")}`
    : "";
  return `${AGENT_WIKI_PAGE_PROMPT}

Respond in ${languageName(language)}.

Wiki page: ${page.title}
Coverage: ${page.description}${relevantSection}${changedSection}`;
}

async function createAgentKernel(
  project: { path: string; name: string },
  options: WikiGenOptions,
): Promise<WikiGenKernel> {
  const backend = options.backend;
  if (backend.kind !== "agent") throw new Error("invalid backend");
  const { runId, agentName } = await acpStart({
    ...(backend.agentId ? { agentId: backend.agentId } : {}),
    ...(backend.customCommand ? { customCommand: backend.customCommand } : {}),
    cwd: project.path,
    ...(backend.model ? { model: backend.model } : {}),
    ...(backend.thinking ? { thinking: backend.thinking } : {}),
  });
  // 用量统计与 meta.model 同款「agent 名称 · 所选模型」措辞
  const usageModel = backend.model ? `${agentName} · ${backend.model}` : agentName;
  /** 单次 prompt:invoke 本身不可中止,取消靠 abort 事件触发后端 session/cancel(+超时杀进程) */
  async function promptOnce(
    prompt: string,
    signal: AbortSignal,
    onChunk?: (text: string) => void,
  ): Promise<string> {
    const onAbort = () => {
      void acpCancel(runId).catch(() => {});
    };
    signal.addEventListener("abort", onAbort, { once: true });
    const startedAt = Date.now();
    try {
      const result = await acpPrompt(runId, prompt, (event) => {
        if (event.kind === "chunk") onChunk?.(event.text);
      });
      // PromptResponse.usage 即本次 prompt 消耗;agent 未上报时跳过
      if (result.usage) {
        recordAiUsage({
          taskType: "wiki",
          model: usageModel,
          usage: mapAcpPromptUsage(result.usage),
          durationMs: Date.now() - startedAt,
        });
      }
      return result.text;
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
  }

  return {
    backendId: backendIdOf(backend),
    concurrency: 1,
    // meta.model 记「agent 名称 · 所选模型」,未选模型则仅 agent 名称
    model: backend.model ? `${agentName} · ${backend.model}` : agentName,
    async generateOutline(context, projectName, language, signal) {
      const validFiles = new Set(context.paths);
      let lastError: unknown = null;
      for (let attempt = 0; attempt <= OUTLINE_RETRIES; attempt++) {
        if (signal.aborted) break;
        try {
          // eslint-disable-next-line no-await-in-loop
          const raw = await promptOnce(
            buildAgentOutlinePrompt(context, projectName, language),
            signal,
          );
          return parseWikiOutline(raw, validFiles).pages;
        } catch (e) {
          if (signal.aborted) break;
          lastError = e;
        }
      }
      throw lastError instanceof Error ? lastError : new Error(String(lastError));
    },
    async generatePage(_projectPath, page, language, signal, onPartial, hints) {
      return promptOnce(buildAgentPagePrompt(page, hints, language), signal, (text) =>
        onPartial?.(text),
      );
    },
    async dispose() {
      await acpCancel(runId).catch(() => {});
    },
  };
}

/** 按选项创建生成内核;agent 内核会 spawn agent 进程并完成 ACP 握手(可能耗时) */
export async function createWikiKernel(
  project: { path: string; name: string },
  options: WikiGenOptions,
): Promise<WikiGenKernel> {
  return options.backend.kind === "builtin"
    ? createBuiltinKernel(options)
    : createAgentKernel(project, options);
}

/** 生成单个页面并落盘;失败重试 PAGE_RETRIES 次后抛错(重试间清空半截流式内容) */
async function generateOnePage(
  kernel: WikiGenKernel,
  projectPath: string,
  page: WikiOutlinePage,
  language: SupportedLocale,
  signal: AbortSignal,
  onProgress?: (partial: string) => void,
  hints?: WikiPageHints,
): Promise<void> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt <= PAGE_RETRIES; attempt++) {
    if (signal.aborted) return;
    try {
      onProgress?.(""); // 重试时清空上一次的半截流式内容
      // eslint-disable-next-line no-await-in-loop
      const content = await kernel.generatePage(
        projectPath,
        page,
        language,
        signal,
        (partial) => onProgress?.(partial),
        hints,
      );
      if (signal.aborted) return;
      // eslint-disable-next-line no-await-in-loop
      await saveWikiPage(projectPath, page.file, content);
      return;
    } catch (e) {
      if (signal.aborted) return;
      lastError = e;
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

/**
 * 整本生成 wiki。单页失败不影响其他页面;signal 中止时停止派发新页,
 * 不写 meta.json(旧 wiki 已被 begin_wiki 清空,整本失效,等待下次生成)。
 * 全部结束(含部分页面失败)后写 meta 并回调 done。
 */
export async function generateWiki(
  project: { path: string; name: string },
  options: WikiGenOptions,
  signal: AbortSignal,
  callbacks: WikiGenCallbacks,
): Promise<void> {
  const { onPhase, onPage, onPageProgress } = callbacks;
  try {
    onPhase("collecting");
    const context = await collectWikiContext(project.path);
    // agent 内核的 spawn/握手(首跑含 npx 下载适配器)也计入收集阶段
    const kernel = await createWikiKernel(project, options);
    try {
      if (signal.aborted) throw new DOMException("aborted", "AbortError");

      onPhase("outlining");
      const pages = await kernel.generateOutline(context, project.name, options.language, signal);
      if (signal.aborted) throw new DOMException("aborted", "AbortError");

      await beginWiki(project.path);
      for (const page of pages) onPage(page, "pending");

      onPhase("generating");
      // 已终态(done/failed)的页;取消时只对未完成的页补发 cancelled
      const finished = new Set<string>();
      const tasks = pages.map((page) => async () => {
        onPage(page, "running");
        try {
          await generateOnePage(kernel, project.path, page, options.language, signal, (partial) =>
            onPageProgress?.(page, partial),
          );
          finished.add(page.id);
          onPage(page, signal.aborted ? "cancelled" : "done");
        } catch (e) {
          if (signal.aborted) {
            onPage(page, "cancelled");
            return;
          }
          finished.add(page.id);
          onPage(page, "failed", e instanceof Error ? e.message : String(e));
        }
      });
      await runPool(kernel.concurrency, tasks, signal);

      if (signal.aborted) {
        for (const page of pages) {
          if (!finished.has(page.id)) onPage(page, "cancelled");
        }
        onPhase("cancelled");
        return;
      }

      await saveWikiMeta(project.path, {
        version: 0, // 后端覆写
        projectPath: "", // 后端回填
        generatedAt: "", // 后端覆写
        headSha: context.headSha,
        model: kernel.model,
        language: options.language,
        status: "completed",
        outline: pages,
        generator: kernel.backendId,
      });
      onPhase("done");
    } finally {
      await kernel.dispose().catch(() => {});
    }
  } catch (e) {
    if (signal.aborted || (e instanceof DOMException && e.name === "AbortError")) {
      onPhase("cancelled");
      return;
    }
    onPhase("failed");
    throw e;
  }
}

/** 重新生成单个页面(不触碰大纲与其他页面;调用方负责后续刷新与内核生命周期) */
export async function regenerateWikiPage(
  kernel: WikiGenKernel,
  projectPath: string,
  page: WikiOutlinePage,
  language: SupportedLocale,
  signal: AbortSignal,
  hints?: WikiPageHints,
): Promise<void> {
  await generateOnePage(kernel, projectPath, page, language, signal, undefined, hints);
}
