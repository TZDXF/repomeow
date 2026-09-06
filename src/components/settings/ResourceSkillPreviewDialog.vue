<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderOpen } from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import type { SupportedLocale } from "@/i18n";
import { createBeforeDownload } from "@/lib/markdown-download";
import {
  openResourceSkillDir,
  readResourceSkillBody,
  type ResourceSkill,
} from "@/lib/resource-library";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

const props = defineProps<{
  open: boolean;
  skill: ResourceSkill;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
}>();

const { t, locale } = useI18n();

const body = ref("");
const loadingBody = ref(false);
const language = computed(() => locale.value as SupportedLocale);

const controls: ControlsConfig = {
  table: { copy: true, download: true },
  code: { copy: true, collapse: true },
};
const beforeDownload = createBeforeDownload(t);

// 传游离元素,避免 Markdown 库将 island/glass 的十六进制主题变量写成无效的 hsl(#…)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

/** 每次打开重新加载正文;token 丢弃迟到的加载结果。
 * immediate 必开:对话框随 open=true 首次挂载(v-if 与 open 同时变真),
 * 不加 immediate 时挂载初值不会触发 watch,正文永远不加载。 */
let loadToken = 0;

watch(
  () => props.open,
  async (open) => {
    if (!open) {
      return;
    }
    const token = ++loadToken;
    body.value = "";
    loadingBody.value = true;
    try {
      const loaded = await readResourceSkillBody(props.skill.id);
      if (token === loadToken) {
        body.value = loaded.body;
      }
    } catch (e) {
      if (token === loadToken) {
        toast.error(
          t("settings.resources.skills.previewDialog.loadBodyFailed", { error: String(e) }),
        );
      }
    } finally {
      if (token === loadToken) {
        loadingBody.value = false;
      }
    }
  },
  { immediate: true },
);

async function openDir() {
  try {
    await openResourceSkillDir(props.skill.id);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle :title="skill.name">{{ skill.name }}</DialogTitle>
        <DialogDescription v-if="skill.description">{{ skill.description }}</DialogDescription>
      </DialogHeader>

      <div class="max-h-[60vh] overflow-y-auto pr-1">
        <p v-if="loadingBody" class="py-8 text-center text-xs text-muted-foreground">
          {{ t("common.loading") }}
        </p>
        <p
          v-else-if="!body"
          class="rounded-md border border-dashed px-3 py-8 text-center text-xs text-muted-foreground"
        >
          {{ t("settings.resources.skills.previewDialog.emptyBody") }}
        </p>
        <Markdown
          v-else
          mode="static"
          :content="body"
          :controls="controls"
          :theme-element="themeElement"
          :locale="language"
          :before-download="beforeDownload"
        />
      </div>

      <DialogFooter>
        <Button variant="outline" class="gap-1.5" @click="openDir">
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("settings.resources.skills.previewDialog.openDir") }}
        </Button>
        <Button variant="outline" @click="emit('update:open', false)">
          {{ t("common.close") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
