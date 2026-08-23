import { Channel } from "@tauri-apps/api/core";
import { cmd } from "@/lib/tauri";
import type { SupportedLocale } from "@/i18n";
import type { WikiOutlinePage } from "@/types";

/**
 * Wiki 后端任务的类型与 Tauri Channel 桥。前端只维护可渲染状态，文件收集、
 * 生成后端选择、ACP 会话、重试、并发和落盘均由 Rust 完成。
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
  onPhase: (phase: WikiGenPhase) => void;
  onPage: (page: WikiOutlinePage, status: WikiPageStatus, error?: string) => void;
  /** 页面流式生成的增量内容(partial 为当前已生成的全部正文,非增量片段) */
  onPageProgress?: (page: WikiOutlinePage, partial: string) => void;
  onContext?: (context: WikiContextSummary) => void;
  onActivities?: (activities: WikiGenerationActivity[]) => void;
  onRetry?: (retry: WikiRetryStatus) => void;
}

export interface WikiContextSummary {
  fileCount: number;
  treeTruncated: boolean;
  hasReadme: boolean;
  manifestCount: number;
}

export type WikiGenerationActivityType = "scan" | "read" | "tool";

export interface WikiGenerationActivity {
  type: WikiGenerationActivityType;
  text: string;
}

export interface WikiRetryStatus {
  pageId?: string;
  attempt: number;
  maxAttempts: number;
  delaySeconds: number;
  reason: "rateLimited" | "temporary";
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

/** 单个项目独立保存于 Wiki 目录 config.json 的生成配置。 */
export interface WikiGenerationConfig {
  version: number;
  backend: WikiGenBackend;
}

export interface WikiGenOptions {
  language: SupportedLocale;
  /** 并发生成的页数(复用设置的 aiConcurrency;agent 内核固定为单会话顺序,忽略此值) */
  concurrency: number;
}

/** 页面生成的附加上下文(增量更新时传入变更文件清单,帮助内核聚焦) */
export interface WikiPageHints {
  changedFiles?: string[];
}

type WikiGenerationEvent =
  | { kind: "phase"; phase: WikiGenPhase }
  | { kind: "page"; page: WikiOutlinePage; status: WikiPageStatus; error?: string }
  | { kind: "progress"; pageId: string; content: string }
  | ({ kind: "retry" } & WikiRetryStatus)
  | ({ kind: "context" } & WikiContextSummary)
  | { kind: "activityBatch"; activityType: WikiGenerationActivityType; items: string[] };

/** 整本生成的前端桥：仅把 Rust Channel 事件映射为既有 UI 回调。 */
export async function generateWiki(
  project: { path: string; name: string },
  options: WikiGenOptions,
  signal: AbortSignal,
  callbacks: WikiGenCallbacks,
): Promise<void> {
  const { onPhase, onPage, onPageProgress, onContext, onActivities, onRetry } = callbacks;
  const id = `wiki-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const pages = new Map<string, WikiOutlinePage>();
  const channel = new Channel<WikiGenerationEvent>();
  channel.onmessage = (event) => {
    if (event.kind === "phase") {
      onPhase(event.phase);
    } else if (event.kind === "page") {
      pages.set(event.page.id, event.page);
      onPage(event.page, event.status, event.error);
    } else if (event.kind === "progress") {
      const page = pages.get(event.pageId);
      if (page) {
        onPageProgress?.(page, event.content);
      }
    } else if (event.kind === "retry") {
      onRetry?.(event);
    } else if (event.kind === "context") {
      onContext?.(event);
    } else {
      onActivities?.(event.items.map((text) => ({ type: event.activityType, text })));
    }
  };
  const cancel = () => void cmd<void>("ai_cancel_run", { runId: id }).catch(() => {});
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  try {
    await cmd<void>("ai_generate_wiki", {
      request: {
        runId: id,
        projectPath: project.path,
        projectName: project.name,
        language: options.language,
        concurrency: options.concurrency,
      },
      onEvent: channel,
    });
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

export interface WikiPageGenerationResult {
  model: string;
  generator: string;
}

export interface WikiUpdateProgress {
  completed: number;
  total: number;
}

/** 单页/增量生成桥；ACP 会话、重试与落盘均在后端。 */
export async function regenerateWikiPage(
  project: { path: string; name: string },
  page: WikiOutlinePage,
  language: SupportedLocale,
  signal: AbortSignal,
  hints?: WikiPageHints,
): Promise<WikiPageGenerationResult> {
  const id = `wiki-page-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const channel = new Channel<string>();
  channel.onmessage = () => {};
  const cancel = () => void cmd<void>("ai_cancel_run", { runId: id }).catch(() => {});
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  try {
    return await cmd<WikiPageGenerationResult>("ai_regenerate_wiki_page", {
      request: {
        runId: id,
        projectPath: project.path,
        language,
        page,
        changedFiles: hints?.changedFiles ?? [],
      },
      onProgress: channel,
    });
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

/** 增量更新桥；变更检测、页面筛选、生成顺序、落盘与 meta 推进均由后端完成。 */
export async function updateWiki(
  project: { path: string; name: string },
  options: WikiGenOptions,
  automatic: boolean,
  onProgress: (progress: WikiUpdateProgress) => void,
): Promise<number> {
  const id = `wiki-update-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const channel = new Channel<WikiUpdateProgress>();
  channel.onmessage = onProgress;
  return cmd<number>("ai_update_wiki", {
    request: {
      runId: id,
      projectPath: project.path,
      language: options.language,
      automatic,
    },
    onEvent: channel,
  });
}
