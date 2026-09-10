import { Channel } from "@tauri-apps/api/core";
import type { SupportedLocale } from "@/i18n";
import type { AiApiType } from "@/lib/ai-config";
import { cmd } from "@/lib/tauri";
import type { GitCommitInfo, ReportPeriodType } from "@/types";

/** 一个项目在给定时间范围内的提交记录(日报输入) */
export interface ProjectCommits {
  projectId?: number;
  projectName: string;
  projectDescription: string;
  commits: GitCommitInfo[];
}

export interface GeneratedReport {
  historyId: number;
  result: string;
  commitData: ProjectCommits[];
}

export interface ReportGenerationInput {
  projectIds: number[];
  dateFrom: string;
  dateTo: string;
  rangeLabel: string;
  authorMode: "me" | "all";
  language: SupportedLocale;
  periodType: ReportPeriodType;
}

export interface BatchReportBridgeItem {
  dateFrom: string;
  dateTo: string;
  label: string;
  status: string;
  error?: string;
}

export interface BatchReportStatusEvent {
  dateFrom: string;
  dateTo: string;
  status: string;
  error?: string;
}

function runId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function bindCancellation(id: string, signal?: AbortSignal): () => void {
  if (!signal) return () => {};
  const cancel = () => void cmd<void>("ai_cancel_run", { runId: id }).catch(() => {});
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  return () => signal.removeEventListener("abort", cancel);
}

/** 提交上下文、提示词、模型请求和用量统计均由后端完成。取消后返回 null。 */
export async function generateCommitMessage(
  project: { path: string; name: string; description: string },
  language: SupportedLocale,
  includeUntracked: boolean,
  paths?: string[] | null,
  signal?: AbortSignal,
): Promise<string | null> {
  const id = runId("commit");
  const unbind = bindCancellation(id, signal);
  try {
    return await cmd<string | null>("ai_generate_commit_message", {
      request: {
        projectPath: project.path,
        projectName: project.name,
        projectDescription: project.description,
        language,
        runId: id,
        includeUntracked,
        paths: paths ?? null,
      },
    });
  } finally {
    unbind();
  }
}

/** 内置 agent 生成 AGENTS.md 的返回;claudeAction 为 CLAUDE.md 对齐结果。 */
export interface GenerateAgentsMdResult {
  claudeAction: "created" | "aligned" | "unchanged";
}

/** 内置 agent(读写权限,写仅 AGENTS.md)生成/重新生成 AGENTS.md 并对齐 CLAUDE.md 引用。
 *  options 可指定模型(复合值 "providerId/modelId")、思考强度(空 = 设置页默认 /
 *  模型默认档)与取消信号。取消后返回 null。 */
export async function generateAgentsMd(
  project: { path: string; name: string },
  language: SupportedLocale,
  options?: { model?: string; thinking?: string; signal?: AbortSignal },
): Promise<GenerateAgentsMdResult | null> {
  const id = runId("agents-md");
  const unbind = bindCancellation(id, options?.signal);
  try {
    return await cmd<GenerateAgentsMdResult | null>("ai_generate_agents_md", {
      request: {
        projectPath: project.path,
        language,
        runId: id,
        model: options?.model ?? null,
        thinking: options?.thinking ?? null,
      },
    });
  } finally {
    unbind();
  }
}
/** Git 提交收集、AI 生成与历史保存作为一个后端操作完成。 */
export async function generateAndSaveReport(
  input: ReportGenerationInput,
  signal?: AbortSignal,
): Promise<GeneratedReport | null> {
  const id = runId("report");
  const unbind = bindCancellation(id, signal);
  try {
    return await cmd<GeneratedReport | null>("ai_generate_and_save_report", {
      request: { runId: id, ...input },
    });
  } finally {
    unbind();
  }
}

/** 批量报告只把后端状态事件转交给 Pinia；并发、取消、生成和保存都在 Rust。 */
export async function generateBatchReports(
  input: {
    items: BatchReportBridgeItem[];
    projectIds: number[];
    authorMode: "me" | "all";
    language: SupportedLocale;
    periodType: ReportPeriodType;
    concurrency: number;
  },
  signal: AbortSignal,
  onEvent: (event: BatchReportStatusEvent) => void,
): Promise<void> {
  const id = runId("batch-report");
  const unbind = bindCancellation(id, signal);
  const channel = new Channel<BatchReportStatusEvent>();
  channel.onmessage = onEvent;
  try {
    await cmd<void>("ai_generate_batch_reports", {
      request: { runId: id, ...input },
      onEvent: channel,
    });
  } finally {
    unbind();
  }
}

/** 使用后端 SDK 的 Models API，允许设置页用尚未保存的表单配置探测。 */
export function fetchAiModels(baseURL: string, apiKey: string, api: AiApiType): Promise<string[]> {
  return cmd<string[]>("ai_list_models", {
    config: { aiBaseUrl: baseURL, aiApiKey: apiKey, aiModel: "", api },
  });
}

/** 使用当前已保存配置发送最小测试请求；测试连接不计入用量。 */
export function testAiConnection(): Promise<void> {
  return cmd<void>("ai_test_connection");
}
