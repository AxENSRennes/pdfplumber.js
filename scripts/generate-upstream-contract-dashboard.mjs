#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const repoRoot = path.resolve(new URL("..", import.meta.url).pathname);
const outDir = path.join(repoRoot, "docs", "upstream-contract-dashboard");
const rowsPath = path.join(outDir, "dashboard.tsv");
const summaryPath = path.join(outDir, "README.md");

const allowedScopes = new Set([
  "public-api",
  "runtime-adaptation",
  "native-engine",
  "pdfjs-capability",
  "robustness-corpus",
  "duplicate",
  "excluded"
]);

const columns = [
  "source",
  "asserted_behavior",
  "scope",
  "subsystem",
  "js_test_or_reason",
  "status",
  "rationale"
];

function slash(value) {
  return value.split(path.sep).join("/");
}

function rel(file) {
  return slash(path.relative(repoRoot, file));
}

function read(file) {
  return fs.readFileSync(file, "utf8");
}

function walk(dir) {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (entry.name === "__pycache__" || entry.name === "node_modules") continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...walk(full));
    else files.push(full);
  }
  return files.sort();
}

function firstSentence(value) {
  return String(value ?? "")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/\.$/, "");
}

function cleanCell(value) {
  return String(value ?? "")
    .replace(/\r?\n/g, " ")
    .replace(/\t/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function lineNumberForOffset(text, offset) {
  let line = 1;
  for (let i = 0; i < offset; i += 1) {
    if (text.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

function detectSubsystem(source, behavior) {
  const haystack = `${source} ${behavior}`.toLowerCase();
  const checks = [
    ["table", /\b(table|tables|edge|edges|intersection|cell)\b/],
    ["annotations", /\b(annot|annotation|acroform|widget|hyperlink|uri|link)\b/],
    ["marked-content", /\b(mcid|marked|struct|structure|tagged|tag)\b/],
    ["images", /\b(image|jpeg|jpx|jbig|ccitt|bitmap|mask|xobject)\b/],
    ["colors", /\b(color|colorspace|icc|pattern|shading|rgb|cmyk|gray)\b/],
    ["vectors", /\b(line|rect|curve|path|stroke|fill|bezier|dash|graphics)\b/],
    ["text", /\b(text|word|char|glyph|unicode|encoding|cmap|cid|bidi|font|type1|cff|truetype|to_unicode|layout|laparams)\b/],
    ["search", /\b(search|find|regex|match)\b/],
    ["boxes", /\b(box|crop|mediabox|cropbox|rotate|rotation|bbox|viewport)\b/],
    ["metadata", /\b(metadata|info|xmp|outline|destinations|attachments|nametree|catalog)\b/],
    ["security", /\b(crypt|encrypt|password|permission|aes|rc4|security)\b/],
    ["streams", /\b(stream|filter|flate|lzw|runlength|asciihex|predictor|decode)\b/],
    ["parser", /\b(parser|xref|object|primitive|token|literal|number|name|dict|array|document|page)\b/],
    ["runtime", /\b(fetch|network|node|browser|worker|arraybuffer|blob|url|range)\b/],
    ["viewer-ui", /\b(viewer|editor|toolbar|thumbnail|history|sidebar|canvas|svg|text_layer|xfa|scripting|print|download)\b/],
    ["cli", /\b(cli|tool|dumppdf|pdf2txt|command)\b/]
  ];
  for (const [name, pattern] of checks) {
    if (pattern.test(haystack)) return name;
  }
  return "general";
}

function passedBrowserInputGate(subsystem) {
  return {
    scope: "runtime-adaptation",
    subsystem,
    status: "passed",
    js: "test/browser/pdfplumber.browser.spec.ts",
    rationale: "The Playwright browser ESM gate runs Chromium, Firefox, and WebKit against the built package and verifies ArrayBuffer, Blob, and URL inputs against validated Node extraction results."
  };
}

function passedPublicInputGate(subsystem) {
  return {
    scope: "runtime-adaptation",
    subsystem,
    status: "passed",
    js: "test/smoke/api-shape.test.ts; test/browser/pdfplumber.browser.spec.ts",
    rationale: "The public API smoke test verifies Node file-path input, and the Playwright browser ESM gate verifies ArrayBuffer, Blob, URL object, and URL string inputs against the same public extraction summary."
  };
}

function passedPdfplumberCompatGate(subsystem, scenario) {
  return {
    scope: "public-api",
    subsystem,
    status: "passed",
    js: `test/compat/pdfplumber.compat.test.ts (${scenario})`,
    rationale: "The compat gate compares the JS public API against Python pdfplumber-generated goldens for this upstream behavior."
  };
}

const pdfplumberCompatCoveredTests = new Map(
  [
    ["pdfplumber-python/tests/test_ca_warn_report.py", "Test.test page limiting", "pages-option-load"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test metadata", "open-basic-objects-text-and-edges"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test pagecount", "open-basic-objects-text-and-edges"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test page number", "open-basic-objects-text-and-edges"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test objects", "open-basic-objects-text-and-edges"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test annots", "annotations-and-hyperlinks"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test annots cropped", "annotations-cropped"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test annots rotated", "annotations-rotated-*"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test outside bbox", "open-basic-objects-text-and-edges"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test crop and filter", "crop-filter-and-validation"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test relative crop", "crop-filter-and-validation"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test invalid crops", "crop-filter-and-validation"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test rotation", "rotation-page-boxes"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test password", "password-open"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test unicode normalization", "unicode-normalization"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test colors", "basic-colors"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test text colors", "basic-colors"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test load with custom laparams", "laparams-custom-layout"],
    ["pdfplumber-python/tests/test_basics.py", "Test.test uncommon boxes", "uncommon-page-boxes"],
    ["pdfplumber-python/tests/test_ctm.py", "Test.test pdffill demo", "char-ctm-matrix"],
    ["pdfplumber-python/tests/test_dedupe_chars.py", "Test.test extract table", "dedupe-chars-primary"],
    ["pdfplumber-python/tests/test_dedupe_chars.py", "Test.test extract words", "dedupe-chars-primary"],
    ["pdfplumber-python/tests/test_dedupe_chars.py", "Test.test extract text", "dedupe-chars-primary"],
    ["pdfplumber-python/tests/test_dedupe_chars.py", "Test.test extract text2", "dedupe-chars"],
    ["pdfplumber-python/tests/test_dedupe_chars.py", "Test.test extra attrs", "dedupe-extra-attrs"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 53", "table-text-layout"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 140", "table-lines-strict"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 598", "ligatures"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 683", "dedupe-chars"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 1181", "mediabox-offset-table-coordinates"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test without laparams", "laparams-layout-objects-none"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test with laparams", "laparams-layout-objects-default"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test vertical texts", "laparams-layout-objects-vertical"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test issue 383", "laparams-layout-objects-default"],
    ["pdfplumber-python/tests/test_list_metadata.py", "Test.test load", "list-metadata-load"],
    ["pdfplumber-python/tests/test_mcids.py", "TestMCIDs.test mcids", "marked-content-ids"],
    ["pdfplumber-python/tests/test_table.py", "Test.test edges strict", "table-lines-strict"],
    ["pdfplumber-python/tests/test_table.py", "Test.test text tolerance", "table-text-strategy-and-tolerance"],
    ["pdfplumber-python/tests/test_table.py", "Test.test text layout", "table-text-layout"],
    ["pdfplumber-python/tests/test_table.py", "Test.test table curves", "table-curves"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test x tolerance ratio", "x-tolerance-ratio"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words", "words-directions-and-extra-attrs"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words return chars", "extract-words-return-chars"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words punctuation", "punctuation-splitting"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text punctuation", "punctuation-splitting"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test search string", "search-and-text-lines"]
  ].map(([file, behavior, scenario]) => [`${file}|${behavior.toLowerCase()}`, scenario])
);

const excludedPdfplumberUtilityTests = new Set(
  [
    "Test.test cluster list",
    "Test.test cluster objects",
    "Test.test resolve",
    "Test.test resolve all",
    "Test.test decode psl list",
    "Test.test intersects bbox",
    "Test.test merge bboxes",
    "Test.test resize object",
    "Test.test move object",
    "Test.test snap objects",
    "Test.test filter edges",
    "Test.test to list"
  ].map((behavior) => behavior.toLowerCase())
);

function classifyPdfjsUnit(sourceFile, behavior, subsystem) {
  const lowerBehavior = behavior.toLowerCase();

  if (sourceFile.endsWith("api_spec.js")) {
    if (/creates pdf doc from (url-string|url-object|url$|typedarray|arraybuffer)/.test(lowerBehavior)) {
      return passedBrowserInputGate("runtime");
    }
    if (lowerBehavior === "gets current workersrc") {
      return {
        ...passedBrowserInputGate("runtime"),
        rationale: "The browser ESM gate proves the packaged worker source is configured well enough for public extraction in Chromium, Firefox, and WebKit."
      };
    }
    if (/\b(worker|workerport|abort|loadingtask|_worker)\b/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem: "runtime",
        status: "excluded",
        js: "PDF.js worker lifecycle controls are internal implementation details, not exposed pdfplumber.js APIs.",
        rationale: "The public browser contract is covered through open() input tests; raw PDF.js worker APIs are not part of this library."
      };
    }
    if (/\b(destination|destinations|page labels|page layout|page mode|viewer preferences|open action|attachments|javascript|jsactions|permissions|optional content|field objects|calculation order|markinfo|data|download info|stats)\b/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem,
        status: "excluded",
        js: "PDF.js document-proxy convenience API is not exposed by pdfplumber.js.",
        rationale: "pdfplumber.js exposes extraction objects and metadata, not raw PDF.js navigation/viewer/document-management methods."
      };
    }
    if (/\b(non-existent url|invalid pdf|bad xref|bad \/pages|circular references|incomplete trailer|bad \/resources|password protected|protected with|empty typedarray)\b/.test(lowerBehavior)) {
      return {
        scope: "robustness-corpus",
        subsystem,
        status: "needs-adapted-js-test",
        js: "Add a public open() robustness test that either extracts matching behavior or raises the documented stable error.",
        rationale: "Malformed, encrypted, and failed-load API cases matter as stable public open() outcomes rather than as raw PDF.js APIs."
      };
    }
    if (/\b(gets number of pages|gets page\b|gets non-existent page|gets page multiple time|gets page index|gets invalid page index|gets metadata|gets outline|gets annotations|get text content|get operator list)\b/.test(lowerBehavior)) {
      return {
        scope: "pdfjs-capability",
        subsystem,
        status: "needs-adapted-js-test",
        js: "Cover through public extraction tests for page loading, metadata, annotations, text, and operator-derived objects.",
        rationale: "These are PDF.js capabilities currently used underneath pdfplumber.js extraction, so retained use needs explicit public coverage."
      };
    }
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js API behavior is not exposed directly by pdfplumber.js.",
      rationale: "Unexposed PDF.js display/document API behavior is outside the supported public extraction surface unless separately classified as a retained capability."
    };
  }

  if (sourceFile.endsWith("document_spec.js")) {
    return {
      scope: "pdfjs-capability",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Adapt where document parsing behavior is retained by pdfplumber.js; otherwise reclassify rows as excluded or duplicate.",
      rationale: "PDF.js document core behavior matters only when it feeds public extraction behavior."
    };
  }

  if (/node_stream_spec|fetch_stream_spec|network_spec|network_utils_spec/.test(sourceFile)) {
    return {
      scope: "runtime-adaptation",
      subsystem: "runtime",
      status: "needs-adapted-js-test",
      js: "Adapt only for public open() URL/fetch/file behavior in Node and browsers.",
      rationale: "Runtime stream behavior is relevant only through the pdfplumber.js input contract, not through raw PDF.js stream classes."
    };
  }

  return null;
}

function classify(source, behavior, kind) {
  const subsystem = detectSubsystem(source, behavior);
  const lowerSource = source.toLowerCase();
  const lowerSourceFile = lowerSource.replace(/[:#].*$/, "");
  const lowerBehavior = behavior.toLowerCase();

  if (kind === "support") {
    if (/\.(pdf|zip|png|jpg|jpeg|ttf|otf|cff|bcmap|txt|json)$/i.test(source)) {
      return {
        scope: "robustness-corpus",
        subsystem,
        status: "inventory",
        js: "Fixture/support artifact; covered when referenced by an adapted JS test or selected parity-corpus case.",
        rationale: "Upstream fixture retained in the contract inventory so file-backed behavior is not silently omitted."
      };
    }
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "Support code for upstream tests, not a pdfplumber.js runtime behavior.",
      rationale: "Harness helpers and local upstream utilities are not exposed by this library."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_display.py")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "Python display/debug helper behavior is outside the public extraction API.",
      rationale: "The stable goal explicitly excludes PIL/Jupyter/debug display helpers."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_convert.py")) {
    return {
      scope: "excluded",
      subsystem: subsystem === "cli" ? "cli" : "general",
      status: "excluded",
      js: "Python JSON/CSV conversion helpers and CLI output are not exposed by the pdfplumber.js extraction API.",
      rationale: "The supported JS public API returns extraction objects directly; Python-only export helpers and CLI formatting are outside this library's stable surface."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_basics.py")) {
    if (lowerBehavior === "test.test loading pathobj" || lowerBehavior === "test.test loading fileobj") {
      return passedPublicInputGate("runtime");
    }
    if (lowerBehavior === "test.test bad fileobj") {
      return {
        scope: "robustness-corpus",
        subsystem: "runtime",
        status: "needs-adapted-js-test",
        js: "Add a public open() robustness test for empty or invalid PDF inputs that either raises the documented stable error or proves matching extraction behavior.",
        rationale: "The upstream row covers failed file loading plus Python file-object lifetime semantics; JS does not expose Python file objects, but invalid public inputs need a stable open() outcome."
      };
    }
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_repair.py")) {
    return {
      scope: "excluded",
      subsystem: subsystem === "security" ? "security" : "runtime",
      status: "excluded",
      js: "Python Ghostscript repair helpers (`repair=True`, `pdfplumber.repair()`, `gs_path`) are not exposed by the pdfplumber.js extraction API.",
      rationale: "The stable JS API opens and extracts PDFs directly; optional Python/Ghostscript file repair plumbing is a Python-only helper outside this library's public surface."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_structure.py")) {
    return {
      scope: "excluded",
      subsystem: "marked-content",
      status: "excluded",
      js: "Python PDFStructTree, pdf.structure_tree, page.structure_tree, and structure-element search/bbox helpers are not exposed by the pdfplumber.js public API.",
      rationale: "pdfplumber.js exposes marked-content `mcid` and `tag` fields on extracted objects; full tagged-PDF structure tree traversal is outside the documented stable surface and is covered separately only where it affects object extraction."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_oss_fuzz.py")) {
    return {
      scope: "robustness-corpus",
      subsystem: "parser",
      status: "needs-adapted-js-test",
      js: "Add an OSS-Fuzz corpus gate that opens each PDF through the public API and either extracts stable structured data or raises the documented stable error.",
      rationale: "The upstream test is a malformed-PDF robustness harness; Python-only conversion and image helpers it opportunistically calls are not themselves part of the JS public surface."
    };
  }

  if (lowerSourceFile.includes("pdfplumber-python/tests/test_utils.py") && excludedPdfplumberUtilityTests.has(lowerBehavior)) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "Python pdfplumber utility helpers are not exported by the pdfplumber.js package or documented as public extraction APIs.",
      rationale: "These rows exercise internal helper functions such as clustering, object resolving, object geometry transforms, edge filtering, and list coercion; corresponding public extraction behavior remains covered by page/document API tests."
    };
  }

  if (lowerSourceFile.includes("pdfminer-six/tests/test_tools_")) {
    return {
      scope: "excluded",
      subsystem: "cli",
      status: "excluded",
      js: "pdfminer CLI behavior is outside the pdfplumber.js public extraction API.",
      rationale: "The stable goal excludes CLI-only behavior."
    };
  }

  if (lowerSourceFile.startsWith("pdfplumber-python/tests/")) {
    const compatScenario = pdfplumberCompatCoveredTests.get(`${lowerSourceFile}|${lowerBehavior}`);
    if (compatScenario) return passedPdfplumberCompatGate(subsystem, compatScenario);
    return {
      scope: "public-api",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Adapt through test/compat, test/parity, or a targeted public API test backed by Python pdfplumber goldens.",
      rationale: "Python pdfplumber tests are the primary public API oracle for extraction semantics."
    };
  }

  if (lowerSourceFile.startsWith("pdfminer-six/tests/")) {
    return {
      scope: "native-engine",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Adapt through test/lowlevel or a targeted native-engine parity test backed by pdfminer.six behavior.",
      rationale: "pdfminer.six tests define parser, layout, font, stream, and security behavior for the native engine."
    };
  }

  if (lowerSourceFile.startsWith("pdfjs/test/test_manifest.json")) {
    if (/\b(eq|fbf|print|annotation-layer|text-layer)\b/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem: "viewer-ui",
        status: "excluded",
        js: "Rendering/reference-image behavior is not exposed by pdfplumber.js.",
        rationale: "Visual rendering and viewer UI checks are excluded unless the capability is used or exposed by the extraction API."
      };
    }
    if (/\btext\b/.test(lowerBehavior)) {
      return {
        scope: "pdfjs-capability",
        subsystem: "text",
        status: "needs-adapted-js-test",
        js: "Add/verify a JS extraction test only if pdf.js text capability remains a named dependency.",
        rationale: "pdf.js text manifest items are capability checks for the retained pdf.js role, not the native default engine."
      };
    }
    return {
      scope: "robustness-corpus",
      subsystem,
      status: "needs-classification",
      js: "Classify as a robustness input, pdf.js capability test, duplicate, or excluded rendering case.",
      rationale: "Manifest entries are upstream corpus items; non-rendering failures should produce stable errors or targeted behavior tests."
    };
  }

  if (lowerSourceFile.startsWith("pdfjs/test/integration/")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js browser viewer integration behavior is outside the pdfplumber.js public API.",
      rationale: "Integration tests primarily cover viewer UI, visual layers, editors, and browser interactions not exposed by this library."
    };
  }

  if (lowerSourceFile.startsWith("pdfjs/test/unit/")) {
    const pdfjsUnit = classifyPdfjsUnit(lowerSourceFile, behavior, subsystem);
    if (pdfjsUnit) return pdfjsUnit;
    if (["runtime"].includes(subsystem)) {
      return {
        scope: "runtime-adaptation",
        subsystem,
        status: "needs-adapted-js-test",
        js: "Adapt where it affects browser ESM, ArrayBuffer, Blob, URL, worker, or fetch behavior.",
        rationale: "Runtime-facing pdf.js behavior matters only where pdfplumber.js accepts browser and Node inputs."
      };
    }
    if (subsystem === "viewer-ui" || /viewer|editor|history|link_service|text_layer|canvas_factory|svg_factory|xfa|scripting|app_options/.test(lowerSourceFile)) {
      return {
        scope: "excluded",
        subsystem: "viewer-ui",
        status: "excluded",
        js: "PDF.js viewer/editor/layer unit behavior is not a pdfplumber.js extraction API.",
        rationale: "Viewer UI, editor, XFA display, and rendering-layer helpers are outside the supported public API."
      };
    }
    return {
      scope: "pdfjs-capability",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Adapt only for pdf.js capabilities used or exposed by pdfplumber.js; otherwise reclassify as excluded or duplicate.",
      rationale: "The goal allows pdf.js only for named tested roles, so retained capabilities need explicit coverage."
    };
  }

  if (lowerSourceFile.startsWith("pdfjs/test/font/")) {
    if (!lowerSourceFile.endsWith("_spec.js")) {
      return {
        scope: "excluded",
        subsystem: "text",
        status: "excluded",
        js: "PDF.js font-test harness support file, not an exposed extraction behavior.",
        rationale: "Harness assets are retained by upstream but do not define a pdfplumber.js public API item."
      };
    }
    return {
      scope: "pdfjs-capability",
      subsystem: "text",
      status: "needs-adapted-js-test",
      js: "Adapt only if the pdf.js font capability remains a named fallback or differential capability.",
      rationale: "Font parser behavior is a pdf.js capability contract only where pdfplumber.js depends on it."
    };
  }

  return {
    scope: "excluded",
    subsystem,
    status: "needs-classification",
    js: "Unclassified upstream item; refine the dashboard generator.",
    rationale: "Fallback classification used to prevent omission."
  };
}

function makeRow({ source, behavior, kind }) {
  const classification = classify(source, behavior, kind);
  if (!allowedScopes.has(classification.scope)) {
    throw new Error(`Invalid scope ${classification.scope} for ${source}`);
  }
  return {
    source,
    asserted_behavior: behavior,
    scope: classification.scope,
    subsystem: classification.subsystem,
    js_test_or_reason: classification.js,
    status: classification.status,
    rationale: classification.rationale
  };
}

function pythonDocstringAfter(lines, index) {
  for (let i = index + 1; i < Math.min(lines.length, index + 5); i += 1) {
    const trimmed = lines[i].trim();
    if (!trimmed) continue;
    const match = /^(?:[rubfRUBF]*)("""|''')(.+?)(?:\1)?$/.exec(trimmed);
    if (match) return firstSentence(match[2]);
    return "";
  }
  return "";
}

function inventoryPythonTests(root) {
  const files = walk(path.join(repoRoot, root));
  const rows = [];
  for (const file of files) {
    const relative = rel(file);
    if (!relative.endsWith(".py") || !path.basename(relative).startsWith("test_")) {
      rows.push(
        makeRow({
          source: relative,
          behavior: `Upstream support/fixture file: ${path.basename(relative)}`,
          kind: "support"
        })
      );
      continue;
    }

    const text = read(file);
    const lines = text.split(/\r?\n/);
    let currentClass = "";
    let found = 0;
    for (let i = 0; i < lines.length; i += 1) {
      const classMatch = /^class\s+([A-Za-z_][\w]*)/.exec(lines[i]);
      if (classMatch) currentClass = classMatch[1];
      const match = /^(\s*)def\s+(test_[A-Za-z_]\w*)\s*\(/.exec(lines[i]);
      if (!match) continue;
      const indent = match[1].length;
      const name = currentClass && indent > 0 ? `${currentClass}.${match[2]}` : match[2];
      const doc = pythonDocstringAfter(lines, i);
      rows.push(
        makeRow({
          source: `${relative}:${i + 1}`,
          behavior: firstSentence(doc || name.replaceAll("_", " ")),
          kind: "test"
        })
      );
      found += 1;
    }
    if (!found) {
      rows.push(
        makeRow({
          source: relative,
          behavior: `Upstream test/support file with no direct test function inventory: ${path.basename(relative)}`,
          kind: "support"
        })
      );
    }
  }
  return rows;
}

function inventoryPdfjsManifest() {
  const source = "pdfjs/test/test_manifest.json";
  const manifest = JSON.parse(read(path.join(repoRoot, source)));
  return manifest.map((entry) => {
    const pageRange = [entry.firstPage, entry.lastPage].filter((value) => value != null).join("-");
    const bits = [
      entry.id,
      `file=${entry.file}`,
      `type=${entry.type ?? "unspecified"}`,
      entry.rounds ? `rounds=${entry.rounds}` : "",
      entry.link ? "linked" : "",
      pageRange ? `pages=${pageRange}` : "",
      entry.partial ? "partial render region" : ""
    ].filter(Boolean);
    return makeRow({
      source: `${source}#${entry.id}`,
      behavior: bits.join("; "),
      kind: "manifest"
    });
  });
}

function inventoryJsSpecs(root) {
  const dir = path.join(repoRoot, root);
  const files = fs.existsSync(dir) ? walk(dir) : [];
  const rows = [];
  for (const file of files) {
    const relative = rel(file);
    if (!/\.(js|mjs)$/.test(relative)) {
      rows.push(
        makeRow({
          source: relative,
          behavior: `Upstream support/fixture file: ${path.basename(relative)}`,
          kind: "support"
        })
      );
      continue;
    }

    const text = read(file);
    const testPattern = /\b(?:it|test)\s*(?:\.\w+)?\s*\(\s*(["'`])([^"'`]+)\1/g;
    let match;
    let found = 0;
    while ((match = testPattern.exec(text))) {
      rows.push(
        makeRow({
          source: `${relative}:${lineNumberForOffset(text, match.index)}`,
          behavior: firstSentence(match[2]),
          kind: "test"
        })
      );
      found += 1;
    }
    if (!found) {
      rows.push(
        makeRow({
          source: relative,
          behavior: `Upstream JS support/spec file with no direct it()/test() inventory: ${path.basename(relative)}`,
          kind: "support"
        })
      );
    }
  }
  return rows;
}

const rows = [
  ...inventoryPythonTests("pdfplumber-python/tests"),
  ...inventoryPythonTests("pdfminer-six/tests"),
  ...inventoryPdfjsManifest(),
  ...inventoryJsSpecs("pdfjs/test/unit"),
  ...inventoryJsSpecs("pdfjs/test/font"),
  ...inventoryJsSpecs("pdfjs/test/integration")
].sort((a, b) => a.source.localeCompare(b.source));

fs.mkdirSync(outDir, { recursive: true });
const tsv = [columns.join("\t"), ...rows.map((row) => columns.map((column) => cleanCell(row[column])).join("\t"))].join("\n") + "\n";
fs.writeFileSync(rowsPath, tsv);

const byScope = new Map();
const byStatus = new Map();
for (const row of rows) {
  byScope.set(row.scope, (byScope.get(row.scope) ?? 0) + 1);
  byStatus.set(row.status, (byStatus.get(row.status) ?? 0) + 1);
}

function countTable(title, counts) {
  const lines = [`## ${title}`, "", "| Value | Rows |", "| --- | ---: |"];
  for (const [key, value] of [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
    lines.push(`| ${key} | ${value} |`);
  }
  return lines.join("\n");
}

const summary = [
  "# Upstream Contract Dashboard",
  "",
  "This dashboard is generated by `npm run contract:dashboard`. It inventories every current upstream item from:",
  "",
  "- `pdfplumber-python/tests`",
  "- `pdfminer-six/tests`",
  "- `pdfjs/test/test_manifest.json`",
  "- `pdfjs/test/unit`",
  "- `pdfjs/test/font`",
  "- `pdfjs/test/integration`",
  "",
  "Rows live in [`dashboard.tsv`](./dashboard.tsv). The generator keeps classifications conservative: rows marked `needs-adapted-js-test` or `needs-classification` are not considered complete contract coverage.",
  "",
  `Generated rows: ${rows.length}`,
  "",
  countTable("Rows By Scope", byScope),
  "",
  countTable("Rows By Status", byStatus),
  ""
].join("\n");
fs.writeFileSync(summaryPath, summary);

console.log(`Wrote ${rows.length} rows to ${rel(rowsPath)}`);
