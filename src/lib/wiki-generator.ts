import { generateWikiOutline, streamWikiPage } from "@/lib/ai";
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
 * 流水线:collect(文件树+README+清单) → outline(LLM 产 XML 大纲,容错解析)
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

export interface WikiGenOptions {
  language: SupportedLocale;
  /** 并发生成的页数(复用设置的 aiConcurrency) */
  concurrency: number;
  /** 记录进 meta 的模型名 */
  model: string;
}

/** 单页生成失败重试次数(与 deepwiki-open 默认一致) */
const PAGE_RETRIES = 2;
/** 大纲解析失败的重试次数(重新请求 LLM 生成大纲) */
const OUTLINE_RETRIES = 1;

/** 生成大纲:LLM 输出解析失败时重试 OUTLINE_RETRIES 次,仍失败抛错 */
async function generateOutline(
  context: WikiContext,
  projectName: string,
  language: SupportedLocale,
  signal: AbortSignal,
): Promise<WikiOutlinePage[]> {
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
}

/** 生成单个页面并落盘(流式);失败重试 PAGE_RETRIES 次后抛错 */
async function generateOnePage(
  projectPath: string,
  page: WikiOutlinePage,
  language: SupportedLocale,
  signal: AbortSignal,
  onProgress?: (partial: string) => void,
): Promise<void> {
  const files = await readWikiFiles(projectPath, page.relevantFiles);
  let lastError: unknown = null;
  for (let attempt = 0; attempt <= PAGE_RETRIES; attempt++) {
    if (signal.aborted) return;
    try {
      onProgress?.(""); // 重试时清空上一次的半截流式内容
      // eslint-disable-next-line no-await-in-loop
      const content = await streamWikiPage(page, files, language, signal, (partial) =>
        onProgress?.(partial),
      );
      if (signal.aborted) return;
      // eslint-disable-next-line no-await-in-loop
      await saveWikiPage(projectPath, page.file, content);
      return;
    } catch (e) {
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
    if (signal.aborted) throw new DOMException("aborted", "AbortError");

    onPhase("outlining");
    const pages = await generateOutline(context, project.name, options.language, signal);
    if (signal.aborted) throw new DOMException("aborted", "AbortError");

    await beginWiki(project.path);
    for (const page of pages) onPage(page, "pending");

    onPhase("generating");
    // 已终态(done/failed)的页;取消时只对未完成的页补发 cancelled
    const finished = new Set<string>();
    const tasks = pages.map((page) => async () => {
      onPage(page, "running");
      try {
        await generateOnePage(project.path, page, options.language, signal, (partial) =>
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
    await runPool(options.concurrency, tasks, signal);

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
      model: options.model,
      language: options.language,
      status: "completed",
      outline: pages,
    });
    onPhase("done");
  } catch (e) {
    if (signal.aborted || (e instanceof DOMException && e.name === "AbortError")) {
      onPhase("cancelled");
      return;
    }
    onPhase("failed");
    throw e;
  }
}

/** 重新生成单个页面(不触碰大纲与其他页面;调用方负责后续刷新) */
export async function regenerateWikiPage(
  projectPath: string,
  page: WikiOutlinePage,
  language: SupportedLocale,
  signal: AbortSignal,
): Promise<void> {
  await generateOnePage(projectPath, page, language, signal);
}
