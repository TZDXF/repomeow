<script setup lang="ts">
import { computed, nextTick, ref, watch, onBeforeUnmount } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { openPath } from "@tauri-apps/plugin-opener";
import { ExternalLink, FileCode, LoaderCircle, Pencil, Save, Undo2, X } from "@lucide/vue";
import { Markdown } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import CodeViewer from "@/components/files/CodeViewer.vue";
import { cmd } from "@/lib/tauri";
import { joinPath } from "@/lib/path";
import { FILE_REF_CLASS, linkifyFileRefs, resolveRefPath } from "@/lib/ai-file-refs";
import { useSettingsStore } from "@/stores/settings";
import type { FilePreview } from "@/types";

/**
 * AI 面板的文件右侧抽屉:经 read_file_preview 读取项目内文件,
 * Markdown 默认渲染态(可切编辑),其余文件代码态;编辑经 CodeViewer
 * editable 模式 + save_text_file 落盘,Esc/遮罩关闭(编辑中有改动时 Esc 不生效)。
 * 渲染态正文里的 `@path/to/file` 引用会被 linkify 成按钮,点击经 navigate
 * 事件让父组件把抽屉切到被引用文件(解析相对当前文件所在目录)。
 */
const props = defineProps<{
  /** 项目根目录(read_file_preview 的 root,越界访问由后端拒绝) */
  root: string;
  /** 仓库内相对路径;null 表示关闭 */
  relPath: string | null;
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "saved"): void;
  /** 点击正文 @ 引用请求跳转;跳转失败时也会回发原路径让父组件恢复 */
  (e: "navigate", relPath: string): void;
}>();

const { t } = useI18n();
const settingsStore = useSettingsStore();

const preview = ref<FilePreview | null>(null);
const loading = ref(false);
const editing = ref(false);
const saving = ref(false);
/** 取消编辑时 +1,强制 CodeViewer 重挂载以丢弃未保存改动 */
const resetSeq = ref(0);
const viewer = ref<InstanceType<typeof CodeViewer> | null>(null);
let loadSeq = 0;

const open = computed(() => props.relPath !== null);
const isMarkdown = computed(() => /\.(md|mdc|markdown)$/i.test(props.relPath ?? ""));
/** 编辑中且内容有改动(用于 Esc 守卫与取消按钮) */
function isDirty(): boolean {
  if (!editing.value || !preview.value) return false;
  return viewer.value?.getText() !== (preview.value.text ?? "");
}

watch(
  () => props.relPath,
  async (rel, prevRel) => {
    editing.value = false;
    if (!rel) {
      preview.value = null;
      return;
    }
    const seq = ++loadSeq;
    loading.value = true;
    try {
      const result = await cmd<FilePreview>("read_file_preview", {
        root: props.root,
        relPath: rel,
      });
      if (seq === loadSeq) preview.value = result;
    } catch (e) {
      if (seq !== loadSeq) return;
      toast.error(String(e));
      // @ 引用跳转失败(文件不存在等):回到之前的文件,不直接关抽屉;
      // 初次打开就失败(此时还没有任何已加载内容)才关闭
      if (prevRel && preview.value) {
        emit("navigate", prevRel);
      } else {
        preview.value = null;
        emit("close");
      }
    } finally {
      if (seq === loadSeq) loading.value = false;
    }
  },
);

function requestClose() {
  if (saving.value) return;
  // 有未保存改动时 Esc/遮罩不静默丢弃,由用户点「取消编辑」显式放弃
  if (isDirty()) return;
  editing.value = false;
  emit("close");
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape" && open.value) {
    e.stopPropagation();
    requestClose();
  }
}
watch(
  open,
  (v) => {
    // capture:先于 CodeViewer 内的编辑器键处理拿到 Esc
    if (v) window.addEventListener("keydown", onKeydown, true);
    else window.removeEventListener("keydown", onKeydown, true);
  },
  { immediate: true },
);
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown, true));

function startEdit() {
  editing.value = true;
}

function cancelEdit() {
  resetSeq.value += 1;
  editing.value = false;
}

async function save() {
  const rel = props.relPath;
  if (!rel || !preview.value || saving.value) return;
  const text = viewer.value?.getText() ?? preview.value.text ?? "";
  if (text === (preview.value.text ?? "")) {
    editing.value = false;
    return;
  }
  saving.value = true;
  try {
    await cmd<void>("save_text_file", { path: joinPath(props.root, rel), content: text });
    preview.value = { ...preview.value, text };
    editing.value = false;
    emit("saved");
    toast.success(t("aiAssets.drawer.saved"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}

async function openExternal() {
  if (!props.relPath) return;
  try {
    await openPath(joinPath(props.root, props.relPath));
  } catch (e) {
    toast.error(String(e));
  }
}

// ── 正文 @ 文件引用 linkify(渲染态 Markdown 专属) ──────────────────────────
const mdBody = ref<HTMLElement | null>(null);

// Markdown 组件渲染完成后(link 语法已由库处理)再扫纯文本节点插入引用按钮;
// 编辑态切换/保存后内容重渲染,同样需要重扫
watch(
  () => [preview.value?.text, editing.value] as const,
  async () => {
    await nextTick();
    if (mdBody.value) linkifyFileRefs(mdBody.value);
  },
);

function onMarkdownClick(e: MouseEvent) {
  const btn = (e.target as HTMLElement).closest(`button.${FILE_REF_CLASS}`);
  if (!btn || !props.relPath) return;
  const ref = (btn as HTMLElement).dataset.ref;
  if (!ref) return;
  emit("navigate", resolveRefPath(props.relPath, ref));
}
</script>

<template>
  <Teleport to="body">
    <Transition name="ai-drawer">
      <div v-if="open" class="fixed inset-0 z-40" @click="requestClose">
        <div class="absolute inset-0 bg-black/40" />
        <!--
          抽屉是 teleport 到 body 的 fixed 层,顶部须避让 TitleBar(h-9, z-[60] 在其上),
          否则抽屉头部会被标题栏盖住
        -->
        <aside
          class="absolute bottom-0 right-0 top-9 flex w-[560px] max-w-[92vw] flex-col border-l border-t bg-background shadow-xl"
          @click.stop
        >
          <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
            <FileCode class="h-4 w-4 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="relPath ?? ''">
              {{ relPath }}
            </span>
            <Button
              size="sm"
              variant="ghost"
              :title="t('aiAssets.drawer.externalOpen')"
              @click="openExternal"
            >
              <ExternalLink class="h-4 w-4" />
            </Button>
            <Button
              v-if="!editing"
              size="sm"
              variant="outline"
              :disabled="!preview || preview.text === null"
              @click="startEdit"
            >
              <Pencil class="h-3.5 w-3.5" />
              {{ t("aiAssets.drawer.edit") }}
            </Button>
            <template v-else>
              <Button size="sm" variant="ghost" :disabled="saving" @click="cancelEdit">
                <Undo2 class="h-3.5 w-3.5" />
                {{ t("common.cancel") }}
              </Button>
              <Button size="sm" :disabled="saving" @click="save">
                <LoaderCircle v-if="saving" class="h-3.5 w-3.5 animate-spin" />
                <Save v-else class="h-3.5 w-3.5" />
                {{ saving ? t("common.saving") : t("common.save") }}
              </Button>
            </template>
            <Button size="sm" variant="ghost" :title="t('common.close')" @click="requestClose">
              <X class="h-4 w-4" />
            </Button>
          </header>

          <div v-if="loading" class="flex flex-1 items-center justify-center">
            <LoaderCircle class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
          <p
            v-else-if="!preview || preview.text === null"
            class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
          >
            {{ t("files.binary") }}
          </p>
          <!-- 编辑态一律走 CodeViewer(editable);非编辑态 Markdown 渲染、其余代码只读 -->
          <div v-else-if="editing || !isMarkdown" class="flex min-h-0 flex-1 flex-col">
            <p
              v-if="preview.truncated"
              class="shrink-0 border-b px-4 py-1 text-xs text-muted-foreground"
            >
              {{ t("files.truncated") }}
            </p>
            <CodeViewer
              :key="`${relPath}:${resetSeq}`"
              ref="viewer"
              :text="preview.text"
              :path="relPath ?? ''"
              :wrap="false"
              :editable="editing"
            />
          </div>
          <div
            v-else
            ref="mdBody"
            class="min-h-0 flex-1 overflow-y-auto px-6 py-4"
            @click="onMarkdownClick"
          >
            <Markdown mode="static" :content="preview.text" :locale="settingsStore.language" />
          </div>
        </aside>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.ai-drawer-enter-active,
.ai-drawer-leave-active {
  transition: opacity 0.15s ease;
}
.ai-drawer-enter-active aside,
.ai-drawer-leave-active aside {
  transition: transform 0.15s ease;
}
.ai-drawer-enter-from,
.ai-drawer-leave-to {
  opacity: 0;
}
.ai-drawer-enter-from aside,
.ai-drawer-leave-to aside {
  transform: translateX(100%);
}
</style>
