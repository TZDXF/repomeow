<script setup lang="ts">
import { ref } from "vue";
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
