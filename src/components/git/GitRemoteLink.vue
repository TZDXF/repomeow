<script setup lang="ts">
import { DropdownMenuContent } from "@/components/common/project-floating-content";
import { computed, ref, watch, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import { ChevronDown, Globe } from "@lucide/vue";
import GithubIcon from "@/components/icons/GithubIcon.vue";
import GiteeIcon from "@/components/icons/GiteeIcon.vue";
import GitlabIcon from "@/components/icons/GitlabIcon.vue";
import { DropdownMenu, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cmd } from "@/lib/tauri";
import { parseGitRemote, type GitProvider, type GitRemoteInfo } from "@/lib/git-remote";
import type { GitRemote, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();

interface RemoteEntry {
  name: string;
  info: GitRemoteInfo;
}

const remotes = ref<RemoteEntry[]>([]);

// 进入详情页/切换项目时读取一次 remote 列表(无远端或解析失败则不显示按钮)
watch(
  () => props.project.path,
  async (path) => {
    remotes.value = [];
    let list: GitRemote[];
    try {
      list = await cmd<GitRemote[]>("list_git_remotes", { path });
    } catch {
      return;
    }
    remotes.value = list.flatMap((r) => {
      const info = parseGitRemote(r.url);
      return info ? [{ name: r.name, info }] : [];
    });
  },
  { immediate: true },
);

const PROVIDER_ICONS: Record<GitProvider, Component> = {
  github: GithubIcon,
  gitee: GiteeIcon,
  gitlab: GitlabIcon,
  generic: Globe,
};

function providerIcon(provider: GitProvider) {
  return PROVIDER_ICONS[provider];
}

const PROVIDER_NAMES: Record<GitProvider, string> = {
  github: "GitHub",
  gitee: "Gitee",
  gitlab: "GitLab",
  generic: "",
};

function providerName(entry: RemoteEntry): string {
  return PROVIDER_NAMES[entry.info.provider] || entry.name;
}

/** 多个远端时以 origin 优先决定按钮图标,否则取第一个 */
const primary = computed(
  () => remotes.value.find((r) => r.name === "origin") ?? remotes.value[0] ?? null,
);

async function openRemote(entry: RemoteEntry) {
  try {
    await openUrl(entry.info.url);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <!-- 单个远端:图标按钮直接打开 -->
  <button
    v-if="remotes.length === 1 && primary"
    type="button"
    class="flex items-center rounded p-0.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
    :title="t('git.openRemote', { name: providerName(primary) })"
    @click="openRemote(primary)"
  >
    <component :is="providerIcon(primary.info.provider)" class="h-3.5 w-3.5" />
  </button>

  <!-- 多个远端:下拉列出每个远端供选择 -->
  <DropdownMenu v-else-if="remotes.length > 1 && primary">
    <DropdownMenuTrigger as-child>
      <button
        type="button"
        class="flex items-center gap-0.5 rounded p-0.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('git.openRemote', { name: remotes.map((r) => r.name).join(', ') })"
      >
        <component :is="providerIcon(primary.info.provider)" class="h-3.5 w-3.5" />
        <ChevronDown class="h-3 w-3 opacity-60" />
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" class="w-64">
      <DropdownMenuItem
        v-for="r in remotes"
        :key="r.name"
        class="gap-2 text-xs"
        :title="r.info.url"
        @click="openRemote(r)"
      >
        <component
          :is="providerIcon(r.info.provider)"
          class="h-3.5 w-3.5 shrink-0 text-muted-foreground"
        />
        <span class="shrink-0 font-medium">{{ r.name }}</span>
        <span class="truncate text-muted-foreground">{{ r.info.url }}</span>
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
