<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
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
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { AiModelDef, AiProvider } from "@/lib/ai-config";

const props = defineProps<{
  open: boolean;
  /** 内置厂商目录(设置页 onMounted 拉取,失败时仅剩自定义) */
  catalog: Record<string, AiProvider>;
  /** 已存在的厂商 id(去重;内置选项过滤与提交前校验共用) */
  existingIds: string[];
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  /** 确认添加:models 为内置厂商带入的预置模型(自定义为空数组) */
  add: [
    payload: { id: string; name: string; baseUrl: string; apiKey: string; models: AiModelDef[] },
  ];
}>();

const { t } = useI18n();

/** 厂商选择中「自定义」选项的固定值 */
const CUSTOM_CHOICE = "custom";

const addChoice = ref(CUSTOM_CHOICE);
const addForm = reactive({ id: "", name: "", baseUrl: "", apiKey: "" });

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    addChoice.value = CUSTOM_CHOICE;
    addForm.id = "";
    addForm.name = "";
    addForm.baseUrl = "";
    addForm.apiKey = "";
  },
);

/** 内置目录中尚未添加的厂商(已存在的 id 不再提供,避免重复) */
const addOptions = computed(() => {
  const used = new Set(props.existingIds);
  return Object.entries(props.catalog)
    .filter(([id]) => !used.has(id))
    .map(([id, provider]) => ({ id, name: provider.name.trim() || id }));
});

/** 选中内置厂商时带入的预置模型数(对话框提示用) */
const seededModelCount = computed(() =>
  addChoice.value === CUSTOM_CHOICE ? 0 : (props.catalog[addChoice.value]?.models.length ?? 0),
);

function onAddChoiceChange(value: unknown) {
  const choice = String(value);
  addChoice.value = choice;
  if (choice === CUSTOM_CHOICE) {
    addForm.id = "";
    addForm.name = "";
    addForm.baseUrl = "";
    return;
  }
  const provider = props.catalog[choice];
  if (!provider) return;
  addForm.id = choice;
  addForm.name = provider.name;
  addForm.baseUrl = provider.baseUrl;
}

function confirmAdd() {
  const id = addForm.id.trim();
  if (!id) {
    toast.error(t("settings.ai.missingProviderId"));
    return;
  }
  if (props.existingIds.includes(id)) {
    toast.error(t("settings.ai.duplicateProviderId", { id }));
    return;
  }
  const catalogModels =
    addChoice.value === CUSTOM_CHOICE ? [] : (props.catalog[addChoice.value]?.models ?? []);
  emit("add", {
    id,
    name: addForm.name.trim(),
    baseUrl: addForm.baseUrl.trim(),
    apiKey: addForm.apiKey.trim(),
    models: catalogModels,
  });
  emit("update:open", false);
}
</script>

<template>
  <!-- 先选内置厂商(带入地址与预置模型)或自定义,再补 API Key -->
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{ t("settings.ai.addProviderTitle") }}</DialogTitle>
        <DialogDescription>{{ t("settings.ai.addProviderDesc") }}</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-3 py-1">
        <div class="flex flex-col gap-1">
          <label class="text-muted-foreground text-xs">{{ t("settings.ai.providerSelect") }}</label>
          <Select :model-value="addChoice" @update:model-value="onAddChoiceChange">
            <SelectTrigger class="h-8 w-full text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem :value="CUSTOM_CHOICE">
                  {{ t("settings.ai.customProvider") }}
                </SelectItem>
                <SelectItem v-for="option in addOptions" :key="option.id" :value="option.id">
                  {{ option.name }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>

        <div class="grid grid-cols-2 gap-2">
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.providerId") }}</label>
            <Input
              v-model="addForm.id"
              class="h-8 text-xs"
              :placeholder="t('settings.ai.providerIdPlaceholder')"
              spellcheck="false"
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.providerName") }}</label>
            <Input
              v-model="addForm.name"
              class="h-8 text-xs"
              :placeholder="t('settings.ai.providerNamePlaceholder')"
              spellcheck="false"
            />
          </div>
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-muted-foreground text-xs">{{ t("settings.ai.baseUrl") }}</label>
          <Input
            v-model="addForm.baseUrl"
            class="h-8 text-xs"
            :placeholder="t('settings.ai.baseUrlPlaceholder')"
            spellcheck="false"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label class="text-muted-foreground text-xs">{{ t("settings.ai.apiKey") }}</label>
          <Input
            v-model="addForm.apiKey"
            type="password"
            class="h-8 text-xs"
            :placeholder="t('settings.ai.apiKeyPlaceholder')"
            autocomplete="off"
            spellcheck="false"
          />
        </div>
        <p v-if="seededModelCount > 0" class="text-muted-foreground text-xs">
          {{ t("settings.ai.seededModelsHint", { count: seededModelCount }) }}
        </p>
      </div>

      <DialogFooter>
        <Button variant="outline" @click="emit('update:open', false)">
          {{ t("common.cancel") }}
        </Button>
        <Button @click="confirmAdd">{{ t("common.add") }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
