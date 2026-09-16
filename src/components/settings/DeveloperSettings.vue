<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Switch } from "@/components/ui/switch";
import { enableDeveloperMode, disableDeveloperMode } from "@/lib/developer-mode";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const settings = useSettingsStore();

// dev 构建恒启用(见 main.ts),开关状态仍落盘,供 release 构建启动时读取;
// 运行时切换即时生效:注册/移除 F12 监听 + 加载/销毁元素选取工具
async function onToggle(value: boolean) {
  await settings.setDeveloperMode(value);
  if (value) {
    await enableDeveloperMode();
  } else {
    disableDeveloperMode();
  }
}
</script>

<template>
  <section>
    <div class="flex items-center justify-between rounded-lg border px-3 py-2.5">
      <div class="flex flex-col gap-0.5">
        <span class="text-sm font-medium">{{ t("settings.developer.title") }}</span>
        <span class="text-xs text-muted-foreground">{{ t("settings.developer.hint") }}</span>
      </div>
      <Switch :model-value="settings.developerMode" @update:model-value="onToggle" />
    </div>
  </section>
</template>
