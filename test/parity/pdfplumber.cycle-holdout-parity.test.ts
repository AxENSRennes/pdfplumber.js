import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_CYCLE_HOLDOUT_PARITY === "1";
const describeCycleHoldout = shouldRun ? describe : describe.skip;
const cycles = (process.env.PDFPLUMBER_JS_CYCLES ?? "10,11,12")
  .split(",")
  .map((value) => Number.parseInt(value.trim(), 10))
  .filter(Number.isFinite);

describeCycleHoldout("pdfplumber Python real-document cycle holdout corpus", () => {
  for (const cycle of cycles) {
    const cycleGoldenPath = path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${String(cycle).padStart(2, "0")}-holdout.json`);
    const goldens = readParityGoldensAt(cycleGoldenPath);
    for (const scenario of goldens.scenarios) {
      it(scenario.id, async () => {
        await runParityScenario(scenario);
      }, 60_000);
    }
  }
});
