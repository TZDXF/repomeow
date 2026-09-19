<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import { ExternalLink, Languages, RotateCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  listMarketplaceAudits,
  readMarketplaceAuditDetail,
  type ResourcePublicAudit,
  type ResourceSkillScanReport,
} from "@/lib/resource-library";
import { Markdown, type NodeRenderers } from "vue-stream-markdown";
import MdLink from "@/components/markdown/MdLink.vue";
import type { SupportedLocale } from "@/i18n";
import { hasScheme, safeLinkHref } from "@/lib/markdown";
import { formatAuditDate } from "@/lib/format";
import { cmd } from "@/lib/tauri";
import { getCachedTranslation, putCachedTranslation } from "@/lib/translation-cache";
import { useSettingsStore } from "@/stores/settings";

const props = withDefaults(
  defineProps<{
    marketplaceId: string | null;
    /** 本地安全扫描报告(父级扫描页传入);null 表示尚未扫描 */
    localScan?: ResourceSkillScanReport | null;
    scanning?: boolean;
  }>(),
  { localScan: null, scanning: false },
);
const { t, te, locale } = useI18n();
const language = computed(() => locale.value as SupportedLocale);
const nodeRenderers: NodeRenderers = { link: MdLink };
// 传游离元素,避免 Markdown 库把 island/pixel 主题变量写成无效 hsl(#…)(与技能预览一致)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
const settings = useSettingsStore();
const audits = ref<ResourcePublicAudit[]>([]);
const loading = ref(false);
const error = ref("");
/** 选中的审计来源:"local"=本地扫描(默认);其余为远程 provider */
const selection = ref<"local" | string>("local");
const selected = computed(
  () => audits.value.find((audit) => audit.provider === selection.value) ?? null,
);
const detailLoading = ref(false);
const detailError = ref("");
/** 远程审计详情缓存(provider 维度):切换来源不重复请求;刷新/切换技能时随 resetDetail 清空 */
const detailCache = new Map<string, { markdown: string; auditedAt: string }>();
const original = ref("");
const auditedAt = ref("");
const translated = ref("");
const showTranslation = ref(false);
const translating = ref(false);
let listSeq = 0;
let detailSeq = 0;
let translationSeq = 0;
let runId = "";
function cancelTranslation() {
  translationSeq++;
  if (runId) {
    void cmd("ai_cancel_run", { runId }).catch(() => {});
  }
  runId = "";
  translating.value = false;
}
function resetDetail() {
  detailSeq++;
  cancelTranslation();
  detailCache.clear();
  selection.value = "local";
  original.value = "";
  translated.value = "";
  showTranslation.value = false;
  detailError.value = "";
  detailLoading.value = false;
}
async function load() {
  const seq = ++listSeq;
  resetDetail();
  audits.value = [];
  error.value = "";
  loading.value = false;
  if (!props.marketplaceId) {
    return;
  }
  loading.value = true;
  try {
    const result = await listMarketplaceAudits(props.marketplaceId);
    if (seq !== listSeq) {
      return;
    }
    audits.value = result;
  } catch (e) {
    if (seq === listSeq) {
      error.value = String(e);
    }
  } finally {
    if (seq === listSeq) {
      loading.value = false;
    }
  }
}
function selectLocal() {
  detailSeq++;
  cancelTranslation();
  // 复位详情加载态:作废旧请求后其 finally 不再复位,不清理会卡死后续切换
  detailLoading.value = false;
  detailError.value = "";
  selection.value = "local";
}
async function selectAudit(audit: ResourcePublicAudit) {
  if (!props.marketplaceId) {
    return;
  }
  detailSeq++;
  cancelTranslation();
  selection.value = audit.provider;
  translated.value = "";
  showTranslation.value = false;
  detailError.value = "";
  // 已缓存的来源直接命中,切回不重复请求
  const cached = detailCache.get(audit.provider);
  if (cached) {
    original.value = cached.markdown;
    auditedAt.value = cached.auditedAt;
    detailLoading.value = false;
    return;
  }
  original.value = "";
  auditedAt.value = "";
  detailLoading.value = true;
  const seq = detailSeq;
  try {
    const detail = await readMarketplaceAuditDetail(props.marketplaceId, audit.provider);
    detailCache.set(audit.provider, {
      markdown: detail.markdown,
      auditedAt: detail.auditedAt ?? "",
    });
    if (seq === detailSeq) {
      original.value = detail.markdown;
      auditedAt.value = detail.auditedAt ?? "";
    }
  } catch (e) {
    if (seq === detailSeq) {
      detailError.value = String(e);
    }
  } finally {
    if (seq === detailSeq) {
      detailLoading.value = false;
    }
  }
}
async function translate() {
  if (translating.value || !original.value) {
    return;
  }
  if (translated.value) {
    showTranslation.value = !showTranslation.value;
    return;
  }
  const seq = ++translationSeq;
  const text = original.value;
  const language = settings.language;
  translating.value = true;
  try {
    const cached = await getCachedTranslation(text, language);
    if (seq !== translationSeq) {
      return;
    }
    runId = `audit-translation-${crypto.randomUUID()}`;
    const result =
      cached ??
      (await cmd<string | null>("ai_translate_markdown", { request: { text, language, runId } }));
    if (seq !== translationSeq || result === null) {
      return;
    }
    translated.value = result;
    showTranslation.value = true;
    if (cached === null) {
      void putCachedTranslation(text, language, result).catch(() => {});
    }
  } catch (e) {
    if (seq === translationSeq) {
      toast.error(String(e));
    }
  } finally {
    if (seq === translationSeq) {
      translating.value = false;
      runId = "";
    }
  }
}
/** 审计正文/译文里的链接:仅放行白名单协议,外链交系统浏览器,其余忽略 */
async function onMarkdownClick(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) {
    return;
  }
  const href = safeLinkHref(a.getAttribute("href"));
  e.preventDefault();
  if (href && hasScheme(href)) {
    await openUrl(href).catch(() => {});
  }
}
async function openOriginal(url: string) {
  try {
    await openUrl(url);
  } catch (e) {
    toast.error(String(e));
  }
}
function statusClass(status: string) {
  switch (status.toLowerCase()) {
    case "pass":
      return "text-emerald-600 dark:text-emerald-400";
    case "warn":
      return "text-amber-600 dark:text-amber-400";
    case "fail":
      return "text-destructive";
    default:
      return "text-muted-foreground";
  }
}
/** 本地扫描来源的状态摘要:扫描中 / 等级+评分 / 未扫描 */
const localStatus = computed(() => {
  if (props.scanning) {
    return t("settings.resources.skills.previewPage.scan.running");
  }
  const report = props.localScan;
  if (!report) {
    return t("settings.resources.skills.previewPage.scan.idle");
  }
  const key = `settings.resources.skills.previewPage.scan.level.${report.level}`;
  const label = te(key) ? t(key) : report.level;
  return `${label} ${report.score}`;
});
const localStatusClass = computed(() => {
  if (props.scanning) {
    return "text-primary";
  }
  switch (props.localScan?.level) {
    case "low":
      return "text-emerald-600 dark:text-emerald-400";
    case "medium":
      return "text-amber-600 dark:text-amber-400";
    case "high":
      return "text-orange-600 dark:text-orange-400";
    case "critical":
      return "text-red-600 dark:text-red-400";
    default:
      return "text-muted-foreground";
  }
});
watch(() => props.marketplaceId, load, { immediate: true });
watch(
  () => settings.language,
  () => {
    cancelTranslation();
    translated.value = "";
    showTranslation.value = false;
  },
);
onBeforeUnmount(() => {
  listSeq++;
  resetDetail();
});
</script>

<template>
  <section class="flex min-h-0 gap-3 p-4">
    <!-- 布局约定:左列审计内容(含标题行)原生滚动,右侧来源卡片固定;保持 section 唯一根节点,父级 class(min-h-0 flex-1)才能继承 -->
    <div class="min-h-0 min-w-0 flex-1 overflow-y-auto">
      <div v-if="selection === 'local'" class="space-y-3">
        <slot name="local" />
      </div>
      <template v-else-if="selected">
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-sm font-medium">
            {{ selected.name }} ·
            <span :class="statusClass(selected.status)">{{ selected.status }}</span>
            <span v-if="auditedAt" class="ml-1 font-normal text-muted-foreground">
              · {{ formatAuditDate(auditedAt) }}
            </span>
          </span>
          <Button
            variant="outline"
            size="sm"
            :disabled="!original || translating"
            @click="translate"
          >
            <Languages class="mr-1 h-3.5 w-3.5" />
            {{
              t(
                translating
                  ? "settings.resources.skills.previewPage.translate.translating"
                  : showTranslation
                    ? "settings.resources.skills.previewPage.translate.showOriginal"
                    : "settings.resources.skills.previewPage.translate.trigger",
              )
            }}
          </Button>
        </div>
        <p v-if="detailLoading" class="mt-3 text-sm">{{ t("common.loading") }}</p>
        <div v-else-if="detailError" class="mt-3 space-y-2 text-sm text-destructive" role="alert">
          {{ detailError }}
          <Button variant="outline" size="sm" @click="selectAudit(selected)">{{
            t("settings.resources.market.refresh")
          }}</Button>
        </div>
        <div v-else class="audit-markdown mt-3 text-sm leading-relaxed" @click="onMarkdownClick">
          <Markdown
            mode="static"
            :content="showTranslation ? translated : original"
            :theme-element="themeElement"
            :locale="language"
            :node-renderers="nodeRenderers"
          />
        </div>
      </template>
    </div>
    <!-- 审计来源卡片 -->
    <div class="w-36 shrink-0 self-start rounded-lg border bg-popover p-1.5 text-sm shadow-md">
      <div class="flex items-center justify-between gap-1 px-1 pb-1">
        <p class="text-[11px] uppercase text-muted-foreground">
          {{ t("settings.resources.market.remote.auditSources") }}
        </p>
        <Button
          v-if="marketplaceId"
          variant="ghost"
          size="icon"
          class="h-5 w-5"
          :disabled="loading"
          :title="t('settings.resources.market.refresh')"
          @click="load"
        >
          <RotateCw class="h-3 w-3" />
        </Button>
      </div>
      <button
        type="button"
        class="hover:bg-accent block w-full truncate rounded-sm px-2 py-1.5 text-left text-xs transition-colors"
        :class="selection === 'local' ? 'bg-accent text-foreground' : 'text-muted-foreground'"
        @click="selectLocal"
      >
        <span class="block truncate">{{
          t("settings.resources.skills.previewPage.scan.title")
        }}</span>
        <span class="block text-[11px]" :class="localStatusClass">{{ localStatus }}</span>
      </button>
      <p v-if="loading" class="px-2 py-1.5 text-xs text-muted-foreground">
        {{ t("common.loading") }}
      </p>
      <p v-else-if="error" role="alert" class="px-2 py-1.5 text-xs text-destructive">
        {{ error }}
      </p>
      <p
        v-else-if="marketplaceId && !audits.length"
        class="px-2 py-1.5 text-[11px] text-muted-foreground"
      >
        {{ t("settings.resources.market.remote.noAudits") }}
      </p>
      <div v-for="audit in audits" :key="audit.provider" class="flex items-center gap-0.5">
        <button
          type="button"
          class="hover:bg-accent block min-w-0 flex-1 truncate rounded-sm px-2 py-1.5 text-left text-xs transition-colors"
          :class="
            selection === audit.provider ? 'bg-accent text-foreground' : 'text-muted-foreground'
          "
          :title="audit.name"
          @click="selectAudit(audit)"
        >
          <span class="block truncate">{{ audit.name }}</span>
          <span class="block text-[11px]" :class="statusClass(audit.status)">{{
            audit.status
          }}</span>
        </button>
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-muted-foreground"
          :title="t('settings.resources.market.remote.originalAudit')"
          @click="openOriginal(audit.url)"
        >
          <ExternalLink class="h-3 w-3" />
        </Button>
      </div>
    </div>
  </section>
</template>
