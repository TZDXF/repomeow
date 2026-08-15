<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronDown, ChevronRight, Loader2, Search } from "@lucide/vue";
import { Icon } from "@iconify/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cmd } from "@/lib/tauri";
import { fileIcon } from "@/lib/file-icons";
import { buildFindRegExp, type FindQuery } from "@/lib/text-search";
import type { TextSearchHit, TextSearchOutcome } from "@/types";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 左栏全文搜索面板:输入即搜(300ms 防抖,Enter 立即),大小写/全字/正则三模式,
// 与后端 search_project_text 的 SearchMatcher 同口径;文件包含/排除 glob 也由后端筛选;
// 结果按文件分组可折叠,行片段用前端同一正则标记命中(转义渲染,不走 v-html);
// 点击行 emit open,由父组件打开文件并把文本查询带过去做文件内定位。

const props = defineProps<{ root: string }>();

const emit = defineEmits<{
  (e: "open", path: string, line: number, query: FindQuery): void;
}>();

const { t } = useI18n();

const query = ref("");
const include = ref("");
const exclude = ref("");
const caseSensitive = ref(false);
const wholeWord = ref(false);
const useRegex = ref(false);
const results = ref<TextSearchHit[]>([]);
const truncated = ref(false);
const searching = ref(false);
const error = ref("");
const collapsed = ref(new Set<string>());
const host = ref<HTMLElement | null>(null);

const findQuery = computed<FindQuery>(() => ({
  text: query.value,
  caseSensitive: caseSensitive.value,
  wholeWord: wholeWord.value,
  useRegex: useRegex.value,
}));

/** 正则模式且查询非法时不发请求,直接置错 */
const invalidRegex = computed(
  () => useRegex.value && !!query.value.trim() && buildFindRegExp(findQuery.value) === null,
);

const totalMatches = computed(() => results.value.reduce((n, h) => n + h.count, 0));

const highlightRe = computed(() => {
  if (!results.value.length || invalidRegex.value) return null;
  return buildFindRegExp(findQuery.value);
});

let timer: ReturnType<typeof setTimeout> | undefined;
let seq = 0;

watch([query, include, exclude, caseSensitive, wholeWord, useRegex], () => {
  clearTimeout(timer);
  timer = setTimeout(runSearch, 300);
});

// 项目切换(root 变化)清空旧结果
watch(
  () => props.root,
  () => {
    seq++;
    results.value = [];
    truncated.value = false;
    error.value = "";
    searching.value = false;
  },
);

onBeforeUnmount(() => clearTimeout(timer));

async function runSearch() {
  clearTimeout(timer);
  const q = findQuery.value;
  if (!q.text.trim() || invalidRegex.value) {
    results.value = [];
    truncated.value = false;
    error.value = "";
    return;
  }
  const mySeq = ++seq;
  searching.value = true;
  error.value = "";
  try {
    const out = await cmd<TextSearchOutcome>("search_project_text", {
      root: props.root,
      query: q.text,
      caseSensitive: q.caseSensitive,
      wholeWord: q.wholeWord,
      useRegex: q.useRegex,
      include: include.value,
      exclude: exclude.value,
    });
    if (mySeq !== seq) return;
    results.value = out.hits;
    truncated.value = out.truncated;
    collapsed.value = new Set();
  } catch (e) {
    if (mySeq !== seq) return;
    results.value = [];
    truncated.value = false;
    // cmd 已把后端错误翻译成本地化文案
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (mySeq === seq) searching.value = false;
  }
}

function toggleCollapse(path: string) {
  const next = new Set(collapsed.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  collapsed.value = next;
}

/** 行片段按命中切分(span 渲染,内容经文本插值转义,无 XSS 面) */
function segments(text: string): { s: string; hit: boolean }[] {
  const re = highlightRe.value;
  if (!re) return [{ s: text, hit: false }];
  const out: { s: string; hit: boolean }[] = [];
  re.lastIndex = 0;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null && out.length < 60) {
    if (m[0].length === 0) {
      re.lastIndex++;
      continue;
    }
    if (m.index > last) out.push({ s: text.slice(last, m.index), hit: false });
    out.push({ s: m[0], hit: true });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ s: text.slice(last), hit: false });
  return out;
}

function focusInput() {
  host.value?.querySelector("input")?.focus();
}

defineExpose({ focusInput });
</script>

<template>
  <div ref="host" class="flex h-full min-h-0 flex-col">
    <div class="shrink-0 space-y-1.5 border-b p-2">
      <div class="relative">
        <Search
          class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="query"
          :placeholder="t('files.textSearchPlaceholder')"
          class="h-8 pl-8 pr-[5.5rem] text-sm"
          :class="invalidRegex ? 'border-destructive focus-visible:ring-destructive/30' : ''"
          @keydown.enter.prevent="runSearch"
        />
        <div class="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
          <Button
            v-for="m in [
              { key: 'caseSensitive', label: 'Aa', title: t('files.matchCase') },
              { key: 'wholeWord', label: 'ab', title: t('files.wholeWord') },
              { key: 'useRegex', label: '.*', title: t('files.useRegex') },
            ]"
            :key="m.key"
            variant="ghost"
            size="icon"
            class="pointer-events-auto h-6 w-7 rounded-sm font-mono text-[11px]"
            :class="
              (m.key === 'caseSensitive' && caseSensitive) ||
              (m.key === 'wholeWord' && wholeWord) ||
              (m.key === 'useRegex' && useRegex)
                ? 'bg-accent'
                : 'text-muted-foreground'
            "
            :title="m.title"
            @click.stop="
              m.key === 'caseSensitive'
                ? (caseSensitive = !caseSensitive)
                : m.key === 'wholeWord'
                  ? (wholeWord = !wholeWord)
                  : (useRegex = !useRegex)
            "
          >
            {{ m.label }}
          </Button>
        </div>
      </div>
      <div
        v-if="searching || invalidRegex || results.length"
        class="flex items-center gap-1.5 text-[11px] text-muted-foreground"
      >
        <Loader2 v-if="searching" class="h-3 w-3 animate-spin" />
        <template v-else-if="invalidRegex">{{ t("files.findInvalid") }}</template>
        <template v-else-if="results.length">
          {{ t("files.textSearchSummary", { files: results.length, matches: totalMatches }) }}
        </template>
      </div>
      <div class="flex items-center gap-1.5">
        <span class="w-20 shrink-0 whitespace-nowrap text-[11px] text-muted-foreground">{{
          t("files.textSearchInclude")
        }}</span>
        <Input
          v-model="include"
          :placeholder="t('files.textSearchIncludePlaceholder')"
          :title="t('files.textSearchIncludeHint')"
          class="h-7 text-xs"
          @keydown.enter.prevent="runSearch"
        />
      </div>
      <div class="flex items-center gap-1.5">
        <span class="w-20 shrink-0 whitespace-nowrap text-[11px] text-muted-foreground">{{
          t("files.textSearchExclude")
        }}</span>
        <Input
          v-model="exclude"
          :placeholder="t('files.textSearchExcludePlaceholder')"
          :title="t('files.textSearchExcludeHint')"
          class="h-7 text-xs"
          @keydown.enter.prevent="runSearch"
        />
      </div>
    </div>

    <ScrollArea class="min-h-0 flex-1">
      <p v-if="error" class="p-3 text-xs text-destructive">{{ error }}</p>
      <p v-else-if="!results.length" class="p-4 text-xs text-muted-foreground">
        {{ query.trim() ? t("files.textSearchNoResults") : t("files.textSearchHint") }}
      </p>
      <div v-else class="py-1">
        <p v-if="truncated" class="border-b px-3 py-1.5 text-xs text-muted-foreground">
          {{ t("files.textSearchTruncated") }}
        </p>
        <div v-for="hit in results" :key="hit.path">
          <button
            class="flex w-full items-center gap-1 px-2 py-1 text-left hover:bg-accent"
            :title="hit.path"
            @click="toggleCollapse(hit.path)"
          >
            <component
              :is="collapsed.has(hit.path) ? ChevronRight : ChevronDown"
              class="h-3 w-3 shrink-0 text-muted-foreground"
            />
            <Icon
              :icon="fileIcon(hit.path.slice(hit.path.lastIndexOf('/') + 1))"
              class="h-3.5 w-3.5 shrink-0"
            />
            <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ hit.path }}</span>
            <span
              class="shrink-0 rounded-sm bg-muted px-1 font-mono text-[10px] text-muted-foreground"
            >
              {{ hit.count }}
            </span>
          </button>
          <template v-if="!collapsed.has(hit.path)">
            <button
              v-for="ln in hit.lines"
              :key="ln.line"
              class="flex w-full items-start gap-2 py-0.5 pl-8 pr-2 text-left hover:bg-accent"
              @click="emit('open', hit.path, ln.line, findQuery)"
            >
              <span
                class="w-8 shrink-0 pt-px text-right font-mono text-[11px] text-muted-foreground"
              >
                {{ ln.line }}
              </span>
              <span class="min-w-0 flex-1 truncate font-mono text-[11px]">
                <template v-for="(seg, i) in segments(ln.text)" :key="i">
                  <mark v-if="seg.hit" class="rounded-sm bg-yellow-500/30 px-0.5 text-inherit">{{
                    seg.s
                  }}</mark>
                  <template v-else>{{ seg.s }}</template>
                </template>
              </span>
            </button>
          </template>
        </div>
      </div>
    </ScrollArea>
  </div>
</template>
