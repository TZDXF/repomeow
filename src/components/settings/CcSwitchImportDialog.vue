<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { listCcSwitchProviders, type CcSwitchProvider, type CcSwitchScan } from "@/lib/ai-config";

const props = defineProps<{
  open: boolean;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 确认导入:勾选的供应商原文交给父组件并入草稿 */
  import: [providers: CcSwitchProvider[]];
}>();

const { t } = useI18n();

const ccSwitchLoading = ref(false);
const ccSwitchScan = ref<CcSwitchScan | null>(null);
/** 勾选项的 key:`${app}/${id}`(不同应用间 id 可能重复) */
const ccSwitchChecked = ref<Set<string>>(new Set());

function ccSwitchKey(provider: CcSwitchProvider): string {
  return `${provider.app}/${provider.id}`;
}

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    ccSwitchLoading.value = true;
    ccSwitchScan.value = null;
    try {
      const scan = await listCcSwitchProviders();
      ccSwitchScan.value = scan;
      // 默认勾选「当前启用」的供应商,其余由用户自行挑选
      ccSwitchChecked.value = new Set(
        scan.providers.filter((provider) => provider.current).map(ccSwitchKey),
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      emit("update:open", false);
    } finally {
      ccSwitchLoading.value = false;
    }
  },
);

function toggleCcSwitchProvider(provider: CcSwitchProvider) {
  const key = ccSwitchKey(provider);
  const next = new Set(ccSwitchChecked.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  ccSwitchChecked.value = next;
}

const ccSwitchAllChecked = computed(() => {
  const providers = ccSwitchScan.value?.providers ?? [];
  return (
    providers.length > 0 &&
    providers.every((provider) => ccSwitchChecked.value.has(ccSwitchKey(provider)))
  );
});

function toggleCcSwitchAll() {
  const providers = ccSwitchScan.value?.providers ?? [];
  ccSwitchChecked.value = ccSwitchAllChecked.value
    ? new Set()
    : new Set(providers.map(ccSwitchKey));
}

function confirmCcSwitchImport() {
  const providers = (ccSwitchScan.value?.providers ?? []).filter((provider) =>
    ccSwitchChecked.value.has(ccSwitchKey(provider)),
  );
  if (!providers.length) return;
  emit("import", providers);
  emit("update:open", false);
}
</script>

<template>
  <!-- 列出本机 ~/.cc-switch 中 OpenAI chat 兼容的供应商,勾选后并入草稿 -->
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>{{ t("settings.ai.ccSwitchTitle") }}</DialogTitle>
        <DialogDescription>{{ t("settings.ai.ccSwitchDesc") }}</DialogDescription>
      </DialogHeader>

      <div
        v-if="ccSwitchLoading"
        class="text-muted-foreground flex items-center gap-2 py-6 text-sm"
      >
        <Loader2 class="h-4 w-4 animate-spin" />
        {{ t("settings.ai.ccSwitchLoading") }}
      </div>
      <p v-else-if="ccSwitchScan && !ccSwitchScan.found" class="text-muted-foreground py-6 text-sm">
        {{ t("settings.ai.ccSwitchNotFound") }}
      </p>
      <p
        v-else-if="ccSwitchScan && !ccSwitchScan.providers.length"
        class="text-muted-foreground py-6 text-sm"
      >
        {{ t("settings.ai.ccSwitchEmpty") }}
      </p>
      <template v-else-if="ccSwitchScan">
        <div class="flex items-center justify-between">
          <span class="text-muted-foreground text-xs">
            {{
              t("settings.ai.ccSwitchSelected", {
                selected: ccSwitchChecked.size,
                total: ccSwitchScan.providers.length,
              })
            }}
          </span>
          <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" @click="toggleCcSwitchAll">
            {{
              ccSwitchAllChecked
                ? t("settings.ai.ccSwitchDeselectAll")
                : t("settings.ai.ccSwitchSelectAll")
            }}
          </Button>
        </div>
        <div class="flex max-h-72 flex-col gap-1.5 overflow-y-auto py-1">
          <button
            v-for="provider in ccSwitchScan.providers"
            :key="ccSwitchKey(provider)"
            type="button"
            class="hover:bg-accent flex items-center gap-2 rounded-md border px-2 py-1.5 text-left"
            :class="
              ccSwitchChecked.has(ccSwitchKey(provider)) ? 'border-primary/50 bg-primary/5' : ''
            "
            @click="toggleCcSwitchProvider(provider)"
          >
            <span
              class="flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border"
              :class="
                ccSwitchChecked.has(ccSwitchKey(provider))
                  ? 'border-primary bg-primary text-primary-foreground'
                  : 'border-input'
              "
            >
              <Check v-if="ccSwitchChecked.has(ccSwitchKey(provider))" class="h-3 w-3" />
            </span>
            <span class="flex min-w-0 flex-1 flex-col">
              <span class="flex items-center gap-1.5">
                <span class="truncate text-sm font-medium">
                  {{ provider.name.trim() || provider.id }}
                </span>
                <span
                  class="bg-muted text-muted-foreground shrink-0 rounded-full px-1.5 py-px text-[10px]"
                >
                  {{ provider.app }}
                </span>
                <span
                  v-if="provider.current"
                  class="bg-primary/10 text-primary shrink-0 rounded-full px-1.5 py-px text-[10px]"
                >
                  {{ t("settings.ai.ccSwitchCurrent") }}
                </span>
              </span>
              <span class="text-muted-foreground truncate text-xs">{{ provider.baseUrl }}</span>
            </span>
            <span class="text-muted-foreground shrink-0 text-xs">
              {{
                provider.apiKey
                  ? t("settings.ai.modelCount", { count: provider.models.length })
                  : t("settings.ai.ccSwitchNoKey")
              }}
            </span>
          </button>
        </div>
      </template>

      <DialogFooter>
        <Button variant="outline" @click="emit('update:open', false)">
          {{ t("common.cancel") }}
        </Button>
        <Button :disabled="!ccSwitchChecked.size" @click="confirmCcSwitchImport">
          {{ t("settings.ai.ccSwitchImport", { count: ccSwitchChecked.size }) }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
