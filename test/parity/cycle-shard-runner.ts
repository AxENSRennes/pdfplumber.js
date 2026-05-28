import path from "node:path";
import { readFileSync } from "node:fs";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario, type ParityScenario } from "./helpers.js";

export type CyclePhase = "working" | "holdout";

export interface CycleParityShardOptions {
  cycle: number;
  phase: CyclePhase;
  shard: string;
  caseIds: string[];
}

const cycleShardTestTimeoutMs = Number.parseInt(process.env.PDFPLUMBER_JS_CYCLE_SHARD_TIMEOUT_MS ?? "180000", 10);
const allowClassifiedHoldoutFailures = process.env.PDFPLUMBER_JS_ALLOW_CLASSIFIED_HOLDOUT_FAILURES === "1";

interface ClassifiedHoldoutFailure {
  id: string;
  cycle: number;
  phase: CyclePhase;
  classification: string;
  messageIncludes: string;
  rationale: string;
}

function cycleLabel(cycle: number): string {
  return String(cycle).padStart(2, "0");
}

function cycleGoldenPath(cycle: number, phase: CyclePhase): string {
  const label = cycleLabel(cycle);
  return path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${label}-${phase}.json`);
}

function classifiedHoldoutFailures(): Map<string, ClassifiedHoldoutFailure> {
  const pathname = path.join(path.dirname(path.dirname(goldenPath)), "parity-cycles", "classified-holdout-failures.json");
  const records = JSON.parse(readFileSync(pathname, "utf8")) as ClassifiedHoldoutFailure[];
  return new Map(records.map((record) => [record.id, record]));
}

function scenariosForIds(scenarios: ParityScenario[], caseIds: string[]): ParityScenario[] {
  const scenarioById = new Map(scenarios.map((scenario) => [scenario.id, scenario]));
  const missing = caseIds.filter((id) => !scenarioById.has(id));
  if (missing.length) {
    throw new Error(`Cycle parity shard references missing golden scenarios: ${missing.join(", ")}`);
  }
  return caseIds.map((id) => scenarioById.get(id)!);
}

export function defineCycleParityShard(options: CycleParityShardOptions): void {
  const label = cycleLabel(options.cycle);
  const goldens = readParityGoldensAt(cycleGoldenPath(options.cycle, options.phase));
  const scenarios = scenariosForIds(goldens.scenarios, options.caseIds);
  const describeCycleShard = process.env.PDFPLUMBER_JS_RUN_CYCLE_SHARDS === "1" ? describe : describe.skip;
  const classifiedFailures = allowClassifiedHoldoutFailures && options.phase === "holdout" ? classifiedHoldoutFailures() : new Map<string, ClassifiedHoldoutFailure>();

  describeCycleShard(`pdfplumber cycle-${label} ${options.phase} ${options.shard}`, () => {
    for (const scenario of scenarios) {
      it(scenario.id, async () => {
        const classified = classifiedFailures.get(scenario.id);
        if (!classified) {
          await runParityScenario(scenario);
          return;
        }

        try {
          await runParityScenario(scenario);
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          if (classified.cycle === options.cycle && classified.phase === options.phase && message.includes(classified.messageIncludes)) {
            console.warn(`[pdfplumber parity] accepted classified holdout miss ${scenario.id}: ${classified.classification}; ${classified.rationale}`);
            return;
          }
          throw error;
        }
        throw new Error(`Classified holdout miss ${scenario.id} now passes; remove or update test/fixtures/parity-cycles/classified-holdout-failures.json.`);
      }, cycleShardTestTimeoutMs);
    }
  });
}
