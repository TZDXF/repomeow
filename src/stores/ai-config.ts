import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  emptyChatPrefs,
  getAiConfig,
  isChatThinkingLevel,
  saveAiConfig,
  type AiConfigFile,
  type AiModelDef,
  type AiModelRef,
  type AiProvider,
  type ChatPermission,
  type ChatThinkingLevel,
} from "@/lib/ai-config";

/** 厂商 + 模型的已解析视图(引用与两侧对象都有效) */
export interface ResolvedAiModel {
  reference: AiModelRef;
  provider: AiProvider;
  model: AiModelDef;
}

/**
 * AI 接入配置 store:厂商列表 + 默认模型 + 问答偏好。
 * 配置的唯一事实源是 ~/.repomeow/ai-config.json(Rust 侧每次调用热读),
 * 这里持有内存副本;setter 改副本后全量落盘,失败时回读并抛出。
 */
export const useAiConfigStore = defineStore("ai-config", () => {
  const config = ref<AiConfigFile | null>(null);
  const loaded = ref(false);

  /** 读取配置(已加载时复用内存副本;force 重新拉取) */
  async function ensureLoaded(force = false): Promise<AiConfigFile> {
    if (config.value && !force) return config.value;
    return reload();
  }

  async function reload(): Promise<AiConfigFile> {
    config.value = await getAiConfig();
    loaded.value = true;
    return config.value;
  }

  async function save(next: AiConfigFile): Promise<void> {
    try {
      await saveAiConfig(next);
      config.value = next;
    } catch (error) {
      // 落盘失败:回读后端真实状态,避免内存副本与文件漂移
      await reload().catch(() => {});
      throw error;
    }
  }

  // ── 引用解析(对齐 Rust resolve_chat_prefs 的回退语义) ──────────────

  function resolveRef(c: AiConfigFile, reference: AiModelRef | null): ResolvedAiModel | null {
    if (!reference) return null;
    const provider = c.providers[reference.providerId];
    const model = provider?.models.find((m) => m.id === reference.modelId);
    if (!provider || !model) return null;
    return { reference, provider, model };
  }

  /** 问答当前生效的模型(chat 引用失效时回退 defaultModel) */
  const chatModel = computed<ResolvedAiModel | null>(() => {
    const c = config.value;
    if (!c) return null;
    const chatRefValid =
      c.chat.providerId != null &&
      c.chat.modelId != null &&
      Boolean(c.providers[c.chat.providerId]?.models.some((m) => m.id === c.chat.modelId));
    const reference: AiModelRef | null = chatRefValid
      ? { providerId: c.chat.providerId as string, modelId: c.chat.modelId as string }
      : c.defaultModel;
    return resolveRef(c, reference);
  });

  /** 默认模型(commit/报告/Wiki/测试连接使用) */
  const defaultModel = computed<ResolvedAiModel | null>(() => {
    const c = config.value;
    return c ? resolveRef(c, c.defaultModel) : null;
  });

  /** 问答面板可用性:模型有效且厂商 baseUrl/apiKey 齐备 */
  const chatReady = computed(() =>
    Boolean(chatModel.value?.provider.baseUrl && chatModel.value?.provider.apiKey),
  );

  /** 默认模型可用性(报告等场景的前置校验) */
  const defaultReady = computed(() =>
    Boolean(defaultModel.value?.provider.baseUrl && defaultModel.value?.provider.apiKey),
  );

  /** chat 模型的选择器复合值 "providerId/modelId" */
  const chatModelValue = computed(() =>
    chatModel.value
      ? `${chatModel.value.reference.providerId}/${chatModel.value.reference.modelId}`
      : "",
  );

  /** 当前问答思考强度(chat 偏好;非法值回退 off) */
  const chatThinking = computed<ChatThinkingLevel>(() => {
    const value = config.value?.chat.thinking;
    return isChatThinkingLevel(value) ? value : "off";
  });

  /** 当前问答工具权限 */
  const chatPermission = computed<ChatPermission>(() => config.value?.chat.permission ?? "all");

  async function updateChat(patch: {
    providerId?: string | null;
    modelId?: string | null;
    thinking?: ChatThinkingLevel;
    permission?: ChatPermission;
  }): Promise<void> {
    const current = await ensureLoaded();
    const chat = { ...emptyChatPrefs(), ...current.chat, ...patch };
    await save({ ...current, chat });
  }

  /** 切换问答模型(选择器复合值 "providerId/modelId" → 拆分引用) */
  async function setChatModelValue(value: string): Promise<void> {
    const separator = value.indexOf("/");
    if (separator <= 0 || separator === value.length - 1) return;
    await updateChat({
      providerId: value.slice(0, separator),
      modelId: value.slice(separator + 1),
    });
  }

  async function setChatThinking(level: ChatThinkingLevel): Promise<void> {
    if (!isChatThinkingLevel(level)) return;
    await updateChat({ thinking: level });
  }

  async function setChatPermission(permission: ChatPermission): Promise<void> {
    await updateChat({ permission });
  }

  return {
    config,
    loaded,
    ensureLoaded,
    reload,
    save,
    chatModel,
    defaultModel,
    chatReady,
    defaultReady,
    chatModelValue,
    chatThinking,
    chatPermission,
    updateChat,
    setChatModelValue,
    setChatThinking,
    setChatPermission,
  };
});
