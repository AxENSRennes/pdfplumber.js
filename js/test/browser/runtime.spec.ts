// The shared runtime tests (js/test/shared/tests.mjs) in Chromium, Firefox
// and WebKit, with the core in the page and in a worker (worker mode).
import { expect, test } from "@playwright/test";

import { tests } from "../shared/tests.mjs";

for (const worker of [false, true]) {
  test.describe(worker ? "worker mode" : "page", () => {
    for (const t of tests as { name: string; only?: string; todo?: string }[]) {
      if (t.todo) {
        test.fixme(`${t.name} (${t.todo})`, () => {});
        continue;
      }
      if (worker && t.only === "direct") continue;
      test(t.name, async ({ page }) => {
        await page.goto("/js/test/browser/page.html");
        await page.waitForFunction(() => (window as unknown as { ready?: boolean }).ready);
        const r = await page.evaluate(
          ([name, w]) =>
            (window as unknown as { runTest(n: string, o: object): Promise<{ ok: boolean; error?: string }> }).runTest(
              name as string,
              { worker: w },
            ),
          [t.name, worker] as const,
        );
        expect(r.error ?? "").toBe("");
        expect(r.ok).toBe(true);
      });
    }
  });
}
