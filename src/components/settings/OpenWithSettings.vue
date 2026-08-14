<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Ban, Check, GripVertical, Pencil, Plus, Trash2 } from "@lucide/vue";
import { VueDraggable } from "vue-draggable-plus";
import OpenWithIcon from "@/components/open/OpenWithIcon.vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import CommandEditor from "@/components/scripts/CommandEditor.vue";
import { COMMAND_ICONS } from "@/lib/command-icons";
import {
  getEditorAvailability,
  isEditorUnavailable,
  isCustomOpenWithId,
  sortOpenWithOptions,
} from "@/lib/open-with";
import type { EditorAvailability, OpenWithOption } from "@/lib/open-with";
import { useSettingsStore } from "@/stores/settings";
import type { CustomOpenWith, OpenWithId } from "@/types";

const { t } = useI18n();
const store = useSettingsStore();
const commandVariables = { pathVariable: "{path}", lineVariable: "{line}" };
const commandPlaceholderExample = "code {path}:{line}";

const availability = ref<EditorAvailability | null>(null);
const dialogOpen = ref(false);
const editingId = ref<string | null>(null);
const formName = ref("");
const formCommand = ref("");
const formIcon = ref("");
const submitting = ref(false);

onMounted(async () => {
  availability.value = await getEditorAvailability();
});

const list = ref<OpenWithOption[]>([]);

watch(
  [availability, () => store.openWithOrder, () => store.customOpenWith],
  () => {
    list.value = sortOpenWithOptions(store.openWithOrder, store.customOpenWith).filter(
      (option) => !isEditorUnavailable(option, availability.value),
    );
  },
  { immediate: true, deep: true },
);

function persistOrder() {
  const visibleIds = new Set(list.value.map((option) => option.id));
  const queue = list.value.map((option) => option.id);
  let cursor = 0;
  const merged: OpenWithId[] = store.openWithOrder.map((id) =>
    visibleIds.has(id) ? queue[cursor++] : id,
  );
  void store.setOpenWithOrder(merged);
}

function openCreate() {
  editingId.value = null;
  formName.value = "";
  formCommand.value = "";
  formIcon.value = "";
  dialogOpen.value = true;
}

function openEdit(option: CustomOpenWith) {
  editingId.value = option.id;
  formName.value = option.name;
  formCommand.value = option.command;
  formIcon.value = option.icon;
  dialogOpen.value = true;
}

async function submit() {
  if (!formName.value.trim() || !formCommand.value.trim() || submitting.value) return;
  submitting.value = true;
  try {
    await store.saveCustomOpenWith({
      id: editingId.value ?? crypto.randomUUID(),
      name: formName.value,
      command: formCommand.value,
      icon: formIcon.value,
    });
    dialogOpen.value = false;
    toast.success(
      t(editingId.value == null ? "openWith.custom.created" : "openWith.custom.updated"),
    );
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function remove(option: CustomOpenWith) {
  if (!window.confirm(t("openWith.custom.deleteConfirm", { name: option.name }))) return;
  try {
    await store.removeCustomOpenWith(option.id);
    toast.success(t("openWith.custom.deleted"));
  } catch (e) {
    toast.error(String(e));
  }
}

const customById = computed(
  () => new Map(store.customOpenWith.map((option) => [`custom:${option.id}`, option])),
);
</script>

<template>
  <section>
    <div class="flex items-start justify-between gap-4">
      <div>
        <h2 class="text-base font-semibold">{{ t("settings.general.openWith") }}</h2>
        <p class="mt-1 text-sm text-muted-foreground">
          {{ t("settings.general.openWithDescription") }}
        </p>
      </div>
      <Button size="sm" variant="outline" @click="openCreate">
        <Plus class="h-4 w-4" />
        {{ t("openWith.custom.new") }}
      </Button>
    </div>
    <VueDraggable
      v-model="list"
      :animation="150"
      :force-fallback="true"
      handle=".drag-handle"
      class="mt-4 flex flex-col gap-2"
      @end="persistOrder"
    >
      <div
        v-for="option in list"
        :key="option.id"
        class="flex items-center gap-3 rounded-lg border px-3 py-2.5 transition-colors hover:bg-accent"
        :class="store.defaultOpenWith === option.id && 'border-primary'"
      >
        <GripVertical class="drag-handle h-4 w-4 shrink-0 cursor-grab text-muted-foreground" />
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-3 text-left"
          @click="store.setDefaultOpenWith(option.id)"
        >
          <OpenWithIcon
            :kind="option.custom ? undefined : option.kind"
            :icon="option.icon"
            icon-class="h-4 w-4 shrink-0"
          />
          <span class="min-w-0 flex-1 truncate text-sm font-medium">
            {{ option.custom ? option.name : t(option.labelKey) }}
          </span>
          <Check v-if="store.defaultOpenWith === option.id" class="h-4 w-4 shrink-0 text-primary" />
        </button>
        <template v-if="isCustomOpenWithId(option.id)">
          <Button
            size="icon-sm"
            variant="ghost"
            :title="t('common.edit')"
            @click="openEdit(customById.get(option.id)!)"
          >
            <Pencil class="h-3.5 w-3.5" />
          </Button>
          <Button
            size="icon-sm"
            variant="ghost"
            :title="t('common.delete')"
            @click="remove(customById.get(option.id)!)"
          >
            <Trash2 class="h-3.5 w-3.5" />
          </Button>
        </template>
      </div>
    </VueDraggable>
  </section>

  <Dialog v-model:open="dialogOpen">
    <DialogContent class="sm:max-w-[min(34rem,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{
          editingId == null ? t("openWith.custom.dialogNew") : t("openWith.custom.dialogEdit")
        }}</DialogTitle>
      </DialogHeader>
      <form class="flex flex-col gap-3" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("openWith.custom.nameLabel") }}</label>
          <Input v-model="formName" :placeholder="t('openWith.custom.namePlaceholder')" />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("openWith.custom.commandLabel") }}</label>
          <CommandEditor
            v-model="formCommand"
            :placeholder="t('openWith.custom.commandPlaceholder', { example: commandPlaceholderExample })"
          />
          <p class="text-xs text-muted-foreground">
            {{ t("openWith.custom.commandHint", commandVariables) }}
          </p>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("openWith.custom.iconLabel") }}</label>
          <div class="grid grid-cols-8 gap-1">
            <button
              type="button"
              class="flex h-8 w-8 items-center justify-center rounded-md border text-muted-foreground transition-colors hover:bg-accent"
              :class="
                formIcon === '' ? 'border-primary bg-accent text-foreground' : 'border-transparent'
              "
              :title="t('openWith.custom.noIcon')"
              @click="formIcon = ''"
            >
              <Ban class="h-4 w-4" />
            </button>
            <button
              v-for="icon in COMMAND_ICONS"
              :key="icon.name"
              type="button"
              class="flex h-8 w-8 items-center justify-center rounded-md border text-muted-foreground transition-colors hover:bg-accent"
              :class="
                formIcon === icon.name
                  ? 'border-primary bg-accent text-foreground'
                  : 'border-transparent'
              "
              :title="icon.name"
              @click="formIcon = icon.name"
            >
              <component :is="icon.component" class="h-4 w-4" />
            </button>
          </div>
        </div>
        <DialogFooter>
          <Button type="submit" :disabled="!formName.trim() || !formCommand.trim() || submitting">
            {{ submitting ? t("common.saving") : t("common.save") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
