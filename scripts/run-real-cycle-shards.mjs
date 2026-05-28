import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const vitestBin = path.join(repoRoot, "node_modules/vitest/vitest.mjs");

function argValue(name, fallback) {
  const index = process.argv.indexOf(name);
  return index === -1 ? fallback : process.argv[index + 1];
}

function parseCycles(value) {
  const parts = value.split(",").map((part) => part.trim()).filter(Boolean);
  const cycles = parts.map((part) => Number.parseInt(part, 10));
  if (!cycles.length || cycles.some((cycle, index) => !Number.isFinite(cycle) || String(cycle) !== parts[index])) {
    throw new Error(`--cycles must be a comma-separated list of cycle numbers, received: ${value}`);
  }
  return cycles;
}

function cycleLabel(cycle) {
  return String(cycle).padStart(2, "0");
}

function classifiedHoldoutFailureCounts() {
  const pathname = path.join(repoRoot, "test/fixtures/parity-cycles/classified-holdout-failures.json");
  const records = JSON.parse(readFileSync(pathname, "utf8"));
  const counts = new Map();
  for (const record of records) {
    if (record.phase !== "holdout") continue;
    counts.set(record.cycle, (counts.get(record.cycle) ?? 0) + 1);
  }
  return counts;
}

const phase = argValue("--phase", "working");
if (!["working", "holdout"].includes(phase)) {
  throw new Error("--phase must be working or holdout");
}

const cycles = parseCycles(argValue("--cycles", "12,13,14"));
const maxWorkers = argValue("--maxWorkers", phase === "holdout" ? "1" : "2");
const classifiedCounts = phase === "holdout" ? classifiedHoldoutFailureCounts() : new Map();
for (const cycle of cycles) {
  const count = classifiedCounts.get(cycle) ?? 0;
  if (count > 1) {
    throw new Error(`cycle-${cycleLabel(cycle)} has ${count} classified holdout failures; the real-document cycle rule allows at most 1`);
  }
}
let failed = false;

for (const cycle of cycles) {
  const label = cycleLabel(cycle);
  const testDir = path.join("test/parity/cycles", `cycle-${label}`, phase);
  console.log(`\n=== pdfplumber cycle-${label} ${phase} shards ===`);
  const result = spawnSync(process.execPath, [vitestBin, "run", `--maxWorkers=${maxWorkers}`, testDir], {
    cwd: repoRoot,
    env: {
      ...process.env,
      PDFPLUMBER_JS_RUN_CYCLE_SHARDS: "1",
      ...(phase === "holdout" ? { PDFPLUMBER_JS_ALLOW_CLASSIFIED_HOLDOUT_FAILURES: "1" } : {})
    },
    stdio: "inherit"
  });
  if (result.status !== 0) failed = true;
}

process.exit(failed ? 1 : 0);
