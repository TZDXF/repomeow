<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { RefreshCw, ShieldCheck } from "@lucide/vue";
import {
  ModelSelector,
  modelDisplayName,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatRelativeTime } from "@/lib/format";
import type { ResourceSkillScanReport } from "@/lib/resource-library";
import { useAiConfigStore } from "@/stores/ai-config";
import {
  scanCategoryLabel,
  scanFindingTitle,
  scanLevelClass,
  scanLevelLabel,
  scanLlmNotice,
  scanSeverityClass,
  scanSeverityLabel,
} from "./scan-labels";
import type { SkillScanModelRef } from "@/composables/useSkillScan";

/**
 * 技能安全扫描面板(资源库技能预览页 #local 插槽内容):
 * 扫描控制(模型选择 + 运行/重新扫描)、进行中进度、报告展示(等级/评分、
 * AI 语义层提示与摘要、发现列表)。扫描生命周期由父级 useSkillScan 持有,
 * 这里只管展示与模型选择。
 */
defineProps<{
  scanning: boolean;
  report: ResourceSkillScanReport | null;
  /** 扫描进行中的已用时长(mm:ss) */
  elapsedLabel: string;
}>();
const emit = defineEmits<{
  /** 运行/重新扫描;model 为 null 时跟随默认模型 */
  (e: "run", model: SkillScanModelRef | null): void;
  (e: "cancel"): void;
}>();

const { t, te } = useI18n();
const aiConfig = useAiConfigStore();

// 模型选择器数据源(已加载时复用内存副本)
onMounted(() => {
  aiConfig.ensureLoaded().catch(() => {});
});
const i18n = { t, te };

// ── 扫描模型选择:复合值 "providerId/modelId",缺省跟随设置页默认模型 ──
// 显式引用失效(厂商/模型被删)时后端会回退默认模型,前端在配置加载后
// 把选项归位,保证触发器显示与实际生效模型一致。
const SCAN_MODEL_STORAGE_KEY = "repomeow.rl-scan-model";
/** 通用选项哨兵值:不含 "/",parseModelOptionValue 解析为 null = 跟随默认模型 */
const SCAN_MODEL_DEFAULT = "default";
const scanModelValue = ref(localStorage.getItem(SCAN_MODEL_STORAGE_KEY) ?? SCAN_MODEL_DEFAULT);

const scanDefaultModelLabel = computed(() => {
  const model = aiConfig.defaultModel?.model;
  return model
    ? t("settings.resources.skills.previewPage.scan.modelDefaultNamed", {
        model: modelDisplayName(model),
      })
    : t("settings.resources.skills.previewPage.scan.modelDefault");
});

const scanModelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

watch(
  () => aiConfig.loaded,
  (loaded) => {
    if (!loaded || scanModelValue.value === SCAN_MODEL_DEFAULT) return;
    const valid = scanModelGroups.value.some((group) =>
      group.models.some((model) => `${group.providerId}/${model.id}` === scanModelValue.value),
    );
    if (!valid) scanModelValue.value = SCAN_MODEL_DEFAULT;
  },
  { immediate: true },
);

watch(scanModelValue, (value) => {
  if (value === SCAN_MODEL_DEFAULT) {
    localStorage.removeItem(SCAN_MODEL_STORAGE_KEY);
  } else {
    localStorage.setItem(SCAN_MODEL_STORAGE_KEY, value);
  }
});

function onRun() {
  // 通用选项(跟随默认模型)或非法复合值都按 null 上抛
  emit("run", parseModelOptionValue(scanModelValue.value));
}
</script>

<template>
  <!-- 扫描控制:模型选择 + 运行/重新扫描 -->
  <div class="flex items-center justify-end gap-1.5">
    <span :title="t('settings.resources.skills.previewPage.scan.modelLabel')">
      <ModelSelector
        v-model="scanModelValue"
        :groups="scanModelGroups"
        :disabled="scanning"
        :generic-option="{
          value: SCAN_MODEL_DEFAULT,
          label: scanDefaultModelLabel,
        }"
        trigger-class="text-muted-foreground min-w-0 max-w-96"
      />
    </span>
    <Button
      variant="outline"
      size="sm"
      class="h-7 shrink-0 gap-1 px-2 text-xs"
      :disabled="scanning"
      @click="onRun"
    >
      <ShieldCheck v-if="!report" class="h-3.5 w-3.5" />
      <RefreshCw v-else class="h-3.5 w-3.5" />
      {{
        t(
          report
            ? "settings.resources.skills.previewPage.scan.rerun"
            : "settings.resources.skills.previewPage.scan.run",
        )
      }}
    </Button>
  </div>

  <div v-if="scanning" class="flex flex-col items-center gap-4 px-6 py-16 text-center">
    <div class="relative flex h-14 w-14 items-center justify-center">
      <span class="scan-pulse absolute inset-0 rounded-full bg-primary/15" />
      <span class="absolute inset-2 rounded-full border border-primary/25" />
      <ShieldCheck class="h-6 w-6 text-primary" />
    </div>
    <div class="space-y-1">
      <p class="text-sm font-medium">
        {{ t("settings.resources.skills.previewPage.scan.running") }}
      </p>
      <p class="text-xs text-muted-foreground">
        {{ t("settings.resources.skills.previewPage.scan.runningHint") }}
      </p>
    </div>
    <div class="h-0.5 w-52 overflow-hidden rounded-full bg-muted">
      <div class="scan-indeterminate h-full w-1/3 rounded-full bg-primary/70" />
    </div>
    <p class="font-mono text-[11px] tabular-nums text-muted-foreground/70">
      {{ elapsedLabel }}
    </p>
    <Button variant="ghost" size="sm" class="h-7 px-3 text-xs" @click="emit('cancel')">
      {{ t("settings.resources.skills.previewPage.scan.cancel") }}
    </Button>
  </div>

  <template v-else-if="report">
    <div class="flex flex-wrap items-center gap-2">
      <span
        class="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium"
        :class="scanLevelClass(report.level)"
      >
        {{ scanLevelLabel(i18n, report.level) }}
        <span class="font-mono">{{ report.score }}</span>
      </span>
      <span class="text-xs text-muted-foreground">
        {{
          t("settings.resources.skills.previewPage.scan.filesScanned", {
            count: report.filesScanned,
          })
        }}
      </span>
      <span class="text-xs text-muted-foreground">
        {{
          t("settings.resources.skills.previewPage.scan.scannedAt", {
            time: formatRelativeTime(report.scannedAt),
          })
        }}
      </span>
      <span v-if="report.suppressedCount > 0" class="text-xs text-muted-foreground">
        {{
          t("settings.resources.skills.previewPage.scan.suppressed", {
            count: report.suppressedCount,
          })
        }}
      </span>
    </div>

    <p v-if="scanLlmNotice(i18n, report)" class="text-xs text-amber-600 dark:text-amber-400">
      {{ scanLlmNotice(i18n, report) }}
    </p>
    <div v-else-if="report.llmSummary" class="rounded-md border bg-muted/40 p-2.5 text-xs">
      <span class="font-medium">
        {{ t("settings.resources.skills.previewPage.scan.summary") }}
      </span>
      <span class="text-muted-foreground">{{ report.llmSummary }}</span>
    </div>

    <p
      v-if="!report.findings.length"
      class="rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{ t("settings.resources.skills.previewPage.scan.noFindings") }}
    </p>
    <div v-else class="space-y-2">
      <div
        v-for="(finding, index) in report.findings"
        :key="`${finding.ruleId ?? 'llm'}-${index}`"
        class="rounded-md border p-3"
      >
        <div class="flex flex-wrap items-center gap-1.5">
          <span
            class="rounded-full border px-2 py-0.5 text-[11px] font-medium"
            :class="scanSeverityClass(finding.severity)"
          >
            {{ scanSeverityLabel(i18n, finding.severity) }}
          </span>
          <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
            {{ scanCategoryLabel(i18n, finding) }}
          </span>
          <Badge
            variant="outline"
            class="text-[11px] font-normal"
            :title="
              t(
                finding.source === 'llm'
                  ? 'settings.resources.skills.previewPage.scan.llmAnalysis'
                  : 'settings.resources.skills.previewPage.scan.staticRules',
              )
            "
          >
            {{
              t(
                finding.source === "llm"
                  ? "settings.resources.skills.previewPage.scan.source.llm"
                  : "settings.resources.skills.previewPage.scan.source.static",
              )
            }}
          </Badge>
          <span
            v-if="finding.location"
            class="ml-auto min-w-0 truncate font-mono text-[11px] text-muted-foreground"
            :title="finding.location"
          >
            {{ finding.location }}
          </span>
        </div>
        <p class="mt-2 text-sm font-medium">{{ scanFindingTitle(i18n, finding) }}</p>
        <p v-if="finding.detail" class="mt-1 text-xs text-muted-foreground">
          {{ finding.detail }}
        </p>
        <code
          v-if="finding.evidence"
          class="mt-2 block max-h-24 overflow-auto rounded bg-muted px-2 py-1.5 font-mono text-xs"
        >
          {{ finding.evidence }}
        </code>
      </div>
    </div>
  </template>

  <p
    v-else
    class="rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
  >
    {{ t("settings.resources.skills.previewPage.scan.notScanned") }}
  </p>
</template>

<style scoped>
/* 扫描进行中:细进度条滑动 + 盾牌外圈脉冲 */
@keyframes scan-slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(400%);
  }
}

.scan-indeterminate {
  animation: scan-slide 1.4s ease-in-out infinite;
}

@keyframes scan-pulse {
  0% {
    transform: scale(0.85);
    opacity: 0.9;
  }
  70%,
  100% {
    transform: scale(1.4);
    opacity: 0;
  }
}

.scan-pulse {
  animation: scan-pulse 1.8s ease-out infinite;
}
</style>
