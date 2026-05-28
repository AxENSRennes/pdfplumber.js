import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const dashboardPath = path.join(repoRoot, "docs/upstream-contract-dashboard/dashboard.tsv");
const readmePath = path.join(repoRoot, "docs/upstream-contract-dashboard/README.md");
const generatorPath = path.join(repoRoot, "scripts/generate-upstream-contract-dashboard.mjs");
const pdfjsManifestPath = "pdfjs/test/test_manifest.json";
const upstreamContractPdfManifestPath = "test/fixtures/upstream-contract-pdfs/manifest.json";
const allowedScopes = new Set(["public-api", "runtime-adaptation", "native-engine", "pdfjs-capability", "robustness-corpus", "duplicate", "excluded"]);
const terminalStatuses = new Set(["passed", "excluded"]);
const pdfjsSpecRoots = ["pdfjs/test/unit", "pdfjs/test/font", "pdfjs/test/integration"] as const;
const pythonTestRoots = ["pdfplumber-python/tests", "pdfminer-six/tests"] as const;
const upstreamFileRoots = [...pythonTestRoots, ...pdfjsSpecRoots] as const;
const directJsTestCallPattern = /\b(?:it|test)\s*(?:\.\w+)?\s*\(\s*(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)/g;

interface PdfjsManifestEntry {
  id: string;
  file: string;
  md5?: string;
}

interface ExternalFixtureEntry {
  localPath: string;
  md5?: string;
}

function dashboardRows(): Array<Record<string, string>> {
  const text = fs.readFileSync(dashboardPath, "utf8").trimEnd();
  const [header, ...lines] = text.split("\n");
  const columns = header.split("\t");
  return lines.map((line) => Object.fromEntries(line.split("\t").map((value, index) => [columns[index], value])));
}

function readmeCount(readme: string, heading: string, value: string): number {
  const section = readme.slice(readme.indexOf(heading));
  const match = section.match(new RegExp(`\\| ${value} \\| (\\d+) \\|`));
  if (!match) throw new Error(`Missing README count for ${heading} ${value}`);
  return Number(match[1]);
}

function walk(relativeDir: string): string[] {
  const absoluteDir = path.join(repoRoot, relativeDir);
  return fs
    .readdirSync(absoluteDir, { withFileTypes: true })
    .flatMap((entry) => {
      const relativePath = path.posix.join(relativeDir, entry.name);
      if (entry.isDirectory()) return walk(relativePath);
      return [relativePath];
    })
    .sort();
}

function lineNumberForOffset(text: string, offset: number): number {
  return text.slice(0, offset).split(/\r?\n/).length;
}

function directPdfjsSpecTestSources(): string[] {
  const sources: string[] = [];
  for (const root of pdfjsSpecRoots) {
    for (const relativePath of walk(root)) {
      if (!/\.(?:js|mjs)$/.test(relativePath)) continue;
      const text = fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
      for (const match of text.matchAll(directJsTestCallPattern)) {
        sources.push(`${relativePath}:${lineNumberForOffset(text, match.index ?? 0)}`);
      }
    }
  }
  return sources;
}

function directPythonTestSources(): string[] {
  const sources: string[] = [];
  for (const root of pythonTestRoots) {
    for (const relativePath of walk(root)) {
      if (!path.basename(relativePath).startsWith("test_") || !relativePath.endsWith(".py")) continue;
      const lines = fs.readFileSync(path.join(repoRoot, relativePath), "utf8").split(/\r?\n/);
      let currentClass = "";
      lines.forEach((line, index) => {
        const classMatch = /^class\s+([A-Za-z_][\w]*)/.exec(line);
        if (classMatch) currentClass = classMatch[1];
        const match = /^(\s*)def\s+(test_[A-Za-z_]\w*)\s*\(/.exec(line);
        const exactTestMatch = /^(\s*)def\s+(test)\s*\(/.exec(line);
        const activeMatch = match ?? (currentClass && exactTestMatch?.[1].length === 4 ? exactTestMatch : null);
        if (activeMatch) sources.push(`${relativePath}:${index + 1}`);
      });
    }
  }
  return sources;
}

function sourceFilePart(source: string): string {
  return source.replace(/[:#].*$/, "");
}

function stringLiteralSetValues(source: string, name: string): string[] {
  const match = source.match(new RegExp(`const ${name} = new Set\\(\\[([\\s\\S]*?)\\]\\);`));
  if (!match) throw new Error(`Missing string-literal Set ${name}`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map((literal) => literal[1]).sort();
}

function objectArrayIds(source: string, name: string): string[] {
  const match = source.match(new RegExp(`const ${name}:[^=]+ = \\[([\\s\\S]*?)\\];`));
  if (!match) throw new Error(`Missing object array ${name}`);
  return [...match[1].matchAll(/\bid:\s*"([^"]+)"/g)].map((literal) => literal[1]).sort();
}

function pdfStringLiterals(source: string): Set<string> {
  return new Set([...source.matchAll(/"([^"]+\.pdf)"/g)].map((literal) => literal[1]));
}

function manifestFixtureCandidates(
  entry: PdfjsManifestEntry,
  externalEntries: ExternalFixtureEntry[]
): string[] {
  const candidates = [`pdfjs/test/${entry.file}`];
  const externalEntry = externalEntries.find((fixture) => fixture.md5 && fixture.md5 === entry.md5);
  if (externalEntry) candidates.push(externalEntry.localPath);
  const externalBasename = `pdfjs-${path.basename(entry.file)}`;
  const externalAlias = externalEntries.find((fixture) => path.basename(fixture.localPath) === externalBasename);
  if (externalAlias) candidates.push(externalAlias.localPath);
  return candidates;
}

describe("upstream contract dashboard artifact", () => {
  it("matches the current upstream inventory generator output", () => {
    expect(() => execFileSync(process.execPath, [generatorPath, "--check"], { cwd: repoRoot, stdio: "pipe" })).not.toThrow();
  });

  it("stays parseable as plain TSV with no hidden merged rows", () => {
    const text = fs.readFileSync(dashboardPath, "utf8").trimEnd();
    const readme = fs.readFileSync(readmePath, "utf8");
    const lines = text.split("\n");
    const [header, ...rows] = lines;
    const columns = header.split("\t");
    const generatedRows = Number(readme.match(/Generated rows: (\d+)/)?.[1]);

    expect(columns).toEqual(["source", "asserted_behavior", "scope", "subsystem", "js_test_or_reason", "status", "rationale"]);
    expect(rows.length).toBe(generatedRows);
    expect(rows.every((line) => line.split("\t").length === columns.length)).toBe(true);
    expect(rows.some((line) => line.includes("\r") || line.includes('"'))).toBe(false);
  });

  it("does not collapse PDF.js spec files with quoted test names into support rows", () => {
    const rows = dashboardRows();
    const rowBySource = new Map(rows.map((row) => [row.source, row]));

    for (const source of [
      "pdfjs/test/unit/canvas_factory_spec.js:31",
      "pdfjs/test/unit/canvas_factory_spec.js:43",
      "pdfjs/test/unit/svg_factory_spec.js:31",
      "pdfjs/test/unit/pdf_viewer_spec.js:32",
      "pdfjs/test/unit/parser_spec.js:235",
      "pdfjs/test/unit/bidi_spec.js:83",
      "pdfjs/test/unit/annotation_spec.js:4103"
    ]) {
      expect(rowBySource.get(source), source).toBeDefined();
    }

    for (const source of [
      "pdfjs/test/unit/canvas_factory_spec.js",
      "pdfjs/test/unit/svg_factory_spec.js",
      "pdfjs/test/unit/pdf_viewer_spec.js"
    ]) {
      expect(rowBySource.get(source), source).toBeUndefined();
    }

    expect(rowBySource.get("pdfjs/test/unit/parser_spec.js:235")?.asserted_behavior).toContain(
      "a non-visible ASCII character (issue 13999)"
    );
    expect(rowBySource.get("pdfjs/test/unit/annotation_spec.js:4136")?.asserted_behavior).toContain(
      "if parent has ReplyType == Group"
    );
  });

  it("contains every direct PDF.js unit/font/integration it()/test() source line", () => {
    const rowSources = new Set(dashboardRows().map((row) => row.source));
    const directTestSources = directPdfjsSpecTestSources();
    const missing = directTestSources.filter((source) => !rowSources.has(source));

    expect(directTestSources.length).toBeGreaterThan(1000);
    expect(missing).toEqual([]);
  });

  it("contains every direct pdfplumber/pdfminer Python test function source line", () => {
    const rowSources = new Set(dashboardRows().map((row) => row.source));
    const directTestSources = directPythonTestSources();
    const missing = directTestSources.filter((source) => !rowSources.has(source));

    expect(directTestSources.length).toBeGreaterThan(300);
    expect(missing).toEqual([]);
  });

  it("contains every PDF.js test manifest entry by stable id", () => {
    const rows = dashboardRows();
    const rowSources = new Set(rows.map((row) => row.source));
    const manifest = JSON.parse(fs.readFileSync(path.join(repoRoot, pdfjsManifestPath), "utf8")) as Array<{ id: string }>;
    const ids = manifest.map((entry) => entry.id);
    const missing = ids.map((id) => `${pdfjsManifestPath}#${id}`).filter((source) => !rowSources.has(source));

    expect(ids).toHaveLength(new Set(ids).size);
    expect(ids.length).toBeGreaterThan(1000);
    expect(missing).toEqual([]);
  });

  it("keeps passed PDF.js load-manifest dashboard rows tied to robustness fixture cases", () => {
    const generator = fs.readFileSync(generatorPath, "utf8");
    const openRobustness = fs.readFileSync(path.join(repoRoot, "test/smoke/open-robustness.test.ts"), "utf8");
    const manifest = JSON.parse(fs.readFileSync(path.join(repoRoot, pdfjsManifestPath), "utf8")) as PdfjsManifestEntry[];
    const externalFixtures = JSON.parse(fs.readFileSync(path.join(repoRoot, upstreamContractPdfManifestPath), "utf8")) as ExternalFixtureEntry[];
    const robustFixturePaths = pdfStringLiterals(openRobustness);
    const passedLoadIds = stringLiteralSetValues(generator, "passedPdfjsManifestLoadRobustnessIds");
    const rowSources = new Set(
      dashboardRows()
        .filter((row) => row.scope === "robustness-corpus" && row.source.startsWith(`${pdfjsManifestPath}#`))
        .map((row) => row.source.slice(row.source.indexOf("#") + 1))
    );

    expect(rowSources).toEqual(new Set(passedLoadIds));

    const missing = passedLoadIds.flatMap((id) => {
      const entry = manifest.find((candidate) => candidate.id === id);
      if (!entry) return [`${id}: missing from ${pdfjsManifestPath}`];
      const candidates = manifestFixtureCandidates(entry, externalFixtures);
      return candidates.some((candidate) => robustFixturePaths.has(candidate))
        ? []
        : [`${id}: expected one of ${candidates.join(", ")}`];
    });

    expect(missing).toEqual([]);
  });

  it("keeps passed PDF.js text-manifest dashboard rows tied to Python-oracle text cases", () => {
    const generator = fs.readFileSync(generatorPath, "utf8");
    const textManifestTest = fs.readFileSync(path.join(repoRoot, "test/lowlevel/pdfjs-text-manifest-compat.test.ts"), "utf8");
    const passedTextIds = stringLiteralSetValues(generator, "passedPdfjsManifestTextPublicIds");
    const testedTextIds = objectArrayIds(textManifestTest, "textManifestCases");
    const rowSources = new Set(
      dashboardRows()
        .filter((row) => row.scope === "pdfjs-capability" && row.source.startsWith(`${pdfjsManifestPath}#`))
        .map((row) => row.source.slice(row.source.indexOf("#") + 1))
    );

    expect(rowSources).toEqual(new Set(passedTextIds));
    expect(testedTextIds).toEqual(passedTextIds);
  });

  it("inventories every file under the upstream contract roots", () => {
    const dashboardFiles = new Set(dashboardRows().map((row) => sourceFilePart(row.source)));
    const upstreamFiles = [...upstreamFileRoots.flatMap((root) => walk(root)), pdfjsManifestPath].sort();
    const missing = upstreamFiles.filter((source) => !dashboardFiles.has(source));

    expect(upstreamFiles.length).toBeGreaterThan(200);
    expect(missing).toEqual([]);
  });

  it("contains only complete contract classifications in allowed scopes", () => {
    const rows = dashboardRows();
    expect(rows).not.toHaveLength(0);
    expect(new Set(rows.map((row) => row.source)).size).toBe(rows.length);

    for (const row of rows) {
      expect(row.source, "source").not.toBe("");
      expect(row.asserted_behavior, row.source).not.toBe("");
      expect(allowedScopes.has(row.scope), row.source).toBe(true);
      expect(row.subsystem, row.source).not.toBe("");
      expect(row.js_test_or_reason, row.source).not.toBe("");
      expect(terminalStatuses.has(row.status), row.source).toBe(true);
      expect(row.rationale, row.source).not.toBe("");
    }
  });

  it("ties passed rows to existing local JS tests", () => {
    for (const row of dashboardRows()) {
      const testRefs = [...row.js_test_or_reason.matchAll(/\btest\/[A-Za-z0-9_./-]+\.(?:test|spec)\.(?:ts|tsx|js|mjs)\b/g)].map((match) => match[0]);
      if (row.status === "passed") {
        expect(testRefs, row.source).not.toHaveLength(0);
      }
      for (const testRef of testRefs) {
        expect(fs.existsSync(path.join(repoRoot, testRef)), `${row.source} -> ${testRef}`).toBe(true);
      }
    }
  });

  it("keeps README summary counts synchronized with dashboard rows", () => {
    const readme = fs.readFileSync(readmePath, "utf8");
    const rows = dashboardRows();
    const byScope = new Map<string, number>();
    const byStatus = new Map<string, number>();

    for (const row of rows) {
      byScope.set(row.scope, (byScope.get(row.scope) ?? 0) + 1);
      byStatus.set(row.status, (byStatus.get(row.status) ?? 0) + 1);
    }

    for (const scope of allowedScopes) {
      expect(readmeCount(readme, "## Rows By Scope", scope)).toBe(byScope.get(scope));
    }
    for (const status of terminalStatuses) {
      expect(readmeCount(readme, "## Rows By Status", status)).toBe(byStatus.get(status));
    }
  });
});
