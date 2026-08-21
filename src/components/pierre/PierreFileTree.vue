<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { FileTree, type FileTreeRowDecoration, type GitStatusEntry } from "@pierre/trees";

export interface PierreTreeEntry {
  /** 相对路径,'/' 分隔 */
  path: string;
  isDir: boolean;
}

/**
 * @pierre/trees 的 Vue 薄包装:实例生命周期由本组件持有,读写都走 model。
 * 目录路径按库约定补 trailing '/';文件点击经 onSelectionChange 过滤后抛 activate;
 * 内置搜索(search 模式 hide-non-matches)由 openSearch() 打开。
 */
const props = withDefaults(
  defineProps<{
    entries: PierreTreeEntry[];
    /** git 状态着色(added/modified/deleted/ignored 等) */
    gitStatus?: GitStatusEntry[];
    /** 行尾装饰(状态字母、+增/-删计数等),返回 null 的行无装饰 */
    decoration?: (path: string) => FileTreeRowDecoration | null;
    /** 外部驱动的选中文件(树选择同步回它) */
    selected?: string | null;
    initialExpansion?: "open" | "closed";
  }>(),
  { gitStatus: undefined, decoration: undefined, selected: null, initialExpansion: "closed" },
);

const emit = defineEmits<{
  /** 文件行被选中(目录展开/折叠不触发) */
  activate: [path: string];
}>();

const host = ref<HTMLElement | null>(null);

let tree: FileTree | null = null;
/** 当前条目中的文件路径集(选择事件过滤目录用) */
let filePaths = new Set<string>();
/** 程序化选择期间的回火抑制:select() 会触发 onSelectionChange,不能再抛 activate */
let syncingSelection = false;

function toPaths(entries: PierreTreeEntry[]): string[] {
  filePaths = new Set();
  return entries.map((e) => {
    if (e.isDir) {
      return `${e.path}/`;
    }
    filePaths.add(e.path);
    return e.path;
  });
}

function makeTree(paths: string[]) {
  return new FileTree({
    paths,
    initialExpansion: props.initialExpansion,
    density: "compact",
    search: true,
    gitStatus: props.gitStatus,
    renderRowDecoration: props.decoration ? ({ item }) => props.decoration!(item.path) : undefined,
    onSelectionChange(paths: readonly string[]) {
      if (syncingSelection) {
        return;
      }
      const last = paths[paths.length - 1];
      if (last && filePaths.has(last)) {
        emit("activate", last);
      }
    },
    // 库的亮暗默认走 color-scheme + light-dark()(跟随 OS);:host-context 桥接应用的
    // .dark 类,让默认配色随应用主题而非系统主题
    unsafeCSS: ":host { color-scheme: light; }\n:host-context(html.dark) { color-scheme: dark; }",
  });
}

function syncSelected(path: string | null | undefined) {
  if (!tree) {
    return;
  }
  syncingSelection = true;
  try {
    if (path && filePaths.has(path)) {
      tree.getItem(path)?.select();
      tree.scrollToPath(path, { offset: "nearest" });
    }
  } finally {
    syncingSelection = false;
  }
}

onMounted(() => {
  tree = makeTree(toPaths(props.entries));
  if (host.value) {
    tree.render({ containerWrapper: host.value });
  }
  syncSelected(props.selected);
});

watch(
  () => props.entries,
  (entries) => {
    tree?.resetPaths(toPaths(entries));
    syncSelected(props.selected);
  },
);

watch(
  () => props.gitStatus,
  (status) => tree?.setGitStatus(status),
);

watch(
  () => props.selected,
  (path) => syncSelected(path),
);

onBeforeUnmount(() => {
  tree?.cleanUp();
  tree = null;
});

/** 打开内置搜索框(文件树头部搜索按钮用) */
function openSearch() {
  tree?.openSearch();
}

defineExpose({ openSearch });
</script>

<template>
  <div ref="host" class="pierre-tree min-h-0 min-w-0 flex-1" />
</template>

<style>
/* 库渲染在 shadow DOM 内,CSS 变量可穿透;底色/文字/悬停对齐应用主题变量 */
.pierre-tree {
  --trees-bg-override: transparent;
  --trees-fg-override: var(--foreground);
  --trees-fg-muted-override: var(--muted-foreground);
  --trees-bg-muted-override: var(--accent);
  --trees-border-color-override: var(--border);
  --trees-accent: var(--primary);
  /* 行尾装饰(decoration parts)用色:git 新增/删除,随应用亮暗切换 */
  --tree-add: #16a34a;
  --tree-del: #dc2626;
  font-size: 13px;
}

html.dark .pierre-tree {
  --tree-add: #4ade80;
  --tree-del: #f87171;
}
</style>
