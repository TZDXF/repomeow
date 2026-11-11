import { onScopeDispose, reactive, watch } from "vue";

export type CliRevisions = Record<string, string>;
const revisions = reactive<CliRevisions>({});

export function applyCliRevisions(next: CliRevisions) {
  for (const [domain, revision] of Object.entries(next)) {
    revisions[domain] = revision;
  }
}

/** Serialize refreshes, coalesce bursts, and retry failed reads. A change arriving
 * during a read always causes another read, so an old response cannot win last. */
export function onCliDataChanged(domains: string[], refresh: () => unknown | Promise<unknown>) {
  let pending = false,
    running = false,
    disposed = false,
    retry: ReturnType<typeof setTimeout> | undefined;
  async function drain() {
    if (running || disposed) {
      return;
    }
    running = true;
    while (pending) {
      if (disposed) {
        break;
      }
      pending = false;
      try {
        await refresh();
      } catch {
        pending = true;
        retry = setTimeout(() => void drain(), 1000);
        break;
      }
    }
    running = false;
  }
  watch(
    () => domains.map((domain) => revisions[domain] ?? "").join("\n"),
    () => {
      pending = true;
      clearTimeout(retry);
      void drain();
    },
  );
  onScopeDispose(() => {
    disposed = true;
    clearTimeout(retry);
  });
}
