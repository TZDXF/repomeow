<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
import { Switch } from "@/components/ui/switch";

const { t } = useI18n();

// 开机自启的权威状态在系统侧(Windows 注册表 Run 项 / macOS LaunchAgent),
// 不落 settings.json:加载时从插件读,切换失败时回滚开关并提示
const enabled = ref(false);
const busy = ref(false);

onMounted(async () => {
  try {
    enabled.value = await isEnabled();
  } catch {
    /* 读取失败保持关闭态,不影响设置页其它功能 */
  }
});

async function onToggle(value: boolean) {
  if (busy.value) {
    return;
  }
  busy.value = true;
  try {
    if (value) {
      await enable();
    } else {
      await disable();
    }
    enabled.value = value;
  } catch (e) {
    toast.error(t("settings.autostart.toggleFailed", { error: String(e) }));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section>
    <div class="flex items-center justify-between rounded-lg border px-3 py-2.5">
      <div class="flex flex-col gap-0.5">
        <span data-setting="settings.autostart.launchAtLogin" class="text-sm font-medium">{{
          t("settings.autostart.launchAtLogin")
        }}</span>
        <span class="text-xs text-muted-foreground">{{
          t("settings.autostart.launchAtLoginHint")
        }}</span>
      </div>
      <Switch :model-value="enabled" :disabled="busy" @update:model-value="onToggle" />
    </div>
  </section>
</template>
