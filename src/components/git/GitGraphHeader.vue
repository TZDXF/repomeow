<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { ArrowLeft, ChevronDown, ListFilter, Loader2, RefreshCw, Search, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import type { GitGraphCommit } from "@/types";

defineProps<{
  projectName: string;
  searchResults: GitGraphCommit[];
  filterLabel: string;
  resolvedFilterBranch: string;
  totalCount: number;
  loading: boolean;
}>();

const emit = defineEmits<{
  back: [];
  locate: [commit: GitGraphCommit];
  refresh: [];
}>();

const searchQuery = defineModel<string>("searchQuery", { required: true });
const filterValue = defineModel<string>("filterValue", { required: true });
const { t } = useI18n();

function shortHash(hash: string) {
  return hash.slice(0, 7);
}
</script>

<template>
  <header class="flex items-center gap-2 border-b px-4 py-3">
    <Button
      variant="ghost"
      size="icon"
      class="h-8 w-8 shrink-0"
      :title="t('git.graph.back')"
      @click="emit('back')"
    >
      <ArrowLeft class="h-4 w-4" />
    </Button>
    <div class="min-w-0 flex-1">
      <h1 class="truncate text-sm font-medium">{{ projectName }} · {{ t("git.graph.title") }}</h1>
    </div>

    <div class="relative w-60 shrink-0">
      <Search
        class="absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
      />
      <Input
        v-model="searchQuery"
        :placeholder="t('git.graph.searchPlaceholder')"
        class="h-8 pr-7 pl-8 text-xs"
        @keydown.enter.prevent="searchResults[0] && emit('locate', searchResults[0])"
        @keydown.esc="searchQuery = ''"
      />
      <button
        v-if="searchQuery"
        class="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground transition-colors hover:text-foreground"
        @click="searchQuery = ''"
      >
        <X class="h-3.5 w-3.5" />
      </button>
      <div
        v-if="searchQuery.trim()"
        class="absolute top-full right-0 z-50 mt-1 max-h-72 w-80 overflow-auto rounded-md border bg-popover p-1 shadow-md"
      >
        <p v-if="!searchResults.length" class="px-2 py-1.5 text-xs text-muted-foreground">
          {{ t("git.graph.searchEmpty") }}
        </p>
        <button
          v-for="commit in searchResults"
          :key="commit.hash"
          class="flex w-full flex-col gap-0.5 rounded-sm px-2 py-1.5 text-left transition-colors hover:bg-accent"
          @click="emit('locate', commit)"
        >
          <span class="truncate text-xs">{{ commit.subject }}</span>
          <span class="truncate font-mono text-[10px] text-muted-foreground">
            {{ shortHash(commit.hash) }} · {{ commit.author }} · {{ commit.date }}
          </span>
        </button>
      </div>
    </div>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button variant="outline" size="sm" class="max-w-48 shrink-0 gap-1">
          <ListFilter class="h-3.5 w-3.5 shrink-0" />
          <span class="truncate">{{ filterLabel }}</span>
          <ChevronDown class="h-3 w-3 shrink-0 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" class="w-56">
        <DropdownMenuRadioGroup v-model="filterValue">
          <DropdownMenuRadioItem value="all">
            {{ t("git.graph.filterAll") }}
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="current" :disabled="!resolvedFilterBranch">
            {{ t("git.graph.filterCurrent") }}
            <span v-if="resolvedFilterBranch" class="ml-1 truncate text-muted-foreground">
              ({{ resolvedFilterBranch }})
            </span>
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>

    <span v-if="totalCount" class="shrink-0 text-xs text-muted-foreground">
      {{ t("git.graph.commitsCount", { count: totalCount }) }}
    </span>
    <Button
      variant="outline"
      size="sm"
      class="shrink-0"
      :disabled="loading"
      @click="emit('refresh')"
    >
      <Loader2 v-if="loading" class="h-3.5 w-3.5 animate-spin" />
      <RefreshCw v-else class="h-3.5 w-3.5" />
      {{ t("git.graph.refresh") }}
    </Button>
  </header>
</template>
