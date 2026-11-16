import { ref } from "vue";
import { defineStore } from "pinia";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import type { Tag } from "@/types";

export const useTagsStore = defineStore("tags", () => {
  const tags = ref<Tag[]>([]);
  let loadSequence = 0;

  async function fetchTags() {
    const sequence = ++loadSequence,
      fresh = await cmd<Tag[]>("list_tags");
    if (sequence === loadSequence) {
      tags.value = fresh;
    }
  }

  // 标签增删改会影响项目内嵌的 tags(name/color/关联),需同步刷新项目列表;
  // withGit: false 只更新列表数据,保留已有 git 状态,避免触发 git 拉取
  async function refreshProjects() {
    await useProjectsStore().fetchProjects({ withGit: false });
  }

  async function createTag(name: string, color: string) {
    const tag = await cmd<Tag>("create_tag", { name, color });
    await fetchTags();
    return tag;
  }

  async function updateTag(id: number, name: string, color: string) {
    const tag = await cmd<Tag>("update_tag", { id, name, color });
    await fetchTags();
    await refreshProjects();
    return tag;
  }

  async function deleteTag(id: number) {
    await cmd("delete_tag", { id });
    await fetchTags();
    await refreshProjects();
  }

  async function setProjectTags(projectId: number, tagIds: number[]) {
    await cmd("set_project_tags", { projectId, tagIds });
  }

  return { tags, fetchTags, createTag, updateTag, deleteTag, setProjectTags };
});
