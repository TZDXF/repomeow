<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Star } from "@lucide/vue";
import { toast } from "vue-sonner";
import { readMarketplaceRepository } from "@/lib/resource-library";

const props = defineProps<{ source: string }>();
const { t } = useI18n();
const stars = ref<number | null>(null);
const loading = ref(false);
const error = ref("");
let sequence = 0;
async function load() {
  const seq = ++sequence;
  stars.value = null;
  error.value = "";
  loading.value = true;
  try {
    const result = await readMarketplaceRepository(props.source);
    if (seq === sequence) {
      stars.value = result.stars;
    }
  } catch (e) {
    if (seq === sequence) {
      error.value = String(e);
    }
  } finally {
    if (seq === sequence) {
      loading.value = false;
    }
  }
}
async function openRepository() {
  try {
    await openUrl(`https://github.com/${props.source}`);
  } catch (e) {
    toast.error(String(e));
  }
}
watch(() => props.source, load, { immediate: true });
</script>

<template>
  <button
    type="button"
    class="flex shrink-0 items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
    :title="error || t('settings.resources.market.remote.repoStars')"
    :disabled="loading"
    @click="error ? load() : openRepository()"
  >
    <Star class="h-3.5 w-3.5" />
    {{ t("settings.resources.market.remote.repoStars") }}:
    {{
      loading
        ? "…"
        : stars !== null
          ? String(stars)
          : t("settings.resources.market.remote.retryStars")
    }}
  </button>
</template>
