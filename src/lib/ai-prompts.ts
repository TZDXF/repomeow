import { cmd } from "@/lib/tauri";
import type { AiPrompts } from "@/types";

/** 读取用户自定义提示词；空字符串表示生成时使用后端内置模板。 */
export function loadAiPrompts(): Promise<AiPrompts> {
  return cmd<AiPrompts>("get_ai_prompts");
}

/** 获取后端内置模板，仅用于设置页占位预览。 */
export function loadDefaultAiPrompts(): Promise<AiPrompts> {
  return cmd<AiPrompts>("get_default_ai_prompts");
}

export function saveAiPrompts(prompts: AiPrompts): Promise<void> {
  return cmd<void>("set_ai_prompts", { prompts });
}

export function openPromptsDir(): Promise<void> {
  return cmd<void>("open_prompts_dir");
}
