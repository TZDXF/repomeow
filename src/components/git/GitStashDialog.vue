<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useLocalStorage } from "@vueuse/core";
import { ArchiveRestore, FileDiff, Loader2, Plus, RefreshCw, Trash2 } from "@lucide/vue";
import DiffViewer from "@/components/git/DiffViewer.vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { formatLocalDateTime } from "@/lib/format";
import { statusClass } from "@/lib/git-status";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import type { GitCommitFile, GitCommitFileDiff, GitStash, GitStatus, Project } from "@/types";

const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
const { t } = useI18n();
const store = useProjectsStore();

const stashes = ref<GitStash[]>([]);
const loading = ref(false);
const loadError = ref("");
const confirmOpen = ref(false);
const createOpen = ref(false);
const diffOpen = ref(false);
const diffTarget = ref<GitStash | null>(null);
const stashFiles = ref<GitCommitFile[]>([]);
const stashFilesLoading = ref(false);
const stashFilesError = ref("");
const selectedFile = ref<GitCommitFile | null>(null);
const stashDiff = ref<GitCommitFileDiff | null>(null);
const stashDiffLoading = ref(false);
const stashDiffError = ref("");
const ignoreWs = useLocalStorage<"none" | "eol" | "change" | "all">(
  "repomeow:commit-diff-ignore-ws",
  "none",
);
let diffLoadSeq = 0;
const createMessage = ref("");
const includeUntracked = ref(false);
const creating = ref(false);
const pending = ref<{ kind: "pop" | "drop"; stash: GitStash } | null>(null);
const busyKey = ref("");

const pendingRef = computed(() => (pending.value ? `stash@{${pending.value.stash.index}}` : ""));
const trackedChanges = computed(
  () => (props.project.git?.staged ?? 0) + (props.project.git?.modified ?? 0),
);
const untrackedChanges = computed(() => props.project.git?.untracked ?? 0);
const canCreate = computed(
  () => trackedChanges.value > 0 || (includeUntracked.value && untrackedChanges.value > 0),
);
const onlyUntrackedExcluded = computed(
  () => trackedChanges.value === 0 && untrackedChanges.value > 0 && !includeUntracked.value,
);
const splitApplicable = computed(
  () => selectedFile.value?.status !== "A" && selectedFile.value?.status !== "D",
);

watch(open, (value) => {
  if (value) {
    void loadStashes();
  } else {
    confirmOpen.value = false;
    createOpen.value = false;
    diffOpen.value = false;
    pending.value = null;
  }
});

watch(ignoreWs, () => {
  if (diffOpen.value && selectedFile.value) {
    void loadSelectedDiff(selectedFile.value);
  }
});

async function loadStashes() {
  loading.value = true;
  loadError.value = "";
  try {
    stashes.value = await cmd<GitStash[]>("list_git_stashes", { path: props.project.path });
  } catch (error) {
    loadError.value = String(error);
  } finally {
    loading.value = false;
  }
}

function stashRef(stash: GitStash) {
  return `stash@{${stash.index}}`;
}

function shortOid(oid: string) {
  return oid.slice(0, 7);
}

async function openStashDiff(stash: GitStash) {
  diffTarget.value = stash;
  diffOpen.value = true;
  stashFiles.value = [];
  selectedFile.value = null;
  stashDiff.value = null;
  stashFilesError.value = "";
  stashDiffError.value = "";
  stashFilesLoading.value = true;
  try {
    const files = await cmd<GitCommitFile[]>("git_stash_files", {
      path: props.project.path,
      oid: stash.oid,
    });
    if (diffTarget.value?.oid !== stash.oid) {
      return;
    }
    stashFiles.value = files;
    if (files[0]) {
      await selectStashFile(files[0]);
    }
  } catch (error) {
    stashFilesError.value = String(error);
  } finally {
    if (diffTarget.value?.oid === stash.oid) {
      stashFilesLoading.value = false;
    }
  }
}

async function selectStashFile(file: GitCommitFile) {
  selectedFile.value = file;
  await loadSelectedDiff(file);
}

async function loadSelectedDiff(file: GitCommitFile) {
  const target = diffTarget.value;
  if (!target) {
    return;
  }
  const seq = ++diffLoadSeq;
  stashDiff.value = null;
  stashDiffError.value = "";
  stashDiffLoading.value = true;
  try {
    const result = await cmd<GitCommitFileDiff>("git_stash_file_diff", {
      path: props.project.path,
      oid: target.oid,
      filePath: file.path,
      oldPath: file.old_path,
      ignoreWs: ignoreWs.value,
    });
    if (seq === diffLoadSeq) {
      stashDiff.value = result;
    }
  } catch (error) {
    if (seq === diffLoadSeq) {
      stashDiffError.value = String(error);
    }
  } finally {
    if (seq === diffLoadSeq) {
      stashDiffLoading.value = false;
    }
  }
}

function openCreateDialog() {
  createMessage.value = "";
  includeUntracked.value = false;
  createOpen.value = true;
}

async function createStash() {
  if (!canCreate.value || creating.value) {
    return;
  }

  creating.value = true;
  try {
    const status = await cmd<GitStatus>("git_stash_push", {
      path: props.project.path,
      message: createMessage.value.trim(),
      includeUntracked: includeUntracked.value,
    });
    props.project.git = status;
    createOpen.value = false;
    toast.success(t("git.stash.created"));
    await loadStashes();
  } catch (error) {
    toast.error(String(error));
    await store.refreshGitStatus(props.project, { force: true }).catch(() => {});
  } finally {
    creating.value = false;
  }
}

function requestAction(kind: "pop" | "drop", stash: GitStash) {
  pending.value = { kind, stash };
  confirmOpen.value = true;
}

async function confirmAction() {
  const action = pending.value;
  if (!action || busyKey.value) {
    return;
  }

  confirmOpen.value = false;
  pending.value = null;
  busyKey.value = `${action.kind}:${action.stash.oid}`;
  const refName = stashRef(action.stash);
  try {
    const status = await cmd<GitStatus>(
      action.kind === "pop" ? "git_stash_pop" : "git_stash_drop",
      {
        path: props.project.path,
        index: action.stash.index,
        oid: action.stash.oid,
      },
    );
    props.project.git = status;
    toast.success(
      t(action.kind === "pop" ? "git.stash.popped" : "git.stash.dropped", { ref: refName }),
    );
    await loadStashes();
  } catch (error) {
    toast.error(String(error));
    if (action.kind === "pop") {
      await store.refreshGitStatus(props.project, { force: true }).catch(() => {});
    }
    await loadStashes();
  } finally {
    busyKey.value = "";
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="gap-0 overflow-hidden p-0 sm:max-w-2xl">
      <DialogHeader class="border-b px-5 py-4 pr-14">
        <div class="flex items-center justify-between gap-4">
          <DialogTitle>{{ t("git.stash.title") }}</DialogTitle>
          <div class="flex shrink-0 items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              :disabled="loading || !!busyKey || creating"
              @click="openCreateDialog"
            >
              <Plus class="h-3.5 w-3.5" />
              {{ t("git.stash.create") }}
            </Button>
          </div>
        </div>
      </DialogHeader>

      <div class="min-h-52 p-4">
        <div
          v-if="loading && !stashes.length"
          class="flex h-52 items-center justify-center rounded-lg border border-dashed bg-muted/15 text-sm text-muted-foreground"
        >
          <Loader2 class="mr-2 h-4 w-4 animate-spin" />
          {{ t("common.loading") }}
        </div>
        <div
          v-else-if="loadError"
          class="flex h-52 flex-col items-center justify-center gap-3 rounded-lg border border-dashed bg-muted/15 px-6 text-center text-sm text-muted-foreground"
        >
          <p>{{ t("git.stash.loadFailed") }}</p>
          <p class="max-w-full break-all text-xs">{{ loadError }}</p>
          <Button variant="outline" size="sm" @click="loadStashes">
            <RefreshCw class="h-3.5 w-3.5" />
            {{ t("common.refresh") }}
          </Button>
        </div>
        <div
          v-else-if="!stashes.length"
          class="flex h-52 flex-col items-center justify-center gap-3 rounded-lg border border-dashed bg-muted/15 text-sm text-muted-foreground"
        >
          <span class="flex h-12 w-12 items-center justify-center rounded-full bg-muted">
            <ArchiveRestore class="h-6 w-6 opacity-55" />
          </span>
          {{ t("git.stash.empty") }}
        </div>
        <div v-else>
          <p class="px-1 pb-2 text-xs font-medium text-muted-foreground">
            {{ t("git.stash.count", { count: stashes.length }) }}
          </p>
          <ul class="max-h-[min(56vh,30rem)] divide-y overflow-y-auto rounded-lg border">
            <li
              v-for="stash in stashes"
              :key="stash.oid"
              class="flex items-center gap-3 px-3 py-3 transition-colors hover:bg-muted/25"
            >
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <code class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[11px]">
                    {{ stashRef(stash) }}
                  </code>
                  <p class="truncate text-sm font-medium" :title="stash.message">
                    {{ stash.message }}
                  </p>
                </div>
                <p class="mt-1 truncate text-xs text-muted-foreground">
                  <span v-if="stash.author">{{ stash.author }} · </span>
                  <span :title="formatLocalDateTime(stash.created_at)">
                    {{ formatLocalDateTime(stash.created_at) }}
                  </span>
                  · <span class="font-mono">{{ shortOid(stash.oid) }}</span>
                </p>
              </div>
              <div class="flex shrink-0 items-center gap-1.5">
                <Button
                  variant="ghost"
                  size="sm"
                  :disabled="!!busyKey || loading || creating"
                  @click="openStashDiff(stash)"
                >
                  <FileDiff class="h-3.5 w-3.5" />
                  {{ t("git.stash.diff") }}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="!!busyKey || loading || creating"
                  @click="requestAction('pop', stash)"
                >
                  <Loader2 v-if="busyKey === `pop:${stash.oid}`" class="h-3.5 w-3.5 animate-spin" />
                  <ArchiveRestore v-else class="h-3.5 w-3.5" />
                  {{ t(busyKey === `pop:${stash.oid}` ? "git.stash.popping" : "git.stash.pop") }}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="text-destructive hover:text-destructive"
                  :disabled="!!busyKey || loading || creating"
                  @click="requestAction('drop', stash)"
                >
                  <Loader2
                    v-if="busyKey === `drop:${stash.oid}`"
                    class="h-3.5 w-3.5 animate-spin"
                  />
                  <Trash2 v-else class="h-3.5 w-3.5" />
                  {{ t(busyKey === `drop:${stash.oid}` ? "git.stash.dropping" : "git.stash.drop") }}
                </Button>
              </div>
            </li>
          </ul>
        </div>
      </div>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="diffOpen">
    <DialogContent class="flex h-[min(82vh,52rem)] max-h-[52rem] flex-col sm:max-w-6xl">
      <DialogHeader class="shrink-0">
        <DialogTitle>
          {{ t("git.stash.diffTitle", { ref: diffTarget ? stashRef(diffTarget) : "" }) }}
        </DialogTitle>
        <DialogDescription class="truncate" :title="diffTarget?.message">
          {{ diffTarget?.message }}
        </DialogDescription>
      </DialogHeader>
      <div class="flex min-h-0 flex-1 overflow-hidden rounded-md border">
        <aside class="flex w-72 shrink-0 flex-col border-r bg-muted/10">
          <div class="border-b px-3 py-2 text-xs font-medium text-muted-foreground">
            {{ t("git.stash.diffFiles", { count: stashFiles.length }) }}
          </div>
          <div v-if="stashFilesLoading" class="flex min-h-0 flex-1 items-center justify-center">
            <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
          <p v-else-if="stashFilesError" class="p-3 text-xs text-destructive">
            {{ stashFilesError }}
          </p>
          <p v-else-if="!stashFiles.length" class="p-3 text-xs text-muted-foreground">
            {{ t("git.stash.diffEmpty") }}
          </p>
          <div v-else class="min-h-0 flex-1 overflow-y-auto p-1.5">
            <button
              v-for="file in stashFiles"
              :key="`${file.status}:${file.path}`"
              class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs transition-colors hover:bg-accent"
              :class="selectedFile?.path === file.path && 'bg-accent'"
              :title="file.path"
              @click="selectStashFile(file)"
            >
              <span class="w-3 shrink-0 font-mono font-semibold" :class="statusClass(file.status)">
                {{ file.status }}
              </span>
              <span class="min-w-0 flex-1 truncate">{{ file.path }}</span>
              <span v-if="file.additions" class="shrink-0 text-green-600 dark:text-green-400">
                +{{ file.additions }}
              </span>
              <span v-if="file.deletions" class="shrink-0 text-red-600 dark:text-red-400">
                -{{ file.deletions }}
              </span>
            </button>
          </div>
        </aside>
        <DiffViewer
          v-model:ignore-ws="ignoreWs"
          :diff="stashDiff"
          :file-path="selectedFile?.path ?? null"
          :loading="stashDiffLoading"
          :error="stashDiffError"
          :split-applicable="splitApplicable"
          :can-open-ide="false"
        />
      </div>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="createOpen">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.stash.createTitle") }}</DialogTitle>
      </DialogHeader>
      <form class="space-y-4" @submit.prevent="createStash">
        <div class="space-y-1.5">
          <label class="text-sm font-medium" for="stash-message">
            {{ t("git.stash.messageLabel") }}
          </label>
          <Input
            id="stash-message"
            v-model="createMessage"
            :placeholder="t('git.stash.messagePlaceholder')"
            autofocus
          />
        </div>
        <label class="flex cursor-pointer items-start gap-2.5 rounded-md border p-3">
          <input v-model="includeUntracked" type="checkbox" class="mt-0.5 h-4 w-4 accent-primary" />
          <span class="space-y-0.5">
            <span class="block text-sm font-medium">{{ t("git.stash.includeUntracked") }}</span>
            <span class="block text-xs text-muted-foreground">
              {{ t("git.stash.includeUntrackedHint") }}
            </span>
          </span>
        </label>
        <p v-if="!canCreate" class="text-xs text-amber-600 dark:text-amber-400">
          {{ t(onlyUntrackedExcluded ? "git.stash.untrackedOnly" : "git.stash.cleanWorktree") }}
        </p>
        <DialogFooter>
          <Button type="button" variant="outline" :disabled="creating" @click="createOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button type="submit" :disabled="!canCreate || creating">
            <Loader2 v-if="creating" class="h-3.5 w-3.5 animate-spin" />
            <Plus v-else class="h-3.5 w-3.5" />
            {{ t(creating ? "git.stash.creating" : "git.stash.create") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>

  <Dialog v-model:open="confirmOpen">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>
          {{ t(pending?.kind === "drop" ? "git.stash.dropTitle" : "git.stash.popTitle") }}
        </DialogTitle>
        <DialogDescription>
          {{
            t(pending?.kind === "drop" ? "git.stash.dropConfirm" : "git.stash.popConfirm", {
              ref: pendingRef,
            })
          }}
        </DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button variant="outline" @click="confirmOpen = false">{{ t("common.cancel") }}</Button>
        <Button
          :variant="pending?.kind === 'drop' ? 'destructive' : 'default'"
          @click="confirmAction"
        >
          {{ t(pending?.kind === "drop" ? "git.stash.drop" : "git.stash.pop") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
