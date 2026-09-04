import { cmd } from "@/lib/tauri";

/**
 * AI 接入配置(~/.repomeow/ai-config.json)前后端桥。
 * 多厂商 + 模型元数据(上下文窗口/思考/费率)来自 pi 的 models.json 格式;
 * 前端一律经命令读写,不自算路径。Rust 侧每次调用重读文件,保存即生效。
 */

/** 已实现的 AI wire API；开放字符串保留外部配置的未知值。 */
export const AI_API_TYPES = [
  "openai-completions",
  "openai-responses",
  "anthropic-messages",
  "google-generative-ai",
] as const;
export type KnownAiApiType = (typeof AI_API_TYPES)[number];
export type AiApiType = KnownAiApiType | (string & {});

export function isKnownAiApiType(value: string): value is KnownAiApiType {
  return (AI_API_TYPES as readonly string[]).includes(value);
}

/** 问答工具权限档位:all = 全部工具;ask = 写操作前询问确认 */
export type ChatPermission = "all" | "ask";

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

/**
 * OpenAI 兼容端点的模型级兼容开关(对齐 pi 的 OpenAICompletionsCompat 子集)。
 * 字段缺省 = 按 provider/baseUrl 自动探测;自建网关报错时在设置页模型
 * 高级配置里显式覆盖。UI 未暴露的字段(thinkingFormat 等)原样透传。
 */
export interface AiModelCompat {
  /** 端点是否接受 store 字段 */
  supportsStore?: boolean;
  /** reasoning 模型的系统提示词是否可用 developer 角色(否则回退 system) */
  supportsDeveloperRole?: boolean;
  /** 端点是否接受 reasoning_effort 思考强度参数 */
  supportsReasoningEffort?: boolean;
  /** 流式响应是否携带 usage */
  supportsUsageInStreaming?: boolean;
  /** 端点是否返回 finish_reason */
  supportsFinishReason?: boolean;
  /** 令牌上限字段名:max_completion_tokens(OpenAI 新式)/ max_tokens(旧式) */
  maxTokensField?: "max_completion_tokens" | "max_tokens";
  /** 工具参数是否可附加 strict 标记 */
  supportsStrictMode?: boolean;
  /** Responses / Anthropic 是否支持长时缓存。 */
  supportsLongCacheRetention?: boolean;
  /** Responses 是否接受 max_output_tokens。 */
  supportsMaxOutputTokens?: boolean;
  /** Anthropic 是否接受 eager_input_streaming。 */
  supportsEagerToolInputStreaming?: boolean;
  /** Anthropic 是否发送会话亲和头。 */
  sendSessionAffinityHeaders?: boolean;
  /** Anthropic 是否允许在工具定义上放 cache_control。 */
  supportsCacheControlOnTools?: boolean;
  /** Anthropic 是否接受 temperature。 */
  supportsTemperature?: boolean;
  /** Anthropic 是否强制 adaptive thinking。 */
  forceAdaptiveThinking?: boolean;
  /** Anthropic 是否回放空 thinking signature。 */
  allowEmptySignature?: boolean;
  /** Anthropic 是否支持 strict tools。 */
  supportsStrictTools?: boolean;
}

/** 厂商下的模型定义(元数据支撑上下文占用显示、思考参数与成本计算) */
export interface AiModelDef {
  id: string;
  /** 为空时继承厂商 API。 */
  api?: AiApiType;
  /** 为空时继承厂商 Base URL。 */
  baseUrl?: string;
  /** 模型级 header 覆盖厂商 header。 */
  headers?: Record<string, string>;
  /** OpenAI 系 adapter 的附加采样参数。 */
  samplingParams?: Record<string, unknown>;
  /** 为空时展示与请求按 id 兜底 */
  name: string;
  reasoning: boolean;
  input: ("text" | "image")[];
  contextWindow: number;
  maxTokens: number;
  cost?: AiModelCost;
  compat?: AiModelCompat;
}

/** 一个厂商；api 决定默认 wire adapter。 */
export interface AiProvider {
  name: string;
  baseUrl: string;
  apiKey: string;
  api: AiApiType;
  /** 请求默认 header，可被模型级覆盖。 */
  headers?: Record<string, string>;
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

/** 内置厂商目录(添加厂商对话框的候选清单;含各厂商预置模型,apiKey 恒为空) */
export function getBuiltinAiProviders(): Promise<Record<string, AiProvider>> {
  return cmd<Record<string, AiProvider>>("ai_config_builtin_providers");
}

/** 一个可导入的 CC Switch 供应商(api 已筛选为四种受支持 wire adapter 之一) */
export interface CcSwitchProvider {
  /** CC Switch 内的供应商 id(导入时去重后作为厂商 id 候选) */
  id: string;
  name: string;
  /** 来源应用:claude / claude-desktop / codex / gemini / opencode / openclaw / pi / hermes / grokbuild */
  app: string;
  baseUrl: string;
  /** 可能为空(如密钥走环境变量),导入后需用户补齐 */
  apiKey: string;
  api: AiApiType;
  models: AiModelDef[];
  /** 在 CC Switch 中是否为该应用当前启用项 */
  current: boolean;
}

/** CC Switch 扫描结果;found = false 表示本机未安装/未配置过 CC Switch */
export interface CcSwitchScan {
  found: boolean;
  providers: CcSwitchProvider[];
}

/** 扫描本机 CC Switch(~/.cc-switch)中可导入的供应商 */
export function listCcSwitchProviders(): Promise<CcSwitchScan> {
  return cmd<CcSwitchScan>("ai_cc_switch_providers");
}
