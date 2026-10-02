import { beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "vue-sonner";
import { cmd } from "@/lib/tauri";
import { getCachedScanReport, putCachedScanReport } from "@/lib/scan-cache";
import type { ResourceSkillScanReport } from "@/lib/resource-library";
import { useSkillScan, type SkillScanRunOptions } from "./useSkillScan";

vi.mock("vue-sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));
vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));
vi.mock("@/lib/scan-cache", () => ({
  getCachedScanReport: vi.fn(() => Promise.resolve(null)),
  putCachedScanReport: vi.fn(() => Promise.resolve()),
}));

const cmdMock = vi.mocked(cmd);
const getCachedMock = vi.mocked(getCachedScanReport);
const putCachedMock = vi.mocked(putCachedScanReport);
const toastError = vi.mocked(toast.error);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function makeReport(overrides: Partial<ResourceSkillScanReport> = {}): ResourceSkillScanReport {
  return {
    skillId: "skill-1",
    score: 12,
    level: "low",
    findings: [],
    staticCount: 0,
    suppressedCount: 0,
    filesScanned: 3,
    llmStatus: "ok",
    scannedAt: 1_700_000_000_000,
    ...overrides,
  };
}

function setup(scan: (options: SkillScanRunOptions) => Promise<ResourceSkillScanReport>) {
  return useSkillScan({
    scan,
    getLanguage: () => "zh-CN",
    getCacheId: () => "skill-1",
    getFingerprint: () => "fp-1",
    formatError: (e) => `fmt:${String(e)}`,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  cmdMock.mockResolvedValue(undefined as never);
});

describe("useSkillScan", () => {
  it("扫描成功:回填报告并写缓存,计时复位", async () => {
    const report = makeReport();
    const scan = vi.fn(() => Promise.resolve(report));
    const skillScan = setup(scan);

    await skillScan.runScan(null);
    expect(skillScan.scanReport.value).toBe(report);
    expect(skillScan.scanning.value).toBe(false);
    expect(scan).toHaveBeenCalledWith(
      expect.objectContaining({ language: "zh-CN", runId: expect.stringContaining("scan-") }),
    );
    expect(putCachedMock).toHaveBeenCalledWith("skill-1", "fp-1", report);
    expect(skillScan.scanElapsedLabel.value).toBe("00:00");
  });

  it("扫描失败:toast 格式化错误; dispose 后的在途失败不 toast", async () => {
    const failing = deferred<ResourceSkillScanReport>();
    const scan = vi.fn(() => failing.promise);
    const skillScan = setup(scan);

    const p1 = skillScan.runScan({ providerId: "p", modelId: "m" });
    expect(scan).toHaveBeenCalledWith(expect.objectContaining({ providerId: "p", modelId: "m" }));
    failing.reject(new Error("boom"));
    await p1;
    expect(toastError).toHaveBeenCalledWith("fmt:Error: boom");
    expect(skillScan.scanning.value).toBe(false);

    toastError.mockClear();
    const second = deferred<ResourceSkillScanReport>();
    scan.mockImplementation(() => second.promise);
    const p2 = skillScan.runScan(null);
    // 离开页面:作废在途结果
    skillScan.dispose();
    expect(skillScan.scanning.value).toBe(false);
    expect(cmdMock).toHaveBeenCalledWith("ai_cancel_run", {
      runId: expect.stringContaining("scan-"),
    });
    second.reject(new Error("late failure"));
    await p2;
    expect(toastError).not.toHaveBeenCalled();
    expect(skillScan.scanReport.value).toBeNull();
  });

  it("dispose 后晚到的成功报告不回填、不写缓存", async () => {
    const pending = deferred<ResourceSkillScanReport>();
    const scan = vi.fn(() => pending.promise);
    const skillScan = setup(scan);

    const p = skillScan.runScan(null);
    skillScan.dispose();
    pending.resolve(makeReport());
    await p;
    expect(skillScan.scanReport.value).toBeNull();
    expect(putCachedMock).not.toHaveBeenCalled();
  });

  it("用户取消只发 ai_cancel_run:返回的静态报告(llmStatus=canceled)仍展示", async () => {
    const pending = deferred<ResourceSkillScanReport>();
    const scan = vi.fn(() => pending.promise);
    const skillScan = setup(scan);

    const p = skillScan.runScan(null);
    expect(skillScan.scanning.value).toBe(true);
    skillScan.cancelScan();
    expect(cmdMock).toHaveBeenCalledWith("ai_cancel_run", {
      runId: expect.stringContaining("scan-"),
    });
    // 取消语义层后后端仍返回静态部分报告
    const report = makeReport({ llmStatus: "canceled" });
    pending.resolve(report);
    await p;
    expect(skillScan.scanReport.value).toBe(report);
    expect(skillScan.scanning.value).toBe(false);
  });

  it("hydrateCache 命中回填;dispose 后晚到的缓存回填作废", async () => {
    const cached = makeReport();
    const cacheQuery = deferred<ResourceSkillScanReport | null>();
    getCachedMock.mockReturnValue(cacheQuery.promise);
    const skillScan = setup(vi.fn());

    skillScan.hydrateCache();
    expect(getCachedMock).toHaveBeenCalledWith("skill-1", "fp-1");
    skillScan.dispose();
    cacheQuery.resolve(cached);
    await Promise.resolve();
    await Promise.resolve();
    expect(skillScan.scanReport.value).toBeNull();

    // 未 dispose 的正常命中
    getCachedMock.mockResolvedValue(cached);
    skillScan.hydrateCache();
    await vi.waitFor(() => expect(skillScan.scanReport.value).toBe(cached));
  });
});
