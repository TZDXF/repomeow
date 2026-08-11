import { createRouter, createWebHashHistory } from "vue-router";
import ProjectsHome from "@/views/ProjectsHome.vue";
import ProjectDetail from "@/views/ProjectDetail.vue";
import GitGraph from "@/views/GitGraph.vue";
import Settings from "@/views/Settings.vue";
import ReportHistory from "@/views/ReportHistory.vue";
import TrayPopup from "@/views/TrayPopup.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "home", component: ProjectsHome },
    { path: "/projects/:id", name: "project", component: ProjectDetail },
    { path: "/projects/:id/graph", name: "project-graph", component: GitGraph },
    { path: "/settings", name: "settings", component: Settings },
    { path: "/report-history", name: "history", component: ReportHistory },
    // 托盘迷你弹窗窗口加载 index.html#/tray 进入该路由
    { path: "/tray", name: "tray", component: TrayPopup },
  ],
});
