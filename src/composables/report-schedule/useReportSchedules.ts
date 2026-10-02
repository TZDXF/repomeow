import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import type { ReportSchedule, SystemSchedule } from "@/types";

/**
 * 定时报告设置的数据层:报告任务与系统内置任务的加载、保存、启停、删除与手动执行。
 * 项目数量变化时自动重载(项目归档/删除会影响任务展示)。
 */
export function useReportSchedules() {
  const { t } = useI18n();
  const projectStore = useProjectsStore();

  const schedules = ref<ReportSchedule[]>([]);
  const systemSchedules = ref<SystemSchedule[]>([]);
  const loading = ref(false);
  /** 手动执行中的任务 id 集合(按钮 loading 态) */
  const runningIds = ref<string[]>([]);

  async function load() {
    loading.value = true;
    try {
      const [reportItems, systemItems] = await Promise.all([
        cmd<ReportSchedule[]>("list_report_schedules"),
        cmd<SystemSchedule[]>("list_system_schedules"),
      ]);
      schedules.value = reportItems;
      systemSchedules.value = systemItems;
    } catch {
      toast.error(t("reportSchedule.saveFailed"));
    } finally {
      loading.value = false;
    }
  }

  async function saveSystemSchedule(
    schedule: SystemSchedule,
    enabled: boolean,
    intervalMinutes: number,
  ) {
    const saved = await cmd<SystemSchedule>("save_system_schedule", {
      id: schedule.id,
      enabled,
      intervalMinutes,
    });
    const index = systemSchedules.value.findIndex((item) => item.id === saved.id);
    if (index !== -1) {
      systemSchedules.value[index] = saved;
    }
  }

  async function toggleSystemSchedule(schedule: SystemSchedule) {
    try {
      await saveSystemSchedule(schedule, !schedule.enabled, schedule.intervalMinutes);
    } catch {
      toast.error(t("reportSchedule.saveFailed"));
    }
  }

  async function saveAll(items: ReportSchedule[]) {
    try {
      await cmd("save_report_schedules", { schedules: items });
    } catch {
      toast.error(t("reportSchedule.saveFailed"));
    }
  }

  async function toggleSchedule(s: ReportSchedule) {
    s.enabled = !s.enabled;
    await saveAll(schedules.value);
  }

  /** 编辑对话框提交:按 id 覆盖或追加,然后整体保存 */
  async function upsertSchedule(data: ReportSchedule) {
    const idx = schedules.value.findIndex((s) => s.id === data.id);
    if (idx !== -1) {
      schedules.value[idx] = data;
    } else {
      schedules.value.push(data);
    }
    await saveAll(schedules.value);
    toast.success(t("reportSchedule.saved"));
  }

  async function deleteSchedule(id: string) {
    schedules.value = schedules.value.filter((s) => s.id !== id);
    await saveAll(schedules.value);
    toast.success(t("reportSchedule.deleted"));
  }

  /** 手动执行:忽略星期/去重检查,立即按任务配置生成报告 */
  async function runNow(s: ReportSchedule) {
    if (runningIds.value.includes(s.id)) return;
    runningIds.value = [...runningIds.value, s.id];
    try {
      await cmd<number>("run_report_schedule_now", { id: s.id });
      toast.success(t("reportSchedule.runSuccess"));
      // 刷新 lastRunAt 展示
      await load();
    } catch (e) {
      const message = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
      toast.error(t("reportSchedule.runFailed", { error: message }));
    } finally {
      runningIds.value = runningIds.value.filter((id) => id !== s.id);
    }
  }

  watch(
    () => projectStore.projects.length,
    () => load(),
    { immediate: true },
  );

  return {
    schedules,
    systemSchedules,
    loading,
    runningIds,
    load,
    saveSystemSchedule,
    toggleSystemSchedule,
    toggleSchedule,
    upsertSchedule,
    deleteSchedule,
    runNow,
  };
}
