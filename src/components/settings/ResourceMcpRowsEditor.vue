<script lang="ts">
/** 行式编辑器的单行数据;id 由编辑器分配,仅用于行追踪(key/value 由父组件解释) */
export interface McpRow {
  id: number;
  key: string;
  value: string;
}
</script>

<script setup lang="ts">
import { Plus, X } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const props = withDefaults(
  defineProps<{
    rows: McpRow[];
    addLabel: string;
    keyPlaceholder?: string;
    valuePlaceholder?: string;
    /** 单值模式(如命令参数):隐藏键输入列 */
    single?: boolean;
  }>(),
  { single: false, keyPlaceholder: "", valuePlaceholder: "" },
);

const emit = defineEmits<{
  "update:rows": [rows: McpRow[]];
}>();

const { t } = useI18n();

let seq = 0;

function update(id: number, patch: Partial<Omit<McpRow, "id">>) {
  emit(
    "update:rows",
    props.rows.map((row) => (row.id === id ? { ...row, ...patch } : row)),
  );
}

function remove(id: number) {
  emit(
    "update:rows",
    props.rows.filter((row) => row.id !== id),
  );
}

function add() {
  seq += 1;
  emit("update:rows", [...props.rows, { id: seq, key: "", value: "" }]);
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div v-for="row in rows" :key="row.id" class="flex items-center gap-1.5">
      <Input
        v-if="!single"
        :model-value="row.key"
        class="h-7 w-28 shrink-0 font-mono text-xs"
        :placeholder="keyPlaceholder"
        spellcheck="false"
        @update:model-value="update(row.id, { key: String($event) })"
      />
      <Input
        :model-value="row.value"
        class="h-7 min-w-0 flex-1 font-mono text-xs"
        :placeholder="valuePlaceholder"
        spellcheck="false"
        @update:model-value="update(row.id, { value: String($event) })"
      />
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 text-muted-foreground hover:text-destructive"
        :title="t('settings.resources.mcp.editDialog.removeRow')"
        @click="remove(row.id)"
      >
        <X class="h-3.5 w-3.5" />
      </Button>
    </div>
    <Button variant="outline" size="sm" class="h-7 w-fit gap-1 text-xs" @click="add">
      <Plus class="h-3 w-3" />
      {{ addLabel }}
    </Button>
  </div>
</template>
