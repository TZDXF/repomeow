<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "vue-sonner";
import { Check, ChevronDown, FolderOpen, FolderGit2, KeyRound, Loader2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  getGhCliAccount,
  listAccountRepos,
  listGitAccounts,
  listProjectRemoteUrls,
  normalizeRemoteUrl,
  type GitAccount,
  type RemoteRepo,
} from "@/lib/accounts";
import { formatIsoDate } from "@/lib/format";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import { baseName, cleanPath } from "@/lib/path";

const { t } = useI18n();
const store = useProjectsStore();
const settingsStore = useSettingsStore();
const router = useRouter();

// 上一次添加项目用的存放位置,存 localStorage,作为下次的默认值
const LAST_LOCATION_KEY = "repomeow:last-add-location";

function loadLastLocation(): string {
  return localStorage.getItem(LAST_LOCATION_KEY) ?? "";
}

function saveLastLocation(dir: string) {
  if (dir) localStorage.setItem(LAST_LOCATION_KEY, dir);
}

/** 取路径的父目录(本地目录模式下项目路径本身即选中文件夹,记录其父目录作为存放位置) */
function parentPathOf(p: string): string {
  const trimmed = cleanPath(p);
  const idx = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  return idx > 0 ? trimmed.slice(0, idx) : "";
}

const visible = ref(false);
const mode = ref<"local" | "clone" | "account">("local");

// 本地目录模式
const path = ref("");
const name = ref("");
const submitting = ref(false);

// 克隆仓库模式
const url = ref("");
const parentDir = ref("");
const dirName = ref("");
const cloneName = ref("");
const dirNameTouched = ref(false);
const cloneNameTouched = ref(false);
const cloning = ref(false);
const cancelling = ref(false);
let cloneJobId = "";
// 从「账号仓库」入口克隆时记录账号,后端用其 token 克隆;手动切模式时清空
let cloneAccountId: number | undefined;
// 从「账号仓库」入口克隆时带入的远程仓库简介,作为项目描述;手动切模式时清空
const cloneDescription = ref("");

// 账号仓库模式
const accounts = ref<GitAccount[]>([]);
const accountsLoading = ref(false);
const accountsLoaded = ref(false);
const selectedAccountId = ref<number | null>(null);
const repos = ref<RemoteRepo[]>([]);
const reposLoading = ref(false);
const repoSearch = ref("");
const selectedOwner = ref("");
const ownerPickerOpen = ref(false);
const ownerSearch = ref("");
const ownerSearchInput = ref<{ $el: HTMLInputElement } | null>(null);
const addedRemotes = ref<Set<string>>(new Set());

/** 当前账号下仓库涉及的组织/用户(去重,按名称排序) */
const ownerOptions = computed(() => {
  const owners = new Set(repos.value.map((r) => r.owner).filter(Boolean));
  return [...owners].sort((a, b) => a.localeCompare(b));
});

/** 组织超过该数量时,组织下拉内显示搜索框 */
const OWNER_SEARCH_THRESHOLD = 3;
const showOwnerSearch = computed(() => ownerOptions.value.length > OWNER_SEARCH_THRESHOLD);

/** 按搜索词过滤后的组织选项 */
const filteredOwnerOptions = computed(() => {
  const q = ownerSearch.value.trim().toLowerCase();
  if (!q) return ownerOptions.value;
  return ownerOptions.value.filter((o) => o.toLowerCase().includes(q));
});

const filteredRepos = computed(() => {
  let list = repos.value;
  if (selectedOwner.value) {
    list = list.filter((r) => r.owner === selectedOwner.value);
  }
  // 空格切分为多个查询词,词间 AND:每个词至少命中组织名/全名/仓库名/描述之一
  const terms = repoSearch.value.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return list;
  return list.filter((r) => {
    const fields = [r.owner, r.fullName, r.name, r.description].map((s) => s.toLowerCase());
    return terms.every((q) => fields.some((f) => f.includes(q)));
  });
});

/** 首次进入账号模式时加载账号列表与本地项目 remote 地址(「已添加」匹配用) */
async function ensureAccountsLoaded() {
  if (accountsLoaded.value || accountsLoading.value) return;
  accountsLoading.value = true;
  try {
    const [accs, remoteUrls] = await Promise.all([listGitAccounts(), listProjectRemoteUrls()]);
    // 设置页开启 GitHub CLI 集成时才探测 gh;探测失败静默降级(下拉不显示该项)
    const ghAccount = settingsStore.enableGhCli ? await getGhCliAccount().catch(() => null) : null;
    accounts.value = ghAccount ? [...accs, ghAccount] : accs;
    addedRemotes.value = new Set(remoteUrls.map(normalizeRemoteUrl));
    if (accounts.value.length > 0 && selectedAccountId.value == null) {
      selectedAccountId.value = accounts.value[0].id;
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    accountsLoading.value = false;
    accountsLoaded.value = true;
  }
}

// 仓库加载代际计数:切换账号时递增,过期请求的结果直接丢弃(相当于取消)
let reposLoadSeq = 0;

/** 一次加载账号下全部仓库(后端循环分页拉全,前端只做客户端搜索过滤) */
async function loadRepos() {
  const id = selectedAccountId.value;
  if (id == null) return;
  const seq = ++reposLoadSeq;
  reposLoading.value = true;
  try {
    const list = await listAccountRepos(id);
    // 期间已切换到其他账号:结果过期,丢弃
    if (seq === reposLoadSeq) repos.value = list;
  } catch (e) {
    if (seq === reposLoadSeq) toast.error(String(e));
  } finally {
    if (seq === reposLoadSeq) reposLoading.value = false;
  }
}

watch(selectedAccountId, (id) => {
  repos.value = [];
  repoSearch.value = "";
  selectedOwner.value = "";
  ownerPickerOpen.value = false;
  if (id != null) loadRepos();
});

/** 选中组织(空串为「全部组织」)后收起下拉 */
function pickOwner(owner: string) {
  selectedOwner.value = owner;
  ownerPickerOpen.value = false;
}

/** 组织下拉打开时清空上次搜索,并把焦点交给搜索框(超过阈值才有搜索框) */
function focusOwnerSearch(e: Event) {
  ownerSearch.value = "";
  if (!showOwnerSearch.value) return;
  e.preventDefault();
  ownerSearchInput.value?.$el?.focus();
}

/** 仓库是否已添加为本地项目(remote URL 归一化后匹配) */
function isAdded(repo: RemoteRepo): boolean {
  return addedRemotes.value.has(normalizeRemoteUrl(repo.httpCloneUrl));
}

/** 选中仓库:回填克隆表单并切到克隆模式,克隆时带上账号凭据 */
function pickRepo(repo: RemoteRepo) {
  if (isAdded(repo) || !repo.httpCloneUrl) return;
  cloneAccountId = selectedAccountId.value ?? undefined;
  cloneDescription.value = repo.description;
  dirNameTouched.value = false;
  cloneNameTouched.value = false;
  url.value = repo.httpCloneUrl;
  mode.value = "clone";
}

/** 从仓库 URL 推导目录名:取末段并去掉 .git 后缀 */
function dirNameFromUrl(raw: string): string {
  return baseName(cleanPath(raw)).replace(/\.git$/i, "");
}

watch(url, (value) => {
  const derived = dirNameFromUrl(value);
  if (!dirNameTouched.value) dirName.value = derived;
});

watch(dirName, (value) => {
  if (!cloneNameTouched.value) cloneName.value = value;
});

/** 存放位置与目录名拼出的完整目标路径(分隔符跟随存放位置的写法) */
const targetPath = computed(() => {
  if (!parentDir.value || !dirName.value.trim()) return "";
  const sep = parentDir.value.includes("\\") ? "\\" : "/";
  return parentDir.value.replace(/[\\/]+$/, "") + sep + dirName.value.trim();
});

const cloneReady = computed(
  () => url.value.trim() && parentDir.value && dirName.value.trim() && cloneName.value.trim(),
);

async function pickFolder() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: t("projects.add.dialogTitle"),
    defaultPath: loadLastLocation() || undefined,
  });
  if (typeof selected === "string") {
    path.value = selected;
    if (!name.value) {
      name.value = baseName(selected);
    }
  }
}

async function pickParentDir() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: t("projects.add.locationDialogTitle"),
    defaultPath: parentDir.value || loadLastLocation() || undefined,
  });
  if (typeof selected === "string") {
    parentDir.value = selected;
  }
}

async function submit() {
  if (!path.value || !name.value.trim() || submitting.value) return;
  submitting.value = true;
  try {
    const project = await store.addProject(path.value, name.value.trim());
    saveLastLocation(parentPathOf(path.value));
    toast.success(t("projects.add.added", { name: project.name }));
    visible.value = false;
    path.value = "";
    name.value = "";
    router.push(`/projects/${project.id}`);
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function submitClone() {
  if (!cloneReady.value || cloning.value) return;
  cloning.value = true;
  cancelling.value = false;
  cloneJobId = crypto.randomUUID();
  try {
    const clonedPath = await store.cloneProject(
      url.value.trim(),
      targetPath.value,
      cloneJobId,
      cloneAccountId,
    );
    const project = await store.addProject(
      clonedPath,
      cloneName.value.trim(),
      cloneDescription.value,
    );
    toast.success(t("projects.add.cloned", { name: project.name }));
    saveLastLocation(parentDir.value);
    visible.value = false;
    url.value = "";
    parentDir.value = "";
    dirName.value = "";
    cloneName.value = "";
    dirNameTouched.value = false;
    cloneNameTouched.value = false;
    cloneAccountId = undefined;
    cloneDescription.value = "";
    router.push(`/projects/${project.id}`);
  } catch (e) {
    // 用户主动取消:静默复位,不弹错误
    if (!cancelling.value) toast.error(String(e));
  } finally {
    cloning.value = false;
    cloneJobId = "";
  }
}

function cancelClone() {
  if (!cloning.value || cancelling.value) return;
  cancelling.value = true;
  store.cancelClone(cloneJobId).catch(() => {});
}

// 克隆过程中关闭弹窗(ESC/点 X/点遮罩)视为取消克隆;打开时回填上次的存放位置
watch(visible, (open_) => {
  if (!open_ && cloning.value) cancelClone();
  if (open_ && !parentDir.value) parentDir.value = loadLastLocation();
});

/** 顶部模式切换;手动切出克隆模式时丢弃「账号仓库」带入的凭据与简介 */
function switchMode(m: "local" | "clone" | "account") {
  mode.value = m;
  if (m !== "clone") {
    cloneAccountId = undefined;
    cloneDescription.value = "";
  }
  if (m === "account") ensureAccountsLoaded();
}
</script>

<template>
  <Dialog v-model:open="visible">
    <DialogTrigger as-child>
      <slot />
    </DialogTrigger>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("projects.add.title") }}</DialogTitle>
        <DialogDescription>
          {{
            mode === "local"
              ? t("projects.add.description")
              : mode === "clone"
                ? t("projects.add.cloneDescription")
                : t("projects.add.accountDescription")
          }}
        </DialogDescription>
      </DialogHeader>

      <div class="flex gap-1 rounded-md border p-1">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="h-7 flex-1 gap-1.5"
          :class="mode === 'local' && 'bg-accent'"
          :disabled="cloning"
          @click="switchMode('local')"
        >
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("projects.add.modeLocal") }}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="h-7 flex-1 gap-1.5"
          :class="mode === 'clone' && 'bg-accent'"
          :disabled="cloning"
          @click="switchMode('clone')"
        >
          <FolderGit2 class="h-3.5 w-3.5" />
          {{ t("projects.add.modeClone") }}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="h-7 flex-1 gap-1.5"
          :class="mode === 'account' && 'bg-accent'"
          :disabled="cloning"
          @click="switchMode('account')"
        >
          <KeyRound class="h-3.5 w-3.5" />
          {{ t("projects.add.modeAccount") }}
        </Button>
      </div>

      <form v-if="mode === 'local'" class="flex flex-col gap-4" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.pathLabel") }}</label>
          <div class="flex gap-2">
            <Input
              v-model="path"
              :placeholder="t('projects.add.pathPlaceholder')"
              readonly
              class="flex-1"
            />
            <Button type="button" variant="outline" @click="pickFolder">
              <FolderOpen class="h-4 w-4" />
              {{ t("projects.add.browse") }}
            </Button>
          </div>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.nameLabel") }}</label>
          <Input v-model="name" :placeholder="t('projects.add.namePlaceholder')" autofocus />
        </div>
        <DialogFooter>
          <Button type="submit" :disabled="!path || !name.trim() || submitting">
            {{ submitting ? t("common.adding") : t("common.add") }}
          </Button>
        </DialogFooter>
      </form>

      <div v-else-if="mode === 'account'" class="flex min-w-0 flex-col gap-3">
        <div
          v-if="accountsLoading"
          class="flex items-center justify-center gap-2 rounded-md border border-dashed px-3 py-6 text-sm text-muted-foreground"
        >
          <Loader2 class="h-4 w-4 animate-spin" />
          {{ t("projects.add.accountLoading") }}
        </div>
        <p
          v-else-if="accountsLoaded && accounts.length === 0"
          class="rounded-md border border-dashed px-3 py-6 text-center text-sm text-muted-foreground"
        >
          {{ t("projects.add.accountEmpty") }}
        </p>
        <template v-else>
          <div class="flex items-center gap-2">
            <label class="shrink-0 text-sm font-medium">{{ t("projects.add.accountLabel") }}</label>
            <DropdownMenu>
              <DropdownMenuTrigger as-child>
                <Button
                  variant="outline"
                  class="h-8 flex-1 min-w-0 justify-between gap-2 font-normal"
                  :disabled="accounts.length === 0"
                >
                  <span class="truncate">
                    {{
                      accounts.find((a) => a.id === selectedAccountId)?.label ||
                      accounts.find((a) => a.id === selectedAccountId)?.username ||
                      accounts.find((a) => a.id === selectedAccountId)?.provider ||
                      t("projects.add.accountLabel")
                    }}
                  </span>
                  <ChevronDown class="h-4 w-4 shrink-0 opacity-60" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                class="w-[var(--reka-dropdown-menu-trigger-width)]"
              >
                <DropdownMenuRadioGroup v-model="selectedAccountId">
                  <DropdownMenuRadioItem v-for="a in accounts" :key="a.id" :value="a.id">
                    {{ a.label || a.username || a.provider }}
                    <template v-if="a.username">(@{{ a.username }})</template>
                  </DropdownMenuRadioItem>
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
          <div class="flex gap-2">
            <Input
              v-model="repoSearch"
              :placeholder="t('projects.add.repoSearchPlaceholder')"
              spellcheck="false"
              class="min-w-0 flex-1"
            />
            <Popover v-if="ownerOptions.length > 1" v-model:open="ownerPickerOpen">
              <PopoverTrigger as-child>
                <Button
                  variant="outline"
                  class="h-8 w-32 shrink-0 justify-between gap-2 font-normal"
                >
                  <span class="truncate">
                    {{ selectedOwner || t("projects.add.repoOwnerAll") }}
                  </span>
                  <ChevronDown class="h-4 w-4 shrink-0 opacity-60" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                align="end"
                class="w-56 min-w-[var(--reka-popover-trigger-width)] gap-1 p-1"
                @open-auto-focus="focusOwnerSearch"
              >
                <Input
                  v-if="showOwnerSearch"
                  ref="ownerSearchInput"
                  v-model="ownerSearch"
                  :placeholder="t('projects.add.ownerSearchPlaceholder')"
                  spellcheck="false"
                  class="mb-1"
                />
                <div class="max-h-56 overflow-y-auto">
                  <button
                    type="button"
                    class="flex w-full items-center gap-2 rounded-sm px-1.5 py-1.5 text-left text-sm hover:bg-accent"
                    @click="pickOwner('')"
                  >
                    <Check class="h-3.5 w-3.5 shrink-0" :class="selectedOwner && 'opacity-0'" />
                    <span class="truncate">{{ t("projects.add.repoOwnerAll") }}</span>
                  </button>
                  <button
                    v-for="owner in filteredOwnerOptions"
                    :key="owner"
                    type="button"
                    class="flex w-full items-center gap-2 rounded-sm px-1.5 py-1.5 text-left text-sm hover:bg-accent"
                    @click="pickOwner(owner)"
                  >
                    <Check
                      class="h-3.5 w-3.5 shrink-0"
                      :class="selectedOwner !== owner && 'opacity-0'"
                    />
                    <span class="truncate">{{ owner }}</span>
                  </button>
                  <p
                    v-if="filteredOwnerOptions.length === 0"
                    class="px-2 py-4 text-center text-sm text-muted-foreground"
                  >
                    {{ t("projects.add.ownerSearchEmpty") }}
                  </p>
                </div>
              </PopoverContent>
            </Popover>
          </div>
          <div class="max-h-64 overflow-x-hidden overflow-y-auto rounded-md border">
            <div
              v-if="reposLoading && repos.length === 0"
              class="flex items-center justify-center gap-2 py-8 text-sm text-muted-foreground"
            >
              <Loader2 class="h-4 w-4 animate-spin" />
              {{ t("projects.add.repoLoading") }}
            </div>
            <p
              v-else-if="filteredRepos.length === 0"
              class="py-6 text-center text-sm text-muted-foreground"
            >
              {{ t("projects.add.repoEmpty") }}
            </p>
            <button
              v-for="repo in filteredRepos"
              :key="repo.repoId"
              type="button"
              class="flex w-full flex-col gap-0.5 border-b px-3 py-2 text-left transition-colors last:border-b-0"
              :class="isAdded(repo) ? 'cursor-not-allowed opacity-60' : 'hover:bg-accent'"
              :disabled="isAdded(repo)"
              @click="pickRepo(repo)"
            >
              <div class="flex min-w-0 items-center gap-2">
                <span class="min-w-0 truncate text-sm">
                  <span v-if="repo.owner" class="text-muted-foreground">{{ repo.owner }} / </span>
                  <span class="font-medium">{{ repo.name }}</span>
                </span>
                <Badge v-if="repo.isPrivate" variant="outline" class="shrink-0 text-xs">
                  {{ t("projects.add.repoPrivate") }}
                </Badge>
                <Badge v-if="isAdded(repo)" variant="secondary" class="shrink-0 text-xs">
                  {{ t("projects.add.repoAdded") }}
                </Badge>
                <span class="ml-auto shrink-0 text-xs text-muted-foreground">
                  {{ formatIsoDate(repo.updatedAt) }}
                </span>
              </div>
              <p v-if="repo.description" class="truncate text-xs text-muted-foreground">
                {{ repo.description }}
              </p>
            </button>
          </div>
        </template>
      </div>

      <form v-else class="flex flex-col gap-4" @submit.prevent="submitClone">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.urlLabel") }}</label>
          <Input
            v-model="url"
            :placeholder="t('projects.add.urlPlaceholder')"
            :disabled="cloning"
            autofocus
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.locationLabel") }}</label>
          <div class="flex gap-2">
            <Input
              v-model="parentDir"
              :placeholder="t('projects.add.locationPlaceholder')"
              readonly
              class="flex-1"
            />
            <Button type="button" variant="outline" :disabled="cloning" @click="pickParentDir">
              <FolderOpen class="h-4 w-4" />
              {{ t("projects.add.browse") }}
            </Button>
          </div>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.dirNameLabel") }}</label>
          <Input
            v-model="dirName"
            :placeholder="t('projects.add.dirNamePlaceholder')"
            :disabled="cloning"
            @input="dirNameTouched = true"
          />
          <p v-if="targetPath" class="text-xs text-muted-foreground break-all">{{ targetPath }}</p>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("projects.add.nameLabel") }}</label>
          <Input
            v-model="cloneName"
            :placeholder="t('projects.add.namePlaceholder')"
            :disabled="cloning"
            @input="cloneNameTouched = true"
          />
        </div>
        <DialogFooter>
          <Button v-if="cloning" type="button" variant="outline" @click="cancelClone">
            {{ t("common.cancel") }}
          </Button>
          <Button type="submit" :disabled="!cloneReady || cloning">
            <Loader2 v-if="cloning" class="h-4 w-4 animate-spin" />
            {{ cloning ? t("projects.add.cloning") : t("projects.add.cloneAndAdd") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
