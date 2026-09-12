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
} from "@/lib/resource-library";
import { Markdown, type NodeRenderers } from "vue-stream-markdown";
import MdLink from "@/components/markdown/MdLink.vue";
import type { SupportedLocale } from "@/i18n";
import { hasScheme, safeLinkHref } from "@/lib/markdown";
import { formatAuditDate } from "@/lib/format";
import { cmd } from "@/lib/tauri";
import { getCachedTranslation, putCachedTranslation } from "@/lib/translation-cache";
import { useSettingsStore } from "@/stores/settings";

const props = defineProps<{ marketplaceId: string | null }>();
const { t, locale } = useI18n();
const language = computed(() => locale.value as SupportedLocale);
const nodeRenderers: NodeRenderers = { link: MdLink };
// 传游离元素,避免 Markdown 库把 island/glass 主题变量写成无效 hsl(#…)(与技能预览一致)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
const settings = useSettingsStore();
const audits = ref<ResourcePublicAudit[]>([]);
const loading = ref(false);
const error = ref("");
const selected = ref<ResourcePublicAudit | null>(null);
const detailLoading = ref(false);
const detailError = ref("");
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
  selected.value = null;
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
    // 默认展示第一条审计详情,侧边栏可切换
    if (result.length) {
      void selectAudit(result[0]);
    }
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
async function selectAudit(audit: ResourcePublicAudit) {
  if (!props.marketplaceId || detailLoading.value) {
    return;
  }
  detailSeq++;
  cancelTranslation();
  selected.value = audit;
  original.value = "";
  auditedAt.value = "";
  translated.value = "";
  showTranslation.value = false;
  detailError.value = "";
  detailLoading.value = true;
  const seq = detailSeq;
  try {
    const detail = await readMarketplaceAuditDetail(props.marketplaceId, audit.provider);
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
  <section class="space-y-3 rounded-lg border p-4">
    <div class="flex items-center justify-between gap-2">
      <h3 class="text-sm font-medium">
        {{ t("settings.resources.market.remote.audits") }}
      </h3>
      <Button v-if="marketplaceId" variant="ghost" size="sm" :disabled="loading" @click="load">
        <RotateCw class="mr-1 h-3.5 w-3.5" />{{ t("settings.resources.market.refresh") }}
      </Button>
    </div>
    <p class="text-xs text-muted-foreground">
      {{ t("settings.resources.market.remote.auditDisclaimer") }}
    </p>
    <p v-if="loading" class="text-xs text-muted-foreground">{{ t("common.loading") }}</p>
    <p v-else-if="error" role="alert" class="text-xs text-destructive">{{ error }}</p>
    <p v-else-if="!audits.length" class="text-xs text-muted-foreground">
      {{
        t(
          marketplaceId
            ? "settings.resources.market.remote.noAudits"
            : "settings.resources.market.remote.noAuditSource",
        )
      }}
    </p>
    <!-- 侧边栏列出审计来源,正文直接内联展示选中来源的详情,不再弹窗 -->
    <div v-else class="flex min-w-0 items-stretch gap-0">
      <aside class="w-36 shrink-0 border-r pr-3">
        <p class="mb-1.5 text-[11px] uppercase text-muted-foreground">
          {{ t("settings.resources.market.remote.auditSources") }}
        </p>
        <button
          v-for="audit in audits"
          :key="audit.provider"
          type="button"
          class="hover:bg-accent block w-full truncate rounded-sm px-2 py-1.5 text-left text-xs transition-colors"
          :class="
            selected?.provider === audit.provider
              ? 'bg-accent text-foreground'
              : 'text-muted-foreground'
          "
          :title="audit.name"
          @click="selectAudit(audit)"
        >
          <span class="block truncate">{{ audit.name }}</span>
          <span class="block text-[11px]" :class="statusClass(audit.status)">{{
            audit.status
          }}</span>
        </button>
      </aside>
      <div class="min-w-0 flex-1 pl-3">
        <div class="flex flex-wrap items-center gap-2">
          <span v-if="selected" class="text-sm font-medium">
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
          <Button v-if="selected" variant="ghost" size="sm" @click="openOriginal(selected.url)">
            <ExternalLink class="mr-1 h-3.5 w-3.5" />{{
              t("settings.resources.market.remote.originalAudit")
            }}
          </Button>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">
          {{ t("settings.resources.market.remote.translationHint") }}
        </p>
        <p v-if="detailLoading" class="mt-3 text-sm">{{ t("common.loading") }}</p>
        <div v-else-if="detailError" class="mt-3 space-y-2 text-sm text-destructive" role="alert">
          {{ detailError }}
          <Button v-if="selected" variant="outline" size="sm" @click="selectAudit(selected)">{{
            t("settings.resources.market.refresh")
          }}</Button>
        </div>
        <!-- 外层扫描页已有滚动容器,这里用原生 overflow 避免嵌套 reka ScrollArea(其注入的 viewport <style> 会污染区域文本) -->
        <div
          v-else
          class="audit-markdown mt-3 max-h-96 overflow-y-auto pr-3 text-sm leading-relaxed"
          @click="onMarkdownClick"
        >
          <Markdown
            mode="static"
            :content="showTranslation ? translated : original"
            :theme-element="themeElement"
            :locale="language"
            :node-renderers="nodeRenderers"
          />
        </div>
      </div>
    </div>
  </section>
</template>
