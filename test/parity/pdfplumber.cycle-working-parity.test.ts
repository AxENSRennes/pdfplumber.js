import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_CYCLE_WORKING_PARITY === "1";
const describeCycleWorking = shouldRun ? describe : describe.skip;

function parseCycles(value: string): number[] {
  const parts = value.split(",").map((part) => part.trim()).filter(Boolean);
  const cycles = parts.map((part) => Number.parseInt(part, 10));
  if (!cycles.length || cycles.some((cycle, index) => !Number.isFinite(cycle) || String(cycle) !== parts[index])) {
    throw new Error(`PDFPLUMBER_JS_CYCLES must be a comma-separated list of cycle numbers, received: ${value}`);
  }
  return cycles;
}

const cycles = parseCycles(process.env.PDFPLUMBER_JS_CYCLES ?? "12,13,14");
const cycleParityTestTimeoutMs = Number.parseInt(process.env.PDFPLUMBER_JS_CYCLE_PARITY_TIMEOUT_MS ?? "180000", 10);

describeCycleWorking("pdfplumber Python real-document cycle working corpus", () => {
  for (const cycle of cycles) {
    const cycleGoldenPath = path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${String(cycle).padStart(2, "0")}-working.json`);
    const goldens = readParityGoldensAt(cycleGoldenPath);
    for (const scenario of goldens.scenarios) {
      it(scenario.id, async () => {
        await runParityScenario(scenario);
      }, cycleParityTestTimeoutMs);
    }
  }
});
