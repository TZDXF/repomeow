import type { AiApiType, AiModelDef, AiProvider } from "@/lib/ai-config";

/** 三态兼容开关:auto = 跟随 provider/baseUrl 自动探测(不写入配置) */
export type CompatTriState = "auto" | "on" | "off";
export type CompatMaxTokensField = "auto" | "max_completion_tokens" | "max_tokens";

/** 模型高级配置草稿:仅暴露自建网关常见的四个兼容开关 */
export interface ModelCompatDraft {
  supportsDeveloperRole: CompatTriState;
  supportsReasoningEffort: CompatTriState;
  supportsStore: CompatTriState;
  maxTokensField: CompatMaxTokensField;
}

export interface ModelDraft {
  key: string;
  id: string;
  name: string;
  api: AiApiType | "";
  contextWindow: string;
  maxTokens: string;
  compat: ModelCompatDraft;
  /** 建控件之外的原始定义(cost/input 等透传保存,避免 UI 字段丢元数据) */
  source: AiModelDef;
}

export interface ProviderDraft {
  key: string;
  id: string;
  name: string;
  api: AiApiType;
  baseUrl: string;
  apiKey: string;
  /** UI 未暴露字段(headers 等)保存时透传 */
  source?: AiProvider;
  models: ModelDraft[];
}

let localKey = 0;
export const nextDraftKey = () => `local-${localKey++}`;

export function toTriState(value: boolean | undefined): CompatTriState {
  return value === undefined ? "auto" : value ? "on" : "off";
}

export function draftModel(model: AiModelDef): ModelDraft {
  return {
    key: nextDraftKey(),
    id: model.id,
    name: model.name,
    api: model.api ?? "",
    contextWindow: model.contextWindow > 0 ? String(model.contextWindow) : "",
    maxTokens: model.maxTokens > 0 ? String(model.maxTokens) : "",
    compat: {
      supportsDeveloperRole: toTriState(model.compat?.supportsDeveloperRole),
      supportsReasoningEffort: toTriState(model.compat?.supportsReasoningEffort),
      supportsStore: toTriState(model.compat?.supportsStore),
      maxTokensField: model.compat?.maxTokensField ?? "auto",
    },
    source: model,
  };
}
