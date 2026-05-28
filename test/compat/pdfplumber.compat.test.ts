import { describe, it } from "vitest";
import { readGoldens, runScenario } from "./helpers.js";

const shouldRun = process.env.PDFPLUMBER_JS_RUN_COMPAT === "1";
const describeCompat = shouldRun ? describe : describe.skip;
const goldens = readGoldens();

describeCompat("pdfplumber Python compatibility goldens", () => {
  for (const scenario of goldens.scenarios) {
    it(scenario.id, async () => {
      await runScenario(scenario);
    }, 30_000);
  }
});
