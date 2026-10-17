<script setup lang="ts">
import { computed, provide, ref } from "vue";
import { RouterView, routeLocationKey, type RouteLocationNormalizedLoaded } from "vue-router";
import { ConfigProvider } from "reka-ui";
import { PROJECT_PAGE_ACTIVE } from "@/composables/useProjectPageState";

const props = defineProps<{ route: RouteLocationNormalizedLoaded; active: boolean }>();
const container = ref<HTMLElement>();
provide(
  PROJECT_PAGE_ACTIVE,
  computed(() => props.active),
);
// RouterView 的 route 只决定匹配组件;useRoute() 也必须绑定该页面自己的路由快照。
const scopedRoute = {} as RouteLocationNormalizedLoaded;
for (const key of Object.keys(props.route)) {
  Object.defineProperty(scopedRoute, key, {
    enumerable: true,
    get: () => props.route[key as keyof RouteLocationNormalizedLoaded],
  });
}
provide(routeLocationKey, scopedRoute);
</script>

<template>
  <div
    v-show="active"
    ref="container"
    class="h-full"
    :class="{ 'project-page-inactive': !active }"
    :inert="!active"
  >
    <!-- 下拉菜单、日期选择等 portal 也跟随所属页面隐藏,不会残留到另一个 tab。 -->
    <ConfigProvider :teleport-to="container">
      <RouterView :route="route" />
    </ConfigProvider>
  </div>
</template>

<style scoped>
/* display:none 的祖先会中断退出动画,Presence 不能再等待 animationend 才释放交互锁。 */
.project-page-inactive :deep([data-state="closed"]) {
  animation: none !important;
  transition: none !important;
}
</style>
