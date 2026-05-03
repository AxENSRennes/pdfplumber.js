import { describe, it } from "vitest";
import { readParityGoldens, runParityScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_PARITY === "1";
const describeParity = shouldRun ? describe : describe.skip;
const goldens = readParityGoldens();

describeParity("pdfplumber Python functional parity corpus", () => {
  for (const scenario of goldens.scenarios) {
    it(scenario.id, async () => {
      await runParityScenario(scenario);
    }, 60_000);
  }
});
