import { createRouter, createWebHashHistory } from "vue-router";
// 所有页面按路由懒加载，避免详情、设置、报表及托盘窗口的重型依赖进入首屏主包。
const ProjectsHome = () => import("@/views/ProjectsHome.vue");
const ProjectDetail = () => import("@/views/ProjectDetail.vue");
const ProjectFiles = () => import("@/views/ProjectFiles.vue");
const GitGraph = () => import("@/views/GitGraph.vue");
const ProjectWiki = () => import("@/views/ProjectWiki.vue");
const Settings = () => import("@/views/Settings.vue");
const ResourceSkillPreview = () => import("@/views/ResourceSkillPreview.vue");
const ReportHistory = () => import("@/views/ReportHistory.vue");
const TrayPopup = () => import("@/views/TrayPopup.vue");

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "home", component: ProjectsHome },
    { path: "/projects/:id", name: "project", component: ProjectDetail },
    { path: "/projects/:id/files", name: "project-files", component: ProjectFiles },
    { path: "/projects/:id/graph", name: "project-graph", component: GitGraph },
    { path: "/projects/:id/wiki", name: "project-wiki", component: ProjectWiki },
    { path: "/settings", name: "settings", component: Settings },
    {
      path: "/settings/resources/skills/:id",
      name: "resource-skill",
      component: ResourceSkillPreview,
    },
    { path: "/report-history", name: "history", component: ReportHistory },
    // 托盘迷你弹窗窗口加载 index.html#/tray 进入该路由
    { path: "/tray", name: "tray", component: TrayPopup },
  ],
});
