// The sessions of the public API (oracle/unit/js_api.py) in Chromium,
// Firefox and WebKit, with the core in the page and in a worker, on the
// browser bundles of dist/ (js/test/shared/api-sessions.mjs). The
// node_only sessions (open(path)) are Node's.
import { expect, test } from "@playwright/test";

import { oracleLines } from "../oracle-data.mjs";

const sessions = (oracleLines("js_api", "sessions") as { in: [{ name: string; node_only: boolean }] }[])
  .map((l) => l.in[0])
  .filter((s) => !s.node_only);

for (const worker of [false, true]) {
  test.describe(worker ? "API sessions, worker mode" : "API sessions, page", () => {
    for (const s of sessions) {
      test(s.name, async ({ page }) => {
        await page.goto("/js/test/browser/page.html");
        await page.waitForFunction(() => (window as unknown as { ready?: boolean }).ready);
        const r = await page.evaluate(
          ([name, w]) =>
            (
              window as unknown as {
                runApiSession(n: string, o: object): Promise<{ ok: boolean; error?: string }>;
              }
            ).runApiSession(name as string, { worker: w }),
          [s.name, worker] as const,
        );
        expect(r.error ?? "").toBe("");
        expect(r.ok).toBe(true);
      });
    }
  });
}
