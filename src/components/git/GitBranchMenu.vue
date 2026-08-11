<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, GitBranchPlus, Globe, Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useProjectsStore } from "@/stores/projects";
import type { GitBranches, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const store = useProjectsStore();

const open = ref(false);
const branches = ref<GitBranches>({ local: [], remote: [] });
const loading = ref(false);
const switching = ref(false);

// --- 新建分支对话框 ---
const createOpen = ref(false);
const newBranch = ref("");
const baseBranch = ref("");
const creating = ref(false);

/** 远程分支去掉远端名前缀: "origin/team/x" -> "team/x" */
function remoteShortName(remote: string) {
  return remote.slice(remote.indexOf("/") + 1);
}

// 已有本地同名分支的远程分支不重复展示(本地分支优先)
const remoteOnly = computed(() => {
  const local = new Set(branches.value.local);
  return branches.value.remote.filter((r) => !local.has(remoteShortName(r)));
});

async function loadBranches() {
  loading.value = true;
  try {
    branches.value = await store.listBranches(props.project);
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

// 每次展开菜单时拉取最新分支列表
watch(open, (v) => {
  if (v) {
    loadBranches();
  }
});

// 打开新建对话框时确保分支列表可用,基点默认当前分支
watch(createOpen, (v) => {
  if (!v) {
    return;
  }
  baseBranch.value = props.project.git?.branch ?? "";
  if (!branches.value.local.length && !branches.value.remote.length && !loading.value) {
    loadBranches();
  }
});

async function switchTo(branch: string, remote = false) {
  if (switching.value || (!remote && branch === props.project.git?.branch)) return;
  switching.value = true;
  try {
    await store.checkoutBranch(props.project, branch, { remote });
    const shown = remote ? remoteShortName(branch) : branch;
    toast.success(t("git.branch.switched", { name: shown }));
  } catch (e) {
    toast.error(String(e));
  } finally {
    switching.value = false;
  }
}

async function createBranch() {
  const name = newBranch.value.trim();
  if (!name || creating.value) return;
  creating.value = true;
  try {
    // 基点为当前分支时无需显式传递(等价于基于 HEAD 创建)
    const current = props.project.git?.branch;
    const startPoint =
      baseBranch.value && baseBranch.value !== current ? baseBranch.value : undefined;
    await store.checkoutBranch(props.project, name, { create: true, startPoint });
    toast.success(t("git.branch.switched", { name }));
    createOpen.value = false;
    newBranch.value = "";
  } catch (e) {
    toast.error(String(e));
  } finally {
    creating.value = false;
  }
}
</script>

<template>
  <DropdownMenu v-model:open="open">
    <DropdownMenuTrigger as-child>
      <slot />
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" class="max-h-72 w-56">
      <DropdownMenuItem v-if="loading" disabled class="gap-2 text-xs">
        <Loader2 class="h-3.5 w-3.5 animate-spin" />
        {{ t("common.loading") }}
      </DropdownMenuItem>
      <template v-else>
        <DropdownMenuLabel v-if="branches.local.length" class="text-xs">
          {{ t("git.branch.local") }}
        </DropdownMenuLabel>
        <DropdownMenuItem
          v-for="b in branches.local"
          :key="b"
          class="gap-2 text-xs"
          :disabled="switching"
          @click="switchTo(b)"
        >
          <Check v-if="b === project.git?.branch" class="h-3.5 w-3.5 shrink-0 text-primary" />
          <span v-else class="h-3.5 w-3.5 shrink-0" />
          <span class="truncate">{{ b }}</span>
        </DropdownMenuItem>
        <template v-if="remoteOnly.length">
          <DropdownMenuSeparator />
          <DropdownMenuLabel class="text-xs">{{ t("git.branch.remote") }}</DropdownMenuLabel>
          <DropdownMenuItem
            v-for="r in remoteOnly"
            :key="r"
            class="gap-2 text-xs"
            :disabled="switching"
            @click="switchTo(r, true)"
          >
            <Globe class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span class="truncate">{{ r }}</span>
          </DropdownMenuItem>
        </template>
      </template>
      <DropdownMenuSeparator />
      <DropdownMenuItem class="gap-2 text-xs" @click="createOpen = true">
        <GitBranchPlus class="h-3.5 w-3.5" />
        {{ t("git.branch.newBranch") }}
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  <Dialog v-model:open="createOpen">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.branch.createTitle") }}</DialogTitle>
      </DialogHeader>
      <form class="flex flex-col gap-4" @submit.prevent="createBranch">
        <Input v-model="newBranch" :placeholder="t('git.branch.createPlaceholder')" autofocus />
        <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
          {{ t("git.branch.createBaseLabel") }}
          <Select v-model="baseBranch">
            <SelectTrigger class="w-full">
              <SelectValue :placeholder="t('git.branch.createBaseLabel')" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
                <SelectItem v-for="b in branches.local" :key="b" :value="b">
                  {{ b }}
                </SelectItem>
              </SelectGroup>
              <SelectGroup v-if="remoteOnly.length">
                <SelectLabel>{{ t("git.branch.remote") }}</SelectLabel>
                <SelectItem v-for="r in remoteOnly" :key="r" :value="r">
                  {{ r }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </label>
        <DialogFooter>
          <Button type="submit" :disabled="!newBranch.trim() || creating">
            {{ creating ? t("git.branch.creating") : t("common.create") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
