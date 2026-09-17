<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Terminal } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { cmd } from "@/lib/tauri";

interface CliSkillsInstallResult {
  installed: string[];
  updated: string[];
}

const { t } = useI18n();
const installing = ref(false);
const pathStatus = ref<{ supported: boolean; directory: string; added: boolean } | null>(null);
const pathBusy = ref(false);
onMounted(async () => {
  try {
    pathStatus.value = await cmd("cli_get_path_status");
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
});
async function togglePath() {
  if (!pathStatus.value || pathBusy.value) {
    return;
  }
  pathBusy.value = true;
  try {
    pathStatus.value = await cmd("cli_set_user_path", { enabled: !pathStatus.value.added });
    toast.success(t("settings.cli.pathUpdated"));
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    pathBusy.value = false;
  }
}

const skills = [
  { name: "repomeow", icon: Terminal, descKey: "settings.cli.skillRepomeow" },
] as const;

async function installSkills() {
  if (installing.value) {
    return;
  }
  installing.value = true;
  try {
    const result = await cmd<CliSkillsInstallResult>("cli_install_builtin_skills");
    toast.success(
      t("settings.cli.installDone", {
        added: result.installed.length,
        updated: result.updated.length,
      }),
    );
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    installing.value = false;
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.cli.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.cli.description") }}
    </p>

    <div v-if="pathStatus?.supported" class="mt-5 rounded-lg border p-4">
      <h3 class="text-sm font-semibold">{{ t("settings.cli.pathTitle") }}</h3>
      <p class="mt-1 text-xs text-muted-foreground">{{ t("settings.cli.pathHint") }}</p>
      <code class="mt-3 block break-all text-xs">{{ pathStatus.directory }}</code>
      <p class="mt-2 text-xs text-muted-foreground">
        {{ t(pathStatus.added ? "settings.cli.pathAdded" : "settings.cli.pathMissing") }}
      </p>
      <Button class="mt-3" variant="outline" :disabled="pathBusy" @click="togglePath">
        {{ t(pathStatus.added ? "settings.cli.pathRemove" : "settings.cli.pathAdd") }}
      </Button>
    </div>
    <div class="mt-5">
      <h3 class="text-sm font-semibold">{{ t("settings.cli.skillsTitle") }}</h3>
      <p class="mt-1 text-xs text-muted-foreground">
        {{ t("settings.cli.skillsHint") }}
      </p>

      <div class="mt-3 flex flex-col gap-3">
        <div
          v-for="skill in skills"
          :key="skill.name"
          class="flex items-center gap-3 rounded-lg border px-3 py-3"
        >
          <div class="rounded-md bg-muted p-2 text-muted-foreground">
            <component :is="skill.icon" class="h-4 w-4" />
          </div>
          <div class="min-w-0">
            <p class="text-sm font-medium">
              <code class="text-[13px]">{{ skill.name }}</code>
            </p>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {{ t(skill.descKey) }}
            </p>
          </div>
        </div>
      </div>

      <div class="mt-4">
        <Button :disabled="installing" @click="installSkills">
          {{ installing ? t("settings.cli.installing") : t("settings.cli.install") }}
        </Button>
      </div>
    </div>
  </section>
</template>
