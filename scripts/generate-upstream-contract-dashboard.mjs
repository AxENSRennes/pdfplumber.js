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

function passedRobustnessGate(subsystem) {
  return {
    scope: "robustness-corpus",
    subsystem,
    status: "passed",
    js: "test/smoke/open-robustness.test.ts",
    rationale: "The public open() robustness gate verifies malformed, missing, encrypted, and OSS-Fuzz PDF inputs either extract stable structured page data or raise a documented named error."
  };
}

function passedNativeCompatGate(subsystem, js, rationale) {
  return {
    scope: "native-engine",
    subsystem,
    status: "passed",
    js,
    rationale
  };
}

const pdfplumberCompatCoveredTests = new Map(
  [
    ["pdfplumber-python/tests/test_ca_warn_report.py", "Test.test page limiting", "pages-option-load"],
    ["pdfplumber-python/tests/test_ca_warn_report.py", "Test.test objects", "ca-warn-objects-and-parse"],
    ["pdfplumber-python/tests/test_ca_warn_report.py", "Test.test parse", "ca-warn-objects-and-parse"],
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
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 13", "issue-13-checkboxes"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 14", "issue-14-objects"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 21", "issue-21-objects"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 33", "issue-33-metadata-present"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 67", "issue-67-metadata-present"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test pr 88", "pr-88-word-count"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 90", "issue-90-extract-words-no-error"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test pr 136", "pr-136-extract-words-no-error"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test pr 138", "pr-138-explicit-lines"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 140", "table-lines-strict"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 203", "issue-203-objects"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 216", "issue-216-empty-crop-table"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 297", "issue-297-integer-metadata"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 316", "issue-316-metadata-changes"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 461 and 842", "issue-461-fontnames + issue-842-fontnames"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 463", "issue-463-annotation-unicode"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 1147", "issue-1147-extract-text"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 982", "issue-982-text-flow"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test pr 1195", "pr-1195-annotation-unicode-errors + pr-1195-annotation-unicode-errors-disabled"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 598", "ligatures"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 683", "dedupe-chars"],
    ["pdfplumber-python/tests/test_issues.py", "Test.test issue 1181", "mediabox-offset-table-coordinates"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test without laparams", "laparams-layout-objects-none"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test with laparams", "laparams-layout-objects-default"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test vertical texts", "laparams-layout-objects-vertical"],
    ["pdfplumber-python/tests/test_laparams.py", "Test.test issue 383", "laparams-layout-objects-default"],
    ["pdfplumber-python/tests/test_list_metadata.py", "Test.test load", "list-metadata-load"],
    ["pdfplumber-python/tests/test_mcids.py", "TestMCIDs.test mcids", "marked-content-ids"],
    ["pdfplumber-python/tests/test_nics_report.py", "Test.test filter", "nics-filter-min-char-size"],
    ["pdfplumber-python/tests/test_nics_report.py", "Test.test edges", "nics-document-edges"],
    ["pdfplumber-python/tests/test_nics_report.py", "Test.test explicit horizontal", "nics-explicit-horizontal"],
    ["pdfplumber-python/tests/test_nics_report.py", "Test.test plain", "nics-plain-table-and-month"],
    ["pdfplumber-python/tests/test_nics_report.py", "Test.test text only strategy", "nics-table-text-only-strategy"],
    ["pdfplumber-python/tests/test_table.py", "Test.test table settings errors", "table-settings-errors"],
    ["pdfplumber-python/tests/test_table.py", "Test.test edges strict", "table-lines-strict"],
    ["pdfplumber-python/tests/test_table.py", "Test.test rows and columns", "table-rows-and-columns"],
    ["pdfplumber-python/tests/test_table.py", "Test.test explicit desc decimalization", "table-explicit-desc-decimalization"],
    ["pdfplumber-python/tests/test_table.py", "Test.test text tolerance", "table-text-strategy-and-tolerance"],
    ["pdfplumber-python/tests/test_table.py", "Test.test text layout", "table-text-layout"],
    ["pdfplumber-python/tests/test_table.py", "Test.test text without words", "table-text-without-words"],
    ["pdfplumber-python/tests/test_table.py", "Test.test order", "table-order"],
    ["pdfplumber-python/tests/test_table.py", "Test.test issue 466 mixed strategy", "table-mixed-strategy-issue-466"],
    ["pdfplumber-python/tests/test_table.py", "Test.test discussion 539 null value", "table-null-value-discussion-539"],
    ["pdfplumber-python/tests/test_table.py", "Test.test table curves", "table-curves"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test x tolerance ratio", "x-tolerance-ratio"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words", "words-directions-and-extra-attrs"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words return chars", "extract-words-return-chars"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text rotation", "text-rotation"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text rotation layout", "text-rotation-layout"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text render directions", "text-render-directions"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test invalid directions", "text-render-directions"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extra attrs", "text-extra-attrs"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract words punctuation", "punctuation-splitting"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text punctuation", "punctuation-splitting"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text flow", "text-flow"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text flow overlapping", "text-flow-overlapping"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test text flow words mixed lines", "text-flow-words-mixed-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text blank", "extract-text-nochars"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text layout", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text layout cropped", "extract-text-layout-cropped"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text layout widths", "extract-text-layout-widths"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text nochars", "extract-text-nochars"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test search regex compiled", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test search regex uncompiled", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test search string", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test extract text lines", "search-and-text-lines"],
    ["pdfplumber-python/tests/test_utils.py", "Test.test handle empty and whitespace search results", "search-and-text-lines"]
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

const excludedPdfplumberInternalHelperTests = new Set([
  "pdfplumber-python/tests/test_ca_warn_report.py|test.test edge merging",
  "pdfplumber-python/tests/test_ca_warn_report.py|test.test vertices",
  "pdfplumber-python/tests/test_issues.py|test.test issue 386",
  "pdfplumber-python/tests/test_table.py|test.test orientation errors"
]);

function classifyPdfjsUnit(sourceFile, behavior, subsystem) {
  const lowerBehavior = behavior.toLowerCase();

  if (sourceFile.endsWith("core_utils_spec.js") && lowerBehavior === "handles one arraybuffer") {
    return passedBrowserInputGate("runtime");
  }

  if (sourceFile.endsWith("pdf.worker_spec.js")) {
    return {
      ...passedBrowserInputGate("runtime"),
      rationale: "The browser ESM gate proves the packaged PDF.js worker bundle is loadable and sufficient for public extraction in Chromium, Firefox, and WebKit."
    };
  }

  if (sourceFile.endsWith("util_spec.js") && lowerBehavior === "correctly creates a valid url for allowed protocols") {
    return passedBrowserInputGate("runtime");
  }

  if (sourceFile.endsWith("node_stream_spec.js")) {
    return passedPublicInputGate("runtime");
  }

  if (/fetch_stream_spec|network_spec/.test(sourceFile)) {
    return {
      scope: "excluded",
      subsystem: "runtime",
      status: "excluded",
      js: "Raw PDF.js network stream and range-reader behavior is not exposed by pdfplumber.js; public URL input is covered by the browser ESM gate.",
      rationale: "pdfplumber.js accepts URLs through open(), but it does not expose PDF.js stream classes, range readers, redirect policy controls, or chunking internals."
    };
  }

  if (/network_utils_spec|display_utils_spec/.test(sourceFile)) {
    return {
      scope: "excluded",
      subsystem: "runtime",
      status: "excluded",
      js: "PDF.js filename, Fetch-protocol, and response-header helper functions are not public pdfplumber.js APIs.",
      rationale: "The stable runtime contract is public open() input handling; low-level PDF.js URL/header helper utilities are not exposed."
    };
  }

  if (sourceFile.endsWith("pdf_link_service_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js link-service viewer helpers are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js returns extraction objects rather than viewer link-service state."
    };
  }

  if (/xfa_parser_spec/.test(sourceFile)) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js XFA parser/display binding behavior is not exposed by pdfplumber.js.",
      rationale: "XFA viewer form binding is outside the supported extraction API."
    };
  }

  if (/annotation_spec|cff_parser_spec|cmap_spec|postscript_spec|primitives_spec/.test(sourceFile)) {
    return {
      scope: "pdfjs-capability",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Adapt only if this PDF.js parser capability remains a named dependency for extraction; otherwise reclassify as excluded or duplicate.",
      rationale: "This is a retained PDF.js internal capability rather than browser/package runtime input behavior."
    };
  }

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
      return passedRobustnessGate(subsystem);
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
      return passedRobustnessGate("runtime");
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
    return passedRobustnessGate("parser");
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

  if (excludedPdfplumberInternalHelperTests.has(`${lowerSourceFile}|${lowerBehavior}`)) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "Python pdfplumber helper functions such as table geometry helpers and pdfplumber.utils.extract_text are not exported by pdfplumber.js or documented as public extraction APIs.",
      rationale: "This upstream row validates direct internal helper behavior; public extraction behavior for the same subsystems is covered by compat scenarios such as ca-warn-objects-and-parse, table-settings-errors, and text extraction tests."
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
    if (lowerSourceFile.includes("pdfminer-six/tests/test_casting.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/geometry-compat.test.ts",
        "The low-level native test verifies pdfminer-compatible safe_float and safe_rect_list casting for numeric, string, null, invalid object, oversized, short, and overlong rectangle inputs."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_cmapdb.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/strings-glyphs-compat.test.ts",
        "The low-level native test verifies pdfminer-compatible IdentityCMap and IdentityCMapByte decoding, including empty, single-byte, odd-length, and maximum unsigned-short buffers."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_cmapdb_security.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/cmapdb-compat.test.ts",
        "The low-level native CMapDB tests verify pdfminer-compatible gzipped JSON CMap loading, recursive CODE2CID integer-key restoration, CID-to-Unicode integer-key restoration, and standard H/Adobe-Japan1 lookup behavior against upstream assets."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_encodingdb.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/strings-glyphs-compat.test.ts; test/lowlevel/fonts.test.ts",
        "The low-level native tests verify pdfminer-compatible Adobe glyph-name conversion rules and that invalid Encoding Differences entries are ignored while valid entries continue to map."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_pdfencoding.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/fonts.test.ts; test/lowlevel/strings-glyphs-compat.test.ts",
        "The low-level native tests verify pdfminer-compatible Type0/CID CMap-name normalization and writing-mode behavior for Identity, DLIdent, OneByteIdentity, H/V stream encodings, literal-string CMapName values, and missing encoding defaults."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_font_size.py")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/font-size-compat.test.ts",
        "The low-level native font-size test verifies that digit-only text lines in the upstream font-size fixture expose character sizes whose rounded values match pdfminer LTChar.size."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_pdfminer_crypto.py")) {
      if (lowerBehavior.includes("arcfour") || lowerBehavior.includes("unpad aes")) {
        return passedNativeCompatGate(
          subsystem,
          "test/lowlevel/crypto-compat.test.ts",
          "The low-level native crypto tests verify pdfminer-compatible RC4 and AES padding behavior used by encrypted stream handling."
        );
      }
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/streams.test.ts",
        "The low-level native stream tests verify pdfminer-compatible ASCII85, ASCIIHex, LZW, and RunLength decoding through PDF stream filters."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_pdffont.py") &&
      (
        lowerBehavior.includes("test pdffont char width defaults") ||
        lowerBehavior.includes("test pdffont get widths") ||
        lowerBehavior.includes("issues/629")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/strings-glyphs-compat.test.ts",
        "The low-level native string/glyph tests verify pdfminer-compatible PDFFont width defaults and get_widths parsing for list, range, and object-reference width definitions."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_converter.py") &&
      (
        lowerBehavior.includes("testpaintpath.") ||
        lowerBehavior.includes("path from samples/contrib/issue-00369-excel.pdf") ||
        lowerBehavior.includes("via https://github.com/pdfminer/pdfminer.six/issues/473") ||
        lowerBehavior.includes("see section 4.4, table 4.9")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/path-paint-compat.test.ts",
        "The low-level native path-painting tests verify pdfminer-compatible path classification for lines, rectangles, curves, quadrilateral edge cases, Bezier endpoints/raw paths, dash style, missing initial move handling, and upstream fixture line widths."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_converter.py") &&
      (
        lowerBehavior.includes("testcolorspace.test do rg") ||
        lowerBehavior.includes("test that pattern color spaces are properly handled") ||
        lowerBehavior.includes("test scn/scn operators with all pattern combinations")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/color-compat.test.ts",
        "The low-level native color tests verify pdfminer-compatible character color-space arity for rg/g output, Pattern color preservation on upstream vectors, and SCN/scn colored and uncolored pattern operand handling."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_converter.py") && lowerBehavior.includes("testbinarydetector.")) {
      return {
        scope: "excluded",
        subsystem,
        status: "excluded",
        js: "pdfminer PDFConverter binary/text output stream detection is Python file-like output plumbing; pdfplumber.js returns structured extraction data and does not expose converter output streams.",
        rationale: "The stable JS API covers browser-capable PDF inputs and extraction objects, while Python converter output stream mode detection is not exposed by this library."
      };
    }
    if (
      (
        lowerSourceFile.includes("pdfminer-six/tests/test_pdfdocument.py") &&
        (lowerBehavior.includes("testpdfdocument.test page labels") || lowerBehavior.includes("testpdfdocument.test no page labels"))
      ) ||
      (
        lowerSourceFile.includes("pdfminer-six/tests/test_pdfpage.py") &&
        lowerBehavior.includes("testpdfpage.test page labels")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/page-labels-compat.test.ts",
        "The low-level native page-label tests verify pdfminer-compatible PageLabels number-tree parsing, missing-label behavior, and public page.label values against the upstream pagelabels fixture."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_pdfdocument.py") &&
      (
        lowerBehavior.includes("testpdfdocument.test get zero objid raises pdfobjectnotfound") ||
        lowerBehavior.includes("testpdfdocument.test encrypted no id") ||
        lowerBehavior.includes("testpdfdocument.test annotations")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/pdfdocument-compat.test.ts",
        "The low-level native PDFDocument tests verify pdfminer-compatible object lookup errors, Standard encryption without trailer IDs, encrypted metadata string decryption, and annotation-page traversal against upstream fixtures."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_utils.py")) {
      if (lowerBehavior.includes("openfilename")) {
        return {
          scope: "excluded",
          subsystem,
          status: "excluded",
          js: "pdfminer open_filename is Python path/file-object wrapper behavior; pdfplumber.js browser-capable input adaptation is covered by public open() runtime tests.",
          rationale: "The stable goal excludes Python-only behavior, while the JS public API separately verifies ArrayBuffer, Blob, URL, and Node file inputs."
        };
      }
      if (
        lowerBehavior.includes("testplane.") ||
        lowerBehavior.includes("shorten") ||
        lowerBehavior.includes("format int alpha") ||
        lowerBehavior.includes("format int roman")
      ) {
        return passedNativeCompatGate(
          subsystem,
          "test/lowlevel/pdfminer-utils-compat.test.ts",
          "The low-level native utility tests verify pdfminer-compatible Plane lookup/removal, string shortening, and alpha/Roman page-label formatting helpers."
        );
      }
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_utils.py") &&
      (
        lowerBehavior === "test mult matrix" ||
        lowerBehavior === "test translate matrix" ||
        lowerBehavior === "test apply matrix pt" ||
        lowerBehavior === "test rotation examples based on pdf reference 4.2.2 common transformations"
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/geometry-compat.test.ts",
        "The low-level native test verifies pdfminer-compatible matrix multiplication, projected translation, point transformation, and outside rectangle transformation for identity, translation, scale, rotation, and skew cases."
      );
    }
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
    if (/\btype=highlight\b/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem: "viewer-ui",
        status: "excluded",
        js: "PDF.js text-highlight rendering/reference behavior is not exposed by pdfplumber.js.",
        rationale: "Highlight manifest checks validate PDF.js viewer/text-layer rendering, not the pdfplumber.js extraction API."
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
    if (/\btype=extract\b/.test(lowerBehavior)) {
      return {
        scope: "pdfjs-capability",
        subsystem: "text",
        status: "needs-adapted-js-test",
        js: "Adapt only if PDF.js text extraction remains a named fallback/capability; otherwise reclassify as excluded or duplicate.",
        rationale: "PDF.js extract manifest rows exercise its text extraction harness, which is a retained-capability concern rather than the default native extraction oracle."
      };
    }
    if (/\btype=(?:load|other)\b/.test(lowerBehavior)) {
      return {
        scope: "robustness-corpus",
        subsystem,
        status: "needs-adapted-js-test",
        js: "Select for a public open() robustness corpus gate or mark as duplicate of an existing real-document/parity case.",
        rationale: "PDF.js load/other manifest rows are upstream corpus inputs; each should either open/extract with stable public behavior, raise a documented stable error, or be linked to duplicate corpus coverage."
      };
    }
    return {
      scope: "robustness-corpus",
      subsystem,
      status: "needs-adapted-js-test",
      js: "Classify with a targeted public extraction, robustness, duplicate, or exclusion rule.",
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
      const exactTestMatch = /^(\s*)def\s+(test)\s*\(/.exec(lines[i]);
      const activeMatch = match ?? (currentClass && exactTestMatch?.[1].length === 4 ? exactTestMatch : null);
      if (!activeMatch) continue;
      const indent = activeMatch[1].length;
      const name = currentClass && indent > 0 ? `${currentClass}.${activeMatch[2]}` : activeMatch[2];
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
