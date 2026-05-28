import path from "node:path";
import { readFileSync } from "node:fs";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario, type ParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_CYCLE_HOLDOUT_PARITY === "1";
const describeCycleHoldout = shouldRun ? describe : describe.skip;
const allowClassifiedHoldoutFailures = process.env.PDFPLUMBER_JS_ALLOW_CLASSIFIED_HOLDOUT_FAILURES === "1";

interface ClassifiedHoldoutFailure {
  id: string;
  cycle: number;
  phase: "working" | "holdout";
  classification: string;
  messageIncludes: string;
  rationale: string;
}

function parseCycles(value: string): number[] {
  const parts = value.split(",").map((part) => part.trim()).filter(Boolean);
  const cycles = parts.map((part) => Number.parseInt(part, 10));
  if (!cycles.length || cycles.some((cycle, index) => !Number.isFinite(cycle) || String(cycle) !== parts[index])) {
    throw new Error(`PDFPLUMBER_JS_CYCLES must be a comma-separated list of cycle numbers, received: ${value}`);
  }
  return cycles;
}

function classifiedHoldoutFailures(): Map<string, ClassifiedHoldoutFailure> {
  const pathname = path.join(path.dirname(path.dirname(goldenPath)), "parity-cycles", "classified-holdout-failures.json");
  const records = JSON.parse(readFileSync(pathname, "utf8")) as ClassifiedHoldoutFailure[];
  const requestedCycles = new Set(cycles);
  const counts = new Map<number, number>();
  for (const record of records) {
    if (record.phase !== "holdout" || !requestedCycles.has(record.cycle)) continue;
    counts.set(record.cycle, (counts.get(record.cycle) ?? 0) + 1);
  }
  for (const [cycle, count] of counts) {
    if (count > 1) {
      throw new Error(`cycle-${String(cycle).padStart(2, "0")} has ${count} classified holdout failures; the real-document cycle rule allows at most 1`);
    }
  }
  return new Map(records.map((record) => [record.id, record]));
}

async function runHoldoutScenario(scenario: ParityScenario, cycle: number, classifiedFailures: Map<string, ClassifiedHoldoutFailure>): Promise<void> {
  const classified = classifiedFailures.get(scenario.id);
  if (!classified) {
    await runParityScenario(scenario);
    return;
  }

  try {
    await runParityScenario(scenario);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (classified.cycle === cycle && classified.phase === "holdout" && message.includes(classified.messageIncludes)) {
      console.warn(`[pdfplumber parity] accepted classified holdout miss ${scenario.id}: ${classified.classification}; ${classified.rationale}`);
      return;
    }
    throw error;
  }
  throw new Error(`Classified holdout miss ${scenario.id} now passes; remove or update test/fixtures/parity-cycles/classified-holdout-failures.json.`);
}

const cycles = parseCycles(process.env.PDFPLUMBER_JS_CYCLES ?? "12,13,14");
const classifiedFailures = allowClassifiedHoldoutFailures ? classifiedHoldoutFailures() : new Map<string, ClassifiedHoldoutFailure>();
const cycleParityTestTimeoutMs = Number.parseInt(process.env.PDFPLUMBER_JS_CYCLE_PARITY_TIMEOUT_MS ?? "180000", 10);

describeCycleHoldout("pdfplumber Python real-document cycle holdout corpus", () => {
  for (const cycle of cycles) {
    const cycleGoldenPath = path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${String(cycle).padStart(2, "0")}-holdout.json`);
    const goldens = readParityGoldensAt(cycleGoldenPath);
    for (const scenario of goldens.scenarios) {
      it(scenario.id, async () => {
        await runHoldoutScenario(scenario, cycle, classifiedFailures);
      }, cycleParityTestTimeoutMs);
    }
  }
});
