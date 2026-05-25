import path from "node:path";
import { describe, it } from "vitest";
import { goldenPath, readParityGoldensAt, runParityScenario, type ParityScenario } from "./helpers.js";

export type CyclePhase = "working" | "holdout";

export interface CycleParityShardOptions {
  cycle: number;
  phase: CyclePhase;
  shard: string;
  caseIds: string[];
}

function cycleLabel(cycle: number): string {
  return String(cycle).padStart(2, "0");
}

function cycleGoldenPath(cycle: number, phase: CyclePhase): string {
  const label = cycleLabel(cycle);
  return path.join(path.dirname(goldenPath), "parity-cycles", `pdfplumber-cycle-${label}-${phase}.json`);
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

  describeCycleShard(`pdfplumber cycle-${label} ${options.phase} ${options.shard}`, () => {
    for (const scenario of scenarios) {
      it(scenario.id, async () => {
        await runParityScenario(scenario);
      }, 60_000);
    }
  });
}
