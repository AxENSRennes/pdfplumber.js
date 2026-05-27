import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const vitestBin = path.join(repoRoot, "node_modules/vitest/vitest.mjs");

function argValue(name, fallback) {
  const index = process.argv.indexOf(name);
  return index === -1 ? fallback : process.argv[index + 1];
}

function parseCycles(value) {
  return value
    .split(",")
    .map((part) => Number.parseInt(part.trim(), 10))
    .filter(Number.isFinite);
}

function cycleLabel(cycle) {
  return String(cycle).padStart(2, "0");
}

const phase = argValue("--phase", "working");
if (!["working", "holdout"].includes(phase)) {
  throw new Error("--phase must be working or holdout");
}

const cycles = parseCycles(argValue("--cycles", "10,11,12,13"));
const maxWorkers = argValue("--maxWorkers", phase === "holdout" ? "1" : "2");
let failed = false;

for (const cycle of cycles) {
  const label = cycleLabel(cycle);
  const testDir = path.join("test/parity/cycles", `cycle-${label}`, phase);
  console.log(`\n=== pdfplumber cycle-${label} ${phase} shards ===`);
  const result = spawnSync(process.execPath, [vitestBin, "run", `--maxWorkers=${maxWorkers}`, testDir], {
    cwd: repoRoot,
    env: { ...process.env, PDFPLUMBER_JS_RUN_CYCLE_SHARDS: "1" },
    stdio: "inherit"
  });
  if (result.status !== 0) failed = true;
}

process.exit(failed ? 1 : 0);
