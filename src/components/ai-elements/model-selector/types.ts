import type { AiModelDef, AiModelRef } from "@/lib/ai-config";

/** 模型选择器的分组选项(按厂商分组展示) */
export interface ModelSelectorGroup {
  providerId: string;
  providerName: string;
  models: AiModelDef[];
}

/** 选择器复合值:"providerId/modelId"(model id 自身可含 "/",取首个 / 分隔) */
export function modelOptionValue(providerId: string, modelId: string): string {
  return `${providerId}/${modelId}`;
}

/** 复合值 → 模型引用;格式非法返回 null */
export function parseModelOptionValue(value: string): AiModelRef | null {
  const separator = value.indexOf("/");
  if (separator <= 0 || separator === value.length - 1) return null;
  return { providerId: value.slice(0, separator), modelId: value.slice(separator + 1) };
}

/** 展示名:未填 name 时回退 id */
export function modelDisplayName(model: AiModelDef): string {
  return model.name || model.id;
}
