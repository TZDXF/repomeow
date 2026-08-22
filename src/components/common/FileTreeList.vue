<script setup lang="ts" generic="T">
import { useI18n } from "vue-i18n";
import { ChevronRight, LoaderCircle } from "@lucide/vue";
import { Icon } from "@iconify/vue";
import { fileIcon, folderIcon } from "@/lib/file-icons";
import type { FileTreeRow } from "@/lib/file-tree";

/**
 * 统一文件列表/树渲染:消费 FileTreeRow 行模型(lazy-file-tree 的 buildVisibleRows
 * 或 file-tree 的 flattenVisibleTree / flatFileRows 产出),自身不持有数据与业务行为;
 * 行首(箭头后、图标前)与行尾经 #leading / #trailing 插槽扩展(勾选框、状态徽标、
 * +N/-N 等),文件行点击发 select、可展开目录行点击发 toggle
 */
const props = withDefaults(
  defineProps<{
    rows: FileTreeRow<T>[];
    /** 选中的行 fullPath */
    selected?: string | null;
    /** default = 文件树页(text-sm/图标 h-4);sm = git 面板(text-xs mono/图标 h-3.5) */
    size?: "default" | "sm";
    /** 平铺模式:不缩进不渲染箭头,行内边距 px-3 */
    flat?: boolean;
  }>(),
  { selected: null, size: "default", flat: false },
);

const emit = defineEmits<{
  select: [row: FileTreeRow<T>];
  toggle: [row: FileTreeRow<T>];
}>();

const { t } = useI18n();

function isSelected(row: FileTreeRow<T>) {
  return !row.loading && props.selected != null && props.selected === row.fullPath;
}

function onRowClick(row: FileTreeRow<T>) {
  if (row.loading) {
    return;
  }
  if (row.isDir) {
    // 已知空目录不 expandable,点击无动作;懒加载树的未加载目录由父级按需拉取
    if (row.expandable) emit("toggle", row);
  } else {
    emit("select", row);
  }
}
</script>

<template>
  <div class="py-1">
    <button
      v-for="row in rows"
      :key="row.key"
      type="button"
      class="flex w-full cursor-pointer items-center text-left transition-colors"
      :class="[
        size === 'sm'
          ? 'gap-1.5 py-1 font-mono text-xs hover:bg-accent/60'
          : 'gap-1 py-1 text-sm hover:bg-accent',
        flat ? 'px-3' : 'pr-2',
        isSelected(row)
          ? size === 'sm'
            ? 'bg-accent'
            : 'file-row-selected bg-accent text-accent-foreground'
          : '',
        row.dimmed ? 'opacity-50' : '',
      ]"
      :style="flat ? undefined : { paddingLeft: `${8 + row.depth * 14}px` }"
      :title="row.loading ? undefined : (row.title ?? row.fullPath)"
      @click="onRowClick(row)"
    >
      <template v-if="row.loading">
        <LoaderCircle class="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground" />
        <span class="min-w-0 truncate text-muted-foreground">{{ t("common.loading") }}</span>
      </template>
      <template v-else>
        <span
          v-if="!flat"
          class="shrink-0 text-muted-foreground"
          :class="size === 'sm' ? 'w-3' : 'w-3.5'"
        >
          <ChevronRight
            v-if="row.expandable"
            :class="[size === 'sm' ? 'h-3 w-3' : 'h-3.5 w-3.5', row.expanded ? 'rotate-90' : '']"
            class="transition-transform"
          />
        </span>
        <slot name="leading" :row="row" />
        <Icon
          :icon="row.isDir ? folderIcon(row.name, row.expanded) : fileIcon(row.name)"
          class="shrink-0"
          :class="size === 'sm' ? 'h-3.5 w-3.5' : 'h-4 w-4'"
        />
        <span class="min-w-0 flex-1 truncate">{{ row.name }}</span>
        <slot name="trailing" :row="row" />
      </template>
    </button>
  </div>
</template>
