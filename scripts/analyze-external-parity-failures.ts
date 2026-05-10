#!/usr/bin/env tsx
import { readFileSync } from "node:fs";

interface FailureSummary {
  scenarioId: string;
  checkType: string;
  keyPath: string;
  family: string;
  expected?: string;
  received?: string;
}

function argValue(name: string): string | null {
  const index = process.argv.indexOf(name);
  if (index >= 0) return process.argv[index + 1] ?? null;
  return null;
}

function classify(text: string): string {
  if (/font|cid|unicode|glyph|char|text|words/i.test(text)) return "text/font decoding";
  if (/annot|acroform|widget|uri|popup|checkbox|radio/i.test(text)) return "annotations/forms";
  if (/bbox|x0|x1|y0|y1|top|bottom|doctop|width|height|curve|rect|line|image/i.test(text)) return "geometry/objects";
  if (/metadata|author|title|subject|producer|creator/i.test(text)) return "metadata/pdf strings";
  if (/timeout|timed out|duration/i.test(text)) return "performance/timeout";
  if (/table|extractTables|findTables/i.test(text)) return "tables";
  return "unknown";
}

function truncate(value: string): string {
  const oneLine = value.replace(/\s+/g, " ").trim();
  return oneLine.length > 220 ? `${oneLine.slice(0, 217)}...` : oneLine;
}

function parseFailures(log: string): FailureSummary[] {
  const chunks = log.split(/\n\s*FAIL\s+/).slice(1);
  const failures: FailureSummary[] = [];
  for (const chunk of chunks) {
    const scenarioMatch = chunk.match(/external(?:\/holdout)?\/[A-Za-z0-9_.-]+/);
    const scenarioId = scenarioMatch?.[0] ?? "unknown";
    const checkMatch = chunk.match(/check(?:s)?(?:\[[^\]]+\])?\.type["']?\s*[:=]\s*["']([^"']+)["']/i) ?? chunk.match(/"type":\s*"([^"]+)"/);
    const checkType = checkMatch?.[1] ?? "unknown";
    const pathMatch = chunk.match(/(?:AssertionError|Error):\s*([^\n]+)/) ?? chunk.match(/at\s+([^\n]+)/);
    const keyPath = pathMatch ? truncate(pathMatch[1]) : "unknown";
    const expectedMatch = chunk.match(/Expected:\s*([^\n]+)/i);
    const receivedMatch = chunk.match(/Received:\s*([^\n]+)/i);
    failures.push({
      scenarioId,
      checkType,
      keyPath,
      family: classify(chunk),
      expected: expectedMatch ? truncate(expectedMatch[1]) : undefined,
      received: receivedMatch ? truncate(receivedMatch[1]) : undefined
    });
  }
  return failures;
}

const positionalArgs = process.argv.slice(2).filter((arg) => !arg.startsWith("-"));
const logPath = argValue("--log") ?? positionalArgs[0];
if (!logPath) {
  console.log("Usage: npm run parity:external:analyze -- --log /tmp/external-parity.log");
  console.log("Create a log with: npm run test:external-parity -- --reporter=verbose 2>&1 | tee /tmp/external-parity.log");
  console.log("Holdout logs are for final checkpoints only: npm run parity:holdout:analyze -- --log /tmp/holdout.log");
  process.exit(0);
}

const log = readFileSync(logPath, "utf-8");
const failures = parseFailures(log);
if (!failures.length) {
  console.log("No Vitest failure blocks found in log.");
  process.exit(0);
}

for (const failure of failures) {
  console.log(`${failure.scenarioId}`);
  console.log(`  check: ${failure.checkType}`);
  console.log(`  family: ${failure.family}`);
  console.log(`  first diff: ${failure.keyPath}`);
  if (failure.expected) console.log(`  expected: ${failure.expected}`);
  if (failure.received) console.log(`  received: ${failure.received}`);
}
