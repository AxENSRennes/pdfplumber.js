import fs from "node:fs/promises";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { open, type OpenOptions } from "../src/index.js";

interface BenchmarkCase {
  name: string;
  pdf: string;
  pages?: number[];
  operations: Array<"objects" | "text" | "words" | "images" | "annotations" | "tables">;
}

interface BenchmarkMetric {
  openMs: number;
  totalMs: number;
  rssDeltaMb: number;
  pageCount: number;
  objectCount: number;
  textLength: number;
  wordCount: number;
  imageCount: number;
  annotationCount: number;
  tableCellCount: number;
}

interface BenchmarkBaseline {
  version: 1;
  thresholds: {
    timeRegressionRatio: number;
    memoryRegressionRatio: number;
    minTimeToleranceMs: number;
    minMemoryToleranceMb: number;
  };
  cases: Record<string, BenchmarkMetric>;
}

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");
const baselinePath = path.join(repoRoot, "test/fixtures/performance/baseline-node.json");

const cases: BenchmarkCase[] = [
  {
    name: "small-pdfplumber-scotus-page-1",
    pdf: "pdfplumber-python/tests/pdfs/scotus-transcript-p1.pdf",
    pages: [1],
    operations: ["objects", "text", "words"]
  },
  {
    name: "real-report-federal-reserve-page-117",
    pdf: "test/fixtures/external-pdfs/federal-reserve-2024-ar.pdf",
    pages: [117],
    operations: ["objects", "text", "words", "images", "annotations"]
  },
  {
    name: "long-federal-register-page-399",
    pdf: "test/fixtures/external-holdout-pdfs/federal-register-2024-07-01.pdf",
    pages: [399],
    operations: ["objects", "text", "words"]
  },
  {
    name: "image-heavy-micro-page-1",
    pdf: "test/fixtures/micro-pdfs/images-advanced.pdf",
    pages: [1],
    operations: ["objects", "images"]
  },
  {
    name: "large-pdfjs-reference-page-1",
    pdf: "test/fixtures/upstream-contract-pdfs/pdfjs-pdf-reference-1-7.pdf",
    pages: [1],
    operations: ["objects", "text", "words"]
  },
  {
    name: "large-pdfjs-reference-page-100",
    pdf: "test/fixtures/upstream-contract-pdfs/pdfjs-pdf-reference-1-7.pdf",
    pages: [100],
    operations: ["objects", "text", "words"]
  },
  {
    name: "real-cycle-table-pubtables-page-6",
    pdf: "test/fixtures/external-pdfs/arxiv-pubtables-2110.00061.pdf",
    pages: [6],
    operations: ["objects", "text", "words", "tables"]
  }
];

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

function rssMb(): number {
  return process.memoryUsage().rss / (1024 * 1024);
}

async function measureCase(item: BenchmarkCase): Promise<BenchmarkMetric> {
  globalThis.gc?.();
  const input = path.join(repoRoot, item.pdf);
  const options: OpenOptions = { pages: item.pages, logLevel: "silent" };
  let peakRss = rssMb();
  const startRss = peakRss;
  const sampler = setInterval(() => {
    peakRss = Math.max(peakRss, rssMb());
  }, 5);
  const start = performance.now();
  const document = await open(input, options);
  const openMs = performance.now() - start;
  let objectCount = 0;
  let textLength = 0;
  let wordCount = 0;
  let imageCount = 0;
  let annotationCount = 0;
  let tableCellCount = 0;
  try {
    for (const page of document.pages) {
      if (item.operations.includes("objects")) objectCount += Object.values(page.objects).reduce((sum, values) => sum + values.length, 0);
      if (item.operations.includes("text")) textLength += String(await page.extractText()).length;
      if (item.operations.includes("words")) wordCount += (await page.extractWords()).length;
      if (item.operations.includes("images")) imageCount += page.images.length;
      if (item.operations.includes("annotations")) annotationCount += page.annots.length;
      if (item.operations.includes("tables")) {
        const table = await page.extractTable();
        tableCellCount += table?.reduce((sum, row) => sum + row.length, 0) ?? 0;
      }
    }
  } finally {
    await document.close();
    clearInterval(sampler);
  }
  const totalMs = performance.now() - start;
  peakRss = Math.max(peakRss, rssMb());
  return {
    openMs: round(openMs),
    totalMs: round(totalMs),
    rssDeltaMb: round(Math.max(0, peakRss - startRss)),
    pageCount: document.pages.length,
    objectCount,
    textLength,
    wordCount,
    imageCount,
    annotationCount,
    tableCellCount
  };
}

function compareMetric(name: string, metric: string, actual: number, expected: number, ratio: number, floor: number): string | null {
  if (expected === 0) return actual <= 1 ? null : `${name}.${metric}: expected near 0, got ${actual}`;
  const limit = Math.max(expected * (1 + ratio), expected + floor);
  return actual <= limit ? null : `${name}.${metric}: ${actual} exceeds ${round(limit)} baseline limit`;
}

function compareExactMetric(name: string, metric: keyof BenchmarkMetric, actual: BenchmarkMetric, expected: BenchmarkMetric): string | null {
  return actual[metric] === expected[metric] ? null : `${name}.${metric}: expected ${expected[metric]}, got ${actual[metric]}`;
}

async function main(): Promise<void> {
  const check = process.argv.includes("--check");
  const results: BenchmarkBaseline = {
    version: 1,
    thresholds: {
      timeRegressionRatio: 0.1,
      memoryRegressionRatio: 0.15,
      minTimeToleranceMs: 25,
      minMemoryToleranceMb: 5
    },
    cases: {}
  };
  for (const item of cases) {
    results.cases[item.name] = await measureCase(item);
  }
  if (!check) {
    console.log(JSON.stringify(results, null, 2));
    return;
  }
  const baseline = JSON.parse(await fs.readFile(baselinePath, "utf8")) as BenchmarkBaseline;
  const failures: string[] = [];
  for (const [name, actual] of Object.entries(results.cases)) {
    const expected = baseline.cases[name];
    if (!expected) {
      failures.push(`${name}: missing baseline case`);
      continue;
    }
    for (const metric of ["openMs", "totalMs"] as const) {
      const failure = compareMetric(name, metric, actual[metric], expected[metric], baseline.thresholds.timeRegressionRatio, baseline.thresholds.minTimeToleranceMs);
      if (failure) failures.push(failure);
    }
    const memoryFailure = compareMetric(
      name,
      "rssDeltaMb",
      actual.rssDeltaMb,
      expected.rssDeltaMb,
      baseline.thresholds.memoryRegressionRatio,
      baseline.thresholds.minMemoryToleranceMb
    );
    if (memoryFailure) failures.push(memoryFailure);
    for (const metric of ["pageCount", "objectCount", "textLength", "wordCount", "imageCount", "annotationCount", "tableCellCount"] as const) {
      const failure = compareExactMetric(name, metric, actual, expected);
      if (failure) failures.push(failure);
    }
  }
  console.log(JSON.stringify(results, null, 2));
  if (failures.length) throw new Error(`Performance baseline failed:\n${failures.join("\n")}`);
}

await main();
