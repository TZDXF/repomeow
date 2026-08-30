import { cmd } from "@/lib/tauri";

/**
 * AI 接入配置(~/.repomeow/ai-config.json)前后端桥。
 * 多厂商 + 模型元数据(上下文窗口/思考/费率)来自 pi 的 models.json 格式;
 * 前端一律经命令读写,不自算路径。Rust 侧每次调用重读文件,保存即生效。
 */

/** AI API 类型;当前仅实现 OpenAI 兼容接口,其余值预留扩展 */
export type AiApiType = "openai-completions";

/** 问答工具权限档位:all = 全部工具;readOnly = 仅只读工具 */
export type ChatPermission = "all" | "readOnly";

/** 思考强度:off 关闭,其余对齐 pi 的 ThinkingLevel */
export type ChatThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";

export const CHAT_THINKING_LEVELS: ChatThinkingLevel[] = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

export function isChatThinkingLevel(value: unknown): value is ChatThinkingLevel {
  return typeof value === "string" && CHAT_THINKING_LEVELS.includes(value as ChatThinkingLevel);
}

/** 模型费率($/million tokens) */
export interface AiModelCost {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** 厂商下的模型定义(元数据支撑上下文占用显示、思考参数与成本计算) */
export interface AiModelDef {
  id: string;
  /** 为空时展示与请求按 id 兜底 */
  name: string;
  reasoning: boolean;
  input: ("text" | "image")[];
  contextWindow: number;
  maxTokens: number;
  cost?: AiModelCost;
}

/** 一个厂商(OpenAI 兼容端点) */
export interface AiProvider {
  name: string;
  baseUrl: string;
  apiKey: string;
  api: AiApiType;
  models: AiModelDef[];
}

/** 模型引用:厂商 id + 模型 id */
export interface AiModelRef {
  providerId: string;
  modelId: string;
}

/** 问答面板的全局偏好 */
export interface ChatPrefs {
  providerId: string | null;
  modelId: string | null;
  thinking: ChatThinkingLevel;
  permission: ChatPermission;
}

/** 顶层配置文件 */
export interface AiConfigFile {
  version: number;
  providers: Record<string, AiProvider>;
  /** commit/报告/Wiki/测试连接使用的默认模型 */
  defaultModel: AiModelRef | null;
  chat: ChatPrefs;
}

/** 创建一份带默认值的空 chat 偏好(配置损坏回退用) */
export function emptyChatPrefs(): ChatPrefs {
  return { providerId: null, modelId: null, thinking: "off", permission: "all" };
}

/** 读取配置;后端文件缺失时自动播种(含旧 settings.json 三键迁移) */
export function getAiConfig(): Promise<AiConfigFile> {
  return cmd<AiConfigFile>("ai_config_get");
}

/** 全量保存配置(原子写;后端保存前做引用归一化) */
export function saveAiConfig(config: AiConfigFile): Promise<void> {
  return cmd<void>("ai_config_save", { config });
}

/** 在系统文件管理器中打开配置文件所在目录 */
export function revealAiConfigDir(): Promise<void> {
  return cmd<void>("ai_config_reveal");
}
