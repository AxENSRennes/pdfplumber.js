import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

describe("stability gate commands", () => {
  it("keeps the aggregate stability commands tied to the completion gates", () => {
    const pkg = JSON.parse(read("package.json")) as Record<string, any>;
    const scripts = pkg.scripts as Record<string, string>;
    const readme = read("README.md");

    expect(scripts["test:stability:smoke"]).toBe(
      "npm run typecheck && npm test && npm run test:compat && npm run test:parity && npm run contract:dashboard:check && npm run test:browser && npm run package:check && npm run audit:mupdf:check"
    );
    expect(scripts["test:stability:real-cycles"]).toBe(
      "npm run test:cycle-working-parity -- --maxWorkers=1 && npm run test:cycle-holdout-parity -- --maxWorkers=1"
    );
    expect(scripts["test:stability"]).toBe("npm run test:stability:smoke && npm run test:stability:real-cycles");

    for (const scriptName of [
      "typecheck",
      "test",
      "test:compat",
      "test:parity",
      "contract:dashboard:check",
      "test:browser",
      "package:check",
      "audit:mupdf:check",
      "test:cycle-working-parity",
      "test:cycle-holdout-parity"
    ]) {
      expect(scripts[scriptName], scriptName).toEqual(expect.any(String));
    }
    expect(scripts["package:check"]).toBe("node scripts/check-package-payload.mjs");
    expect(scripts["audit:mupdf:check"]).toContain("--check-classifications test/fixtures/mupdf-differential-classifications.json");
    expect(scripts["test:cycle-working-parity"]).toContain("PDFPLUMBER_JS_CYCLES=12,13,14");
    expect(scripts["test:cycle-holdout-parity"]).toContain("PDFPLUMBER_JS_CYCLES=12,13,14");
    expect(scripts["test:cycle-holdout-parity"]).toContain("PDFPLUMBER_JS_ALLOW_CLASSIFIED_HOLDOUT_FAILURES=1");

    for (const command of [
      "npm run test:stability:smoke",
      "npm run test:stability:real-cycles",
      "npm run test:stability",
      "npm run test:compat",
      "npm run test:parity",
      "npm run contract:dashboard:check",
      "npm run package:check",
      "npm run audit:mupdf:check"
    ]) {
      expect(readme, command).toContain(command);
    }
  });
});
