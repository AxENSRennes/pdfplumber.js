// The sessions of the public API (oracle/unit/js_api.py) in Chromium,
// Firefox and WebKit, with the core in the page and in a worker, on the
// browser bundles of dist/ (js/test/shared/api-sessions.mjs): every session
// of sessions.jsonl but the node_only ones (open(path) is Node's), and the
// depth windows of depth-bytes.jsonl (opened from their bytes), where the
// engines run the Wasm frames on stacks of their own (docs/js-runtime.md,
// "ABI": RangeError, InternalError).
import { expect, test } from "@playwright/test";

import { oracleLines } from "../oracle-data.mjs";

type Line = { in: [{ name: string; node_only: boolean }]; out: unknown[] };
type Outcome = { name: string; ok: boolean; error?: string; checked?: number; made?: number; expected?: number };

const sessions = (oracleLines("js_api", "sessions") as Line[]).map((l) => l.in[0]).filter((s) => !s.node_only);

/** The depth windows, in batches of at most DEPTH_BATCH sessions of one
 * adversarial set (a page load and a parse of depth-bytes.jsonl a batch). */
const DEPTH_BATCH = 25;
const depthBatches: { title: string; names: string[] }[] = [];
{
  const bySet = new Map<string, string[]>();
  for (const l of oracleLines("js_api", "depth-bytes") as Line[]) {
    const s = l.in[0];
    if (s.node_only) throw new Error(`${s.name}: a depth-bytes session is node_only`);
    const set = /^depth-bytes-(adv-[a-z]+)-/.exec(s.name)?.[1] ?? "other";
    if (!bySet.has(set)) bySet.set(set, []);
    bySet.get(set)!.push(s.name);
  }
  for (const [set, names] of bySet) {
    for (let i = 0; i < names.length; i += DEPTH_BATCH) {
      const part = names.slice(i, i + DEPTH_BATCH);
      depthBatches.push({ title: `depth windows ${set} ${i + 1}-${i + part.length}/${names.length}`, names: part });
    }
  }
}

async function run(page: import("@playwright/test").Page, file: string, names: string[], worker: boolean): Promise<Outcome[]> {
  await page.goto("/js/test/browser/page.html");
  await page.waitForFunction(() => (window as unknown as { ready?: boolean }).ready);
  return page.evaluate(
    ([f, n, w]) =>
      (
        window as unknown as {
          runApiSessions(f: string, n: string[], o: object): Promise<Outcome[]>;
        }
      ).runApiSessions(f as string, n as string[], { worker: w }),
    [file, names, worker] as const,
  );
}

function check(outcomes: Outcome[], names: string[]) {
  expect(outcomes.map((o) => o.name)).toEqual(names);
  const failed = outcomes.filter((o) => !o.ok).map((o) => `${o.name}: ${o.error}`);
  expect(failed.join("\n\n")).toBe("");
  for (const o of outcomes) {
    // every expectation compared, and the calls Python made made
    expect(o.checked, o.name).toBe(o.expected);
    expect(o.made, o.name).toBeGreaterThan(0);
  }
}

for (const worker of [false, true]) {
  test.describe(worker ? "API sessions, worker mode" : "API sessions, page", () => {
    for (const s of sessions) {
      test(s.name, async ({ page }) => {
        check(await run(page, "sessions", [s.name], worker), [s.name]);
      });
    }
    for (const b of depthBatches) {
      test(b.title, async ({ page }) => {
        test.setTimeout(900_000);
        check(await run(page, "depth-bytes", b.names, worker), b.names);
      });
    }
  });
}
