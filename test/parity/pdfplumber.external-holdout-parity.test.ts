import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_EXTERNAL_HOLDOUT_PARITY === "1";
const describeExternalHoldoutParity = shouldRun ? describe : describe.skip;
const externalHoldoutGoldenPath = path.join(path.dirname(goldenPath), "pdfplumber-external-holdout-parity.json");
const goldens = shouldRun ? readParityGoldensAt(externalHoldoutGoldenPath) : { scenarios: [] };
const externalHoldoutParityTimeoutMs = 600_000;

describeExternalHoldoutParity("pdfplumber Python external holdout PDF parity corpus", () => {
  for (const scenario of goldens.scenarios) {
    it(
      scenario.id,
      async () => {
        await runParityScenario(scenario);
      },
      externalHoldoutParityTimeoutMs
    );
  }
});
