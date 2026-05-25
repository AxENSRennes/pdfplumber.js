import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_CYCLE_WORKING_PARITY === "1";
const describeCycleWorking = shouldRun ? describe : describe.skip;
const cycles = (process.env.PDFPLUMBER_JS_CYCLES ?? "4,5,6")
  .split(",")
  .map((value) => Number.parseInt(value.trim(), 10))
  .filter(Number.isFinite);

describeCycleWorking("pdfplumber Python generated cycle working corpus", () => {
  for (const cycle of cycles) {
    const cycleGoldenPath = path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${String(cycle).padStart(2, "0")}-working.json`);
    const goldens = readParityGoldensAt(cycleGoldenPath);
    for (const scenario of goldens.scenarios) {
      it(scenario.id, async () => {
        await runParityScenario(scenario);
      }, 60_000);
    }
  }
});
