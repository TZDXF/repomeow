<script setup lang="ts">
import { provide, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { X } from "@lucide/vue";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import MdImage from "@/components/markdown/MdImage.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";
import { cmd } from "@/lib/tauri";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { useSettingsStore } from "@/stores/settings";
import type { Project, ReadmeContent } from "@/types";

const { t } = useI18n();
const settingsStore = useSettingsStore();

const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { default: false });

const readme = ref<ReadmeContent | null>(null);
const content = ref("");
const loading = ref(false);

// 相对路径图片/文件的解析基准(供自定义渲染器使用)
provide(MD_BASE_PATH_KEY, () => props.project.path);

// 自定义渲染器:图片走本地 asset 协议,链接输出真实 href 由外层统一拦截
const nodeRenderers: NodeRenderers = {
  image: MdImage,
  link: MdLink,
};

// 表格复制/导出(CSV/TSV/Markdown)/全屏 + 代码复制/折叠,库默认全开,这里显式声明
const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    // 自定义:替换 download 按钮,让 save dialog 里选什么格式就生成什么内容
    // (库内置下拉的"已选格式"与 save dialog 的扩展名会不同步)
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};

// 覆盖库默认的 <a download> 实现(代码块用):在 Tauri WebView2 下经常静默失败且无反馈,
// 改为弹原生 save dialog + 由 save_text_file 命令写入,失败/取消有 toast。
// 表格由 controls.table.customize 完全接管,这里只处理 code / mermaid。
// 见 src/lib/markdown-download.ts
const beforeDownload = createBeforeDownload(t);

// 阻止库把宿主元素上的 shadcn 变量内联到组件根节点(island 皮肤的 hex 色值
// 会被库误包成 hsl(#xxx) 非法值),MD 主题完全交给 CSS 层(src/styles/markdown/)
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

async function load() {
  loading.value = true;
  try {
    readme.value = await cmd<ReadmeContent | null>("read_readme", {
      path: props.project.path,
    });
    content.value = readme.value?.content ?? "";
  } catch {
    readme.value = null;
    content.value = "";
  } finally {
    loading.value = false;
  }
}

// 打开抽屉或切换项目时(重新)加载
watch(
  [open, () => props.project.id],
  ([isOpen]) => {
    if (isOpen) load();
  },
  { immediate: true },
);

// Esc 关闭
watch(open, (isOpen) => {
  if (isOpen) window.addEventListener("keydown", onEsc);
  else window.removeEventListener("keydown", onEsc);
});

function onEsc(e: KeyboardEvent) {
  if (e.key === "Escape") open.value = false;
}

/** 拦截链接点击:外链交给系统浏览器,相对路径用系统默认程序打开 */
async function onBodyClick(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) return;
  const href = a.getAttribute("href");
  e.preventDefault();
  if (!href || href.startsWith("#")) return;
  try {
    if (hasScheme(href)) {
      await openUrl(href);
    } else {
      await openPath(resolvePath(props.project.path, href));
    }
  } catch {
    // 目标不存在等情况静默忽略
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="open" class="fixed inset-0 z-40 bg-black/50" @click="open = false" />
    </Transition>

    <Transition name="slide">
      <aside
        v-if="open"
        class="readme-surface fixed inset-y-0 right-0 z-50 flex w-full max-w-2xl flex-col border-l shadow-xl"
      >
        <Button
          size="icon"
          variant="secondary"
          class="absolute right-3 top-3 z-10 h-8 w-8 shadow"
          :title="t('readme.closeEsc')"
          @click="open = false"
        >
          <X class="h-4 w-4" />
        </Button>

        <ScrollArea class="min-h-0 flex-1">
          <p v-if="loading" class="p-6 text-sm text-muted-foreground">{{ t("readme.loading") }}</p>
          <p v-else-if="!readme" class="p-6 text-sm text-muted-foreground">
            {{ t("readme.notFound") }}
          </p>
          <div v-else class="p-6 pt-12 text-sm" @click="onBodyClick">
            <Markdown
              mode="static"
              :content="content"
              :controls="controls"
              :node-renderers="nodeRenderers"
              :theme-element="themeElement"
              :locale="settingsStore.language"
              :before-download="beforeDownload"
            />
          </div>
        </ScrollArea>
      </aside>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
.slide-enter-active,
.slide-leave-active {
  transition: transform 0.25s ease;
}
.slide-enter-from,
.slide-leave-to {
  transform: translateX(100%);
}
</style>
