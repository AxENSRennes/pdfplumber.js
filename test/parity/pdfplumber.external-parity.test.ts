import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_EXTERNAL_PARITY === "1";
const describeExternalParity = shouldRun ? describe : describe.skip;
const externalGoldenPath = path.join(path.dirname(goldenPath), "pdfplumber-external-parity.json");
const goldens = readParityGoldensAt(externalGoldenPath);
const externalParityTimeoutMs = 600_000;

describeExternalParity("pdfplumber Python external PDF parity corpus", () => {
  for (const scenario of goldens.scenarios) {
    it(scenario.id, async () => {
      await runParityScenario(scenario);
    }, externalParityTimeoutMs);
  }
});
