<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderOpen } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  createResourceSkill,
  openResourceSkillDir,
  readResourceSkillBody,
  updateResourceSkill,
  type ResourceSkill,
  type ResourceSkillGroup,
  type ResourceSkillInput,
} from "@/lib/resource-library";

const props = defineProps<{
  open: boolean;
  groups: ResourceSkillGroup[];
  /** null = 新建;非 null = 编辑该技能 */
  skill: ResourceSkill | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 创建或更新成功后触发(父组件刷新列表) */
  saved: [];
}>();

const { t } = useI18n();

const name = ref("");
const description = ref("");
const selectedGroupIds = ref<string[]>([]);
const body = ref("");
const loadingBody = ref(false);
const saving = ref(false);

/** 打开目标变化(含编辑 A → 直接切新建)时重置表单;token 丢弃迟到的 body 加载结果 */
let loadToken = 0;

watch(
  () => [props.open, props.skill] as const,
  async ([open]) => {
    if (!open) {
      return;
    }
    const token = ++loadToken;
    name.value = props.skill?.name ?? "";
    description.value = props.skill?.description ?? "";
    selectedGroupIds.value = props.skill ? [...props.skill.groupIds] : [];
    body.value = "";
    if (props.skill) {
      loadingBody.value = true;
      try {
        const loaded = await readResourceSkillBody(props.skill.id);
        if (token === loadToken) {
          body.value = loaded.body;
        }
      } catch (e) {
        if (token === loadToken) {
          toast.error(
            t("settings.resources.skills.editDialog.loadBodyFailed", { error: String(e) }),
          );
        }
      } finally {
        if (token === loadToken) {
          loadingBody.value = false;
        }
      }
    }
  },
);

function toggleGroup(groupId: string) {
  const index = selectedGroupIds.value.indexOf(groupId);
  if (index === -1) {
    selectedGroupIds.value.push(groupId);
  } else {
    selectedGroupIds.value.splice(index, 1);
  }
}

async function openDir() {
  if (!props.skill) {
    return;
  }
  try {
    await openResourceSkillDir(props.skill.id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function save() {
  const trimmed = name.value.trim();
  if (!trimmed || saving.value) {
    return;
  }
  saving.value = true;
  const input: ResourceSkillInput = {
    name: trimmed,
    description: description.value.trim(),
    groupIds: [...selectedGroupIds.value],
    body: body.value,
  };
  try {
    if (props.skill) {
      await updateResourceSkill(props.skill.id, input);
      toast.success(t("settings.resources.skills.editDialog.saved"));
    } else {
      await createResourceSkill(input);
      toast.success(t("settings.resources.skills.editDialog.created"));
    }
    emit("saved");
    emit("update:open", false);
  } catch (e) {
    toast.error(String(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>
          {{
            skill
              ? t("settings.resources.skills.editDialog.editTitle")
              : t("settings.resources.skills.editDialog.createTitle")
          }}
        </DialogTitle>
        <DialogDescription>
          {{
            skill
              ? t("settings.resources.skills.editDialog.editDescription")
              : t("settings.resources.skills.editDialog.createDescription")
          }}
        </DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-3 py-1">
        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.skills.editDialog.nameLabel") }}
          </label>
          <Input
            v-model="name"
            class="h-8 text-xs"
            :placeholder="t('settings.resources.skills.editDialog.namePlaceholder')"
            spellcheck="false"
          />
        </div>

        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.skills.editDialog.descriptionLabel") }}
          </label>
          <Input
            v-model="description"
            class="h-8 text-xs"
            :placeholder="t('settings.resources.skills.editDialog.descriptionPlaceholder')"
            spellcheck="false"
          />
        </div>

        <div class="flex flex-col gap-1.5">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.skills.editDialog.groupsLabel") }}
          </label>
          <div v-if="groups.length" class="flex flex-wrap gap-1.5">
            <button
              v-for="group in groups"
              :key="group.id"
              type="button"
              class="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors"
              :class="
                selectedGroupIds.includes(group.id)
                  ? 'border-foreground bg-foreground text-background'
                  : 'text-muted-foreground hover:bg-accent hover:text-foreground'
              "
              @click="toggleGroup(group.id)"
            >
              <span
                class="h-2 w-2 rounded-full"
                :style="{ backgroundColor: group.color ?? 'var(--muted-foreground)' }"
              />
              {{ group.name }}
            </button>
          </div>
          <p v-else class="text-xs text-muted-foreground">
            {{ t("settings.resources.skills.editDialog.noGroups") }}
          </p>
        </div>

        <div class="flex flex-col gap-1">
          <label class="text-xs text-muted-foreground">
            {{ t("settings.resources.skills.editDialog.bodyLabel") }}
          </label>
          <Textarea
            v-model="body"
            rows="14"
            spellcheck="false"
            :disabled="loadingBody"
            class="min-h-64 resize-y font-mono text-xs"
            :placeholder="t('settings.resources.skills.editDialog.bodyPlaceholder')"
          />
        </div>
      </div>

      <DialogFooter class="gap-2 sm:justify-between">
        <Button v-if="skill" variant="outline" class="gap-1.5" :disabled="saving" @click="openDir">
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("settings.resources.skills.editDialog.openDir") }}
        </Button>
        <span v-else class="flex-1" />
        <span class="flex gap-2">
          <Button variant="outline" :disabled="saving" @click="emit('update:open', false)">
            {{ t("common.cancel") }}
          </Button>
          <Button :disabled="!name.trim() || saving" @click="save">
            {{ t("common.save") }}
          </Button>
        </span>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
