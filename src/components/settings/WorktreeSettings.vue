<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Input } from "@/components/ui/input";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

// 本地草稿:失焦/回车时非空才持久化,避免输入中途清空导致模板丢失
const draft = ref(store.worktreeDirTemplate);

watch(
  () => store.worktreeDirTemplate,
  (v) => {
    draft.value = v;
  },
);

function commit() {
  const v = draft.value.trim();
  if (v) {
    store.setWorktreeDirTemplate(v);
  } else {
    draft.value = store.worktreeDirTemplate;
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.general.worktreeDir") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.general.worktreeDirDescription") }}
    </p>
    <Input
      v-model="draft"
      class="mt-3 max-w-md font-mono text-sm"
      :placeholder="'.worktrees/{branch}'"
      @blur="commit"
      @keydown.enter="commit"
    />
  </section>
</template>
