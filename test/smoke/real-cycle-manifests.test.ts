import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../..");
const previousRealCycles = [10, 11] as const;
const cycles = [12, 13, 14] as const;
const expectedCaseCounts = { working: 100, holdout: 20 } as const;
const phases = Object.keys(expectedCaseCounts) as Array<keyof typeof expectedCaseCounts>;
const allowedHoldoutClassifications = new Set([
  "upstream-compatible-limitation",
  "backend-gap",
  "unsupported-feature",
  "intentional-difference",
  "duplicate-coverage",
  "excluded-behavior"
]);

interface CycleManifestEntry {
  id: string;
  localPath: string;
  origin: string;
  corpus: string;
  sourceDocumentId: string;
  sourceManifest: string;
  sourceUrl: string;
  source: string;
  licenseOrTerms: string;
  sha256: string;
  size: number;
  selectedPages: number[];
  pageCount: number;
  metadata: Record<string, unknown>;
  categories: string[];
  rationale: string;
  counted: boolean;
  title?: string | null;
  creator?: string | null;
  producer?: string | null;
}

interface SourceManifestEntry {
  id: string;
  localPath: string;
  sourceUrl: string;
  source: string;
  license: string;
  categories: string[];
  size: number;
  sha256: string;
}

interface ParityGolden {
  scenarios: Array<{
    id: string;
    pdf: string;
    checks: Array<{
      type: string;
      expected?: {
        pages?: Array<Record<string, unknown>>;
      };
    }>;
  }>;
}

interface ObjectListSummary {
  count: number;
  sha256: string;
  sampleIndices: number[];
  samples: unknown[];
}

function cycleLabel(cycle: number): string {
  return String(cycle).padStart(2, "0");
}

function readJson<T>(relativePath: string): T {
  return JSON.parse(fs.readFileSync(path.join(repoRoot, relativePath), "utf8")) as T;
}

function manifestPath(cycle: number, phase: keyof typeof expectedCaseCounts): string {
  return `test/fixtures/parity-cycles/cycle-${cycleLabel(cycle)}/${phase}-manifest.json`;
}

function goldenPath(cycle: number, phase: keyof typeof expectedCaseCounts): string {
  return `test/fixtures/goldens/parity-cycles/pdfplumber-cycle-${cycleLabel(cycle)}-${phase}.json`;
}

function readManifest(cycle: number, phase: keyof typeof expectedCaseCounts): CycleManifestEntry[] {
  return readJson<CycleManifestEntry[]>(manifestPath(cycle, phase));
}

const fileDigestCache = new Map<string, { sha256: string; size: number }>();
const sourceManifestCache = new Map<string, Map<string, SourceManifestEntry>>();
const cycleShardRunnerPath = "test/parity/cycle-shard-runner.ts";
const realCycleRunnerPath = "scripts/run-real-cycle-shards.mjs";
const realCycleGeneratorPath = "scripts/generate-real-parity-cycle-cases.py";
const shardGeneratorPath = "scripts/generate-parity-test-shards.mjs";
const aggregateWorkingPath = "test/parity/pdfplumber.cycle-working-parity.test.ts";
const aggregateHoldoutPath = "test/parity/pdfplumber.cycle-holdout-parity.test.ts";

function fileDigest(relativePath: string): { sha256: string; size: number } {
  let digest = fileDigestCache.get(relativePath);
  if (digest) return digest;
  const absolutePath = path.join(repoRoot, relativePath);
  const bytes = fs.readFileSync(absolutePath);
  digest = {
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    size: bytes.byteLength
  };
  fileDigestCache.set(relativePath, digest);
  return digest;
}

function sourceManifest(relativePath: string): Map<string, SourceManifestEntry> {
  let manifest = sourceManifestCache.get(relativePath);
  if (manifest) return manifest;
  const entries = readJson<SourceManifestEntry[]>(relativePath);
  manifest = new Map(entries.map((entry) => [entry.id, entry]));
  sourceManifestCache.set(relativePath, manifest);
  return manifest;
}

function expectObjectListSummary(value: unknown, label: string): void {
  const summary = value as Partial<ObjectListSummary>;
  expect(summary.count, `${label}.count`).toEqual(expect.any(Number));
  expect(summary.sha256, `${label}.sha256`).toMatch(/^[a-f0-9]{64}$/);
  expect(Array.isArray(summary.sampleIndices), `${label}.sampleIndices`).toBe(true);
  expect(Array.isArray(summary.samples), `${label}.samples`).toBe(true);
  expect(summary.samples?.length, `${label}.sample/sampleIndices length`).toBe(summary.sampleIndices?.length);
}

describe("real-document parity cycle manifests", () => {
  it("record complete counted provenance for the current three-cycle success streak", () => {
    for (const cycle of cycles) {
      for (const phase of phases) {
        const rows = readManifest(cycle, phase);
        const ids = new Set(rows.map((row) => row.id));
        expect(rows, `cycle-${cycleLabel(cycle)} ${phase}`).toHaveLength(expectedCaseCounts[phase]);
        expect(ids.size, `cycle-${cycleLabel(cycle)} ${phase} ids`).toBe(rows.length);

        for (const row of rows) {
          expect(row.id).toMatch(new RegExp(`^cycle-${cycleLabel(cycle)}/${phase}/`));
          expect(row.counted, row.id).toBe(true);
          expect(row.categories, row.id).toEqual(expect.arrayContaining([`cycle-${cycleLabel(cycle)}`, phase, "real-document"]));
          expect(row.categories, row.id).not.toEqual(expect.arrayContaining(["generated", "diagnostic", "micro-pdf"]));
          expect(row.localPath, row.id).toMatch(/^test\/fixtures\/external-(?:holdout-)?pdfs\/.+\.pdf$/);
          expect(fs.existsSync(path.join(repoRoot, row.localPath)), row.id).toBe(true);
          expect(fs.existsSync(path.join(repoRoot, row.sourceManifest)), row.id).toBe(true);
          expect(row.sourceManifest, row.id).toMatch(/^test\/fixtures\/external-(?:holdout-)?pdfs\/manifest\.json$/);
          expect(row.origin, row.id).toBe(new URL(row.sourceUrl).origin);
          expect(row.corpus, row.id).toEqual(expect.any(String));
          expect(row.sourceDocumentId, row.id).toEqual(expect.any(String));
          expect(row.sourceUrl, row.id).toMatch(/^https?:\/\//);
          expect(row.source, row.id).toEqual(expect.any(String));
          expect(row.licenseOrTerms, row.id).toEqual(expect.any(String));
          expect(row.sha256, row.id).toMatch(/^[a-f0-9]{64}$/);
          expect(row.size, row.id).toBeGreaterThan(0);
          expect(fileDigest(row.localPath), row.id).toEqual({ sha256: row.sha256, size: row.size });
          expect(row.pageCount, row.id).toBeGreaterThan(0);
          expect(row.metadata, row.id).toEqual(expect.any(Object));
          expect(row.selectedPages, row.id).toHaveLength(1);
          expect(row.selectedPages[0], row.id).toBeGreaterThanOrEqual(0);
          expect(row.selectedPages[0], row.id).toBeLessThan(row.pageCount);
          expect(row.rationale, row.id).toMatch(/\bpage\b/i);
          expect(Object.prototype.hasOwnProperty.call(row, "title"), row.id).toBe(true);
          expect(Object.prototype.hasOwnProperty.call(row, "creator"), row.id).toBe(true);
          expect(Object.prototype.hasOwnProperty.call(row, "producer"), row.id).toBe(true);
          expect(row.title ?? null, row.id).toBe((row.metadata.Title as string | undefined) ?? null);
          expect(row.creator ?? null, row.id).toBe((row.metadata.Creator as string | undefined) ?? null);
          expect(row.producer ?? null, row.id).toBe((row.metadata.Producer as string | undefined) ?? null);

          const sourceEntry = sourceManifest(row.sourceManifest).get(row.sourceDocumentId);
          expect(sourceEntry, row.id).toBeDefined();
          expect(sourceEntry, row.id).toMatchObject({
            id: row.sourceDocumentId,
            localPath: row.localPath,
            sourceUrl: row.sourceUrl,
            source: row.source,
            sha256: row.sha256,
            size: row.size
          });
          expect(row.corpus, row.id).toBe(sourceEntry?.source);
          expect(row.licenseOrTerms, row.id).toBe(sourceEntry?.license);
        }
      }
    }
  });

  it("keeps working and holdout source documents separate in each cycle", () => {
    for (const cycle of cycles) {
      const working = readManifest(cycle, "working");
      const holdout = readManifest(cycle, "holdout");
      const keys = [
        ["sourceDocumentId", (row: CycleManifestEntry) => row.sourceDocumentId],
        ["localPath", (row: CycleManifestEntry) => row.localPath],
        ["sourceUrl", (row: CycleManifestEntry) => row.sourceUrl],
        ["sha256", (row: CycleManifestEntry) => row.sha256]
      ] as const;

      for (const [label, keyOf] of keys) {
        const workingDocs = new Set(working.map(keyOf));
        const holdoutDocs = new Set(holdout.map(keyOf));
        const overlap = [...workingDocs].filter((sourceDocumentKey) => holdoutDocs.has(sourceDocumentKey));
        expect(overlap, `cycle-${cycleLabel(cycle)} ${label} overlap`).toEqual([]);
      }
    }
  });

  it("keeps counted real-document page cases unique across available real cycles", () => {
    const seen = new Map<string, string>();

    for (const cycle of [...previousRealCycles, ...cycles]) {
      for (const phase of phases) {
        for (const row of readManifest(cycle, phase)) {
          expect(row.counted, row.id).toBe(true);
          expect(row.selectedPages, row.id).toHaveLength(1);
          for (const key of [
            `${row.sourceDocumentId}:${row.selectedPages[0]}`,
            `${row.localPath}:${row.selectedPages[0]}`,
            `${row.sha256}:${row.selectedPages[0]}`
          ]) {
            expect(seen.get(key), `${row.id} duplicates ${key}`).toBeUndefined();
            seen.set(key, row.id);
          }
        }
      }
    }
  });

  it("keeps Python-golden scenarios aligned with manifest cases and structured assertions", () => {
    for (const cycle of cycles) {
      for (const phase of phases) {
        const manifest = readManifest(cycle, phase);
        const golden = readJson<ParityGolden>(goldenPath(cycle, phase));
        const manifestById = new Map(manifest.map((row) => [row.id, row]));
        let tableCheckCount = 0;
        let nonEmptyObjectSnapshotCount = 0;
        expect(golden.scenarios, `cycle-${cycleLabel(cycle)} ${phase} goldens`).toHaveLength(manifest.length);

        for (const scenario of golden.scenarios) {
          const manifestEntry = manifestById.get(scenario.id);
          expect(manifestEntry, scenario.id).toBeDefined();
          expect(scenario.pdf, scenario.id).toBe(manifestEntry?.localPath);
          tableCheckCount += scenario.checks.filter((check) => check.type === "page.findTablesSummary" || check.type === "page.extractTablesSummary").length;
          expect(scenario.checks.map((check) => check.type), scenario.id).toEqual(expect.arrayContaining(["document.snapshot", "page.textSummary", "page.wordsSummary"]));
          const snapshot = scenario.checks.find((check) => check.type === "document.snapshot");
          const expected = snapshot?.expected as Record<string, unknown> | undefined;
          const firstPage = snapshot?.expected?.pages?.[0] as Record<string, unknown> | undefined;
          const geometry = firstPage?.geometry as Record<string, unknown> | undefined;
          const extractText = firstPage?.extractText as Record<string, unknown> | undefined;
          const extractWords = firstPage?.extractWords as Record<string, unknown> | undefined;
          const objectCounts = firstPage?.objectCounts as Record<string, unknown> | undefined;
          const objectSamples = firstPage?.objectSamples as Record<string, unknown> | undefined;
          expect(firstPage, scenario.id).toMatchObject({
            annots: expect.any(Object),
            edgeCounts: expect.any(Object),
            extractText: expect.any(Object),
            extractWords: expect.any(Object),
            geometry: expect.any(Object),
            hyperlinks: expect.any(Object),
            objectCounts: expect.any(Object),
            objectSamples: expect.any(Object)
          });
          expect(expected?.status, scenario.id).toBe("ok");
          expect(expected?.pageIndices, scenario.id).toEqual(manifestEntry?.selectedPages);
          expect(expected?.metadata, scenario.id).toEqual(expect.any(Object));
          expect(manifestEntry?.metadata, scenario.id).toEqual(expected?.metadata);
          expect(expected?.pageCount, scenario.id).toEqual(expect.any(Number));
          expect(Number(expected?.pageCount), scenario.id).toBeGreaterThan(manifestEntry?.selectedPages[0] ?? -1);
          expect(geometry, scenario.id).toMatchObject({
            page_number: (manifestEntry?.selectedPages[0] ?? 0) + 1,
            width: expect.any(Number),
            height: expect.any(Number),
            bbox: expect.any(Array),
            mediabox: expect.any(Array),
            cropbox: expect.any(Array)
          });
          expect(Object.values(objectCounts ?? {}).every((value) => typeof value === "number"), `${scenario.id} objectCounts`).toBe(true);
          if (Object.keys(objectCounts ?? {}).length > 0) nonEmptyObjectSnapshotCount += 1;
          expect(firstPage?.edgeCounts, scenario.id).toMatchObject({
            rect_edges: expect.any(Number),
            curve_edges: expect.any(Number),
            edges: expect.any(Number)
          });
          expectObjectListSummary(firstPage?.annots, `${scenario.id}.annots`);
          expectObjectListSummary(firstPage?.hyperlinks, `${scenario.id}.hyperlinks`);
          expect(extractText?.status, `${scenario.id}.extractText.status`).toBe("ok");
          expect(extractText?.value, `${scenario.id}.extractText.value`).toMatchObject({
            length: expect.any(Number),
            lineCount: expect.any(Number),
            sha256: expect.stringMatching(/^[a-f0-9]{64}$/)
          });
          expect(extractWords?.status, `${scenario.id}.extractWords.status`).toBe("ok");
          expectObjectListSummary(extractWords?.value, `${scenario.id}.extractWords.value`);
          for (const [objectType, summary] of Object.entries(objectSamples ?? {})) {
            expectObjectListSummary(summary, `${scenario.id}.objectSamples.${objectType}`);
          }
        }
        expect(nonEmptyObjectSnapshotCount, `cycle-${cycleLabel(cycle)} ${phase} nonempty object snapshots`).toBeGreaterThan(0);
        expect(tableCheckCount, `cycle-${cycleLabel(cycle)} ${phase} table summary checks`).toBeGreaterThan(0);
      }
    }
  });

  it("classifies at most one holdout miss per cycle and ties it to a counted holdout case", () => {
    const records = readJson<
      Array<{
        id: string;
        cycle: number;
        phase: string;
        subsystem: string;
        classification: string;
        failurePath: string;
        messageIncludes: string;
        rationale: string;
      }>
    >("test/fixtures/parity-cycles/classified-holdout-failures.json");
    const counts = new Map<number, number>();

    for (const record of records) {
      expect(cycles).toContain(record.cycle as (typeof cycles)[number]);
      expect(record.phase).toBe("holdout");
      expect(allowedHoldoutClassifications.has(record.classification), record.id).toBe(true);
      expect(record.subsystem, record.id).toEqual(expect.any(String));
      expect(record.failurePath, record.id).toEqual(expect.any(String));
      expect(record.messageIncludes, record.id).toContain(record.failurePath);
      expect(record.rationale, record.id).toEqual(expect.any(String));
      expect(readManifest(record.cycle, "holdout").some((row) => row.id === record.id), record.id).toBe(true);
      counts.set(record.cycle, (counts.get(record.cycle) ?? 0) + 1);
    }

    for (const cycle of cycles) {
      expect(counts.get(cycle) ?? 0, `cycle-${cycleLabel(cycle)} classified holdout misses`).toBeLessThanOrEqual(1);
    }
  });

  it("keeps classified holdout misses fresh in the shard runner", () => {
    const runner = fs.readFileSync(path.join(repoRoot, cycleShardRunnerPath), "utf8");
    expect(runner).toContain("accepted classified holdout miss");
    expect(runner).toContain("now passes; remove or update test/fixtures/parity-cycles/classified-holdout-failures.json");
  });

  it("keeps real-cycle command defaults on the current three-cycle success streak", () => {
    const pkg = readJson<{ scripts: Record<string, string> }>("package.json");
    const runner = fs.readFileSync(path.join(repoRoot, realCycleRunnerPath), "utf8");
    const realCycleGenerator = fs.readFileSync(path.join(repoRoot, realCycleGeneratorPath), "utf8");
    const shardGenerator = fs.readFileSync(path.join(repoRoot, shardGeneratorPath), "utf8");
    const aggregateWorking = fs.readFileSync(path.join(repoRoot, aggregateWorkingPath), "utf8");
    const aggregateHoldout = fs.readFileSync(path.join(repoRoot, aggregateHoldoutPath), "utf8");

    expect(pkg.scripts["fixtures:generate:real-parity-cycles"]).toContain("--cycles 12,13,14");
    expect(pkg.scripts["fixtures:generate:real-parity-test-shards"]).toContain("--cycles 12,13,14");
    expect(pkg.scripts["test:cycle-working-parity"]).toContain("PDFPLUMBER_JS_CYCLES=12,13,14");
    expect(pkg.scripts["test:cycle-holdout-parity"]).toContain("PDFPLUMBER_JS_CYCLES=12,13,14");
    expect(pkg.scripts["test:cycle-holdout-parity"]).toContain("PDFPLUMBER_JS_ALLOW_CLASSIFIED_HOLDOUT_FAILURES=1");
    expect(pkg.scripts["test:cycle-shards:real:working"]).toContain("--cycles 12,13,14");
    expect(pkg.scripts["test:cycle-shards:real:holdout"]).toContain("--cycles 12,13,14");
    expect(runner).toContain('argValue("--cycles", "12,13,14")');
    expect(runner).toContain("--cycles must be a comma-separated list of cycle numbers");
    expect(realCycleGenerator).toContain('arg_value("--cycles", "12,13,14")');
    expect(realCycleGenerator).toContain('arg_value("--exclude-cycles", "7,8,9,10,11")');
    expect(realCycleGenerator).toContain("Cycle list must contain only comma-separated integers");
    expect(shardGenerator).toContain("--cycles must be a comma-separated list of cycle numbers");
    expect(aggregateWorking).toContain('process.env.PDFPLUMBER_JS_CYCLES ?? "12,13,14"');
    expect(aggregateHoldout).toContain('process.env.PDFPLUMBER_JS_CYCLES ?? "12,13,14"');
    expect(aggregateHoldout).toContain("the real-document cycle rule allows at most 1");
    expect(aggregateHoldout).toContain("now passes; remove or update test/fixtures/parity-cycles/classified-holdout-failures.json");
  });
});
