import { Channel } from "@tauri-apps/api/core";
import { cmd } from "@/lib/tauri";

/**
 * wiki agent 后端的前端桥:经 ACP 调用本地 coding agent CLI。
 * Rust 侧(commands/agent.rs)负责进程与协议;这里只做类型化封装。
 */

/** 精选 agent 清单条目(agent_list 返回) */
export interface AgentInfo {
  id: string;
  name: string;
  /** 分发方式:npx 包(需 Node)/ 原生二进制 */
  kind: "npx" | "binary";
  installed: boolean;
  /** 探测到的可执行路径(npx 类为 npx 路径);未安装为 null */
  detail: string | null;
  /** 未安装/未登录时的指引文案 */
  loginHint: string;
}

/** 会话流式事件:chunk 为累积正文(activity 为工具调用/权限决策行) */
export type AcpEvent = { kind: "chunk"; text: string } | { kind: "activity"; text: string };

/** 会话配置下拉的一个可选项(session/new 上报) */
export interface AcpConfigChoice {
  id: string;
  name: string;
}

/**
 * agent 上报的会话配置选项(select 类):模型/思考强度等下拉。
 * category 语义取值:"model" / "thought_level" / "mode" / "model_config" / 其他原样。
 */
export interface AcpConfigOptionInfo {
  id: string;
  name: string;
  category: string | null;
  /** 当前选中值 id */
  current: string | null;
  /** 下拉可选项(后端已把分组拍平) */
  choices: AcpConfigChoice[];
}

/** 旧式 mode(无 config_options 的 agent 用它选模型档位) */
export interface AcpModeInfo {
  id: string;
  name: string;
}

export interface AcpStartResult {
  runId: string;
  agentName: string;
  configOptions: AcpConfigOptionInfo[];
  modes: AcpModeInfo[];
}

export interface AcpTestResult {
  agentName: string;
  configOptions: AcpConfigOptionInfo[];
  modes: AcpModeInfo[];
}

export interface AcpPromptResult {
  /** "endTurn" 为正常完成;maxTokens/maxTurnRequests/refusal 等也会返回(附已累计文本) */
  stopReason: string;
  text: string;
  /** 本次 prompt 的 token 用量(ACP unstable 字段;agent 未上报为 null) */
  usage?: AcpPromptUsage | null;
}

/** ACP 单次 prompt 的 token 用量 */
export interface AcpPromptUsage {
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  thoughtTokens?: number;
  cachedReadTokens?: number;
  cachedWriteTokens?: number;
}

/** 精选 agent 清单 + 安装探测 */
export function agentList(): Promise<AgentInfo[]> {
  return cmd<AgentInfo[]>("agent_list");
}

/**
 * 启动会话:agentId 与 customCommand 二选一;cwd 即 agent 的工作目录。
 * model/thinking 为用户选择的模型/思考强度 id(来自 acpTest 上报的选项列表),
 * 后端在建会话后经 set_config_option/set_mode 应用,不在列表内则忽略。
 */
export function acpStart(opts: {
  agentId?: string;
  customCommand?: string;
  cwd: string;
  model?: string;
  thinking?: string;
}): Promise<AcpStartResult> {
  return cmd<AcpStartResult>("acp_start", {
    ...(opts.agentId ? { agentId: opts.agentId } : {}),
    ...(opts.customCommand ? { customCommand: opts.customCommand } : {}),
    cwd: opts.cwd,
    ...(opts.model ? { model: opts.model } : {}),
    ...(opts.thinking ? { thinking: opts.thinking } : {}),
  });
}

/** 发送一次 prompt:onEvent 流式回调(chunk 为累积正文),完成返回最终全文 */
export function acpPrompt(
  runId: string,
  prompt: string,
  onEvent: (event: AcpEvent) => void,
): Promise<AcpPromptResult> {
  const channel = new Channel<AcpEvent>();
  channel.onmessage = onEvent;
  return cmd<AcpPromptResult>("acp_prompt", { runId, prompt, onEvent: channel });
}

/** 取消会话(发 session/cancel,宽限后由后端杀进程树) */
export function acpCancel(runId: string): Promise<void> {
  return cmd<void>("acp_cancel", { runId });
}

/**
 * 自动获取模型清单:spawn + 握手 + 建临时会话,返回 agent 名称
 * 与其上报的 configOptions/modes(下拉数据源),随即收尾进程。
 */
export function acpTest(opts: {
  agentId?: string;
  customCommand?: string;
}): Promise<AcpTestResult> {
  return cmd<AcpTestResult>("acp_test", {
    ...(opts.agentId ? { agentId: opts.agentId } : {}),
    ...(opts.customCommand ? { customCommand: opts.customCommand } : {}),
  });
}

/** 应用会话内的 acpTest 结果缓存:value 为进行中/已完成的 Promise(并发去重;失败不缓存) */
const testCache = new Map<string, Promise<AcpTestResult>>();

/**
 * 带缓存的 acpTest:同一 agent/命令在应用会话内只真实探测一次——生成配置对话框
 * 每次打开都会自动拉取模型清单,避免反复 spawn agent 进程。force = 忽略缓存重测。
 */
export function acpTestCached(
  key: string,
  opts: { agentId?: string; customCommand?: string },
  force = false,
): Promise<AcpTestResult> {
  if (!force) {
    const hit = testCache.get(key);
    if (hit) return hit;
  }
  const pending = acpTest(opts).catch((e) => {
    testCache.delete(key);
    throw e;
  });
  testCache.set(key, pending);
  return pending;
}
