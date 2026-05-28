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

const passedPdfjsManifestLoadRobustnessIds = new Set([
  "arabiccidtruetype-pdf",
  "bug1978317",
  "bug1020858",
  "bug1260585",
  "bug868745",
  "bug867484",
  "bug886717",
  "bug900822",
  "complexttffont-pdf",
  "html5-canvas-cheat-sheet-load",
  "issue11922",
  "issue1249-load",
  "issue1293",
  "issue1586",
  "issue17554",
  "issue18986",
  "issue1985",
  "issue4461-load",
  "issue5599",
  "issue6069",
  "issue6108",
  "issue6151",
  "issue7229",
  "issue7446",
  "issue7665",
  "openoffice-pdf",
  "openofficearabiccidtruetype-pdf",
  "openofficecidtruetype-pdf",
  "scan-bad"
]);

const passedPdfjsManifestTextPublicIds = new Set([
  "arabiccidtruetype-text",
  "bug1245391-text",
  "bug1513120-text",
  "bug1627427",
  "bug1811668",
  "bug1947248-text",
  "bug864847-text",
  "bug900822-encrypted-extract_0",
  "bug946506-text",
  "extgstate-text",
  "IdentityToUnicodeMap_charCodeOf",
  "issue10301",
  "issue1045",
  "issue10529",
  "issue11016",
  "issue11651-text",
  "issue11656",
  "issue11713",
  "issue14048",
  "issue14627",
  "issue15516",
  "issue15629",
  "issue15910",
  "issue16221-text",
  "issue16224-text",
  "issue19800-text",
  "issue19848-text",
  "issue20930-text",
  "issue4665-text",
  "issue4684-text",
  "issue5421-text",
  "issue5734-text",
  "issue5808-text",
  "issue5896-text",
  "issue5972",
  "issue6019-text",
  "issue6342-text",
  "issue6387-text",
  "issue6605",
  "issue6612-text",
  "issue6962",
  "issue7180-text",
  "issue7492-text",
  "issue7878",
  "issue8229",
  "issue8372-text",
  "issue8702-text",
  "issue9655-text",
  "rotated-text",
  "simpletype3font-text",
  "zero_descent"
]);

const classifiedPdfjsManifestTextBackendGaps = new Map([
  ["issue11403-text", "cid-text-normalization"],
  ["issue14415", "layout-ordering"],
  ["issue14497", "layout-ordering"],
  ["issue14999", "layout-ordering"],
  ["issue18059-text", "cid-text-normalization"],
  ["issue18117-text", "malformed-content-recovery"],
  ["issue2017-text", "glyph-decoding"],
  ["issue4550-text", "glyph-decoding"],
  ["issue6901-text", "cid-text-normalization"],
  ["issue7580-text", "glyph-decoding"],
  ["operator-in-TJ-array", "malformed-content-recovery"],
  ["tracemonkey-extract_0_2_12", "glyph-decoding"],
  ["tracemonkey-text", "glyph-decoding"]
]);

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

function passedPublicPageGate() {
  return {
    scope: "public-api",
    subsystem: "parser",
    status: "passed",
    js: "test/smoke/api-shape.test.ts; test/browser/pdfplumber.browser.spec.ts",
    rationale: "The public API smoke test verifies document.pages length, first-page access, page_number, and page boxes in Node; the browser ESM gate verifies the same public page summary in Chromium, Firefox, and WebKit."
  };
}

function passedPdfjsPublicApiGate(subsystem, rationale) {
  return {
    scope: "public-api",
    subsystem,
    status: "passed",
    js: "test/lowlevel/pdfjs-api-public-compat.test.ts",
    rationale
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

  if (sourceFile.endsWith("core_utils_spec.js")) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js core utility helpers are not exposed by pdfplumber.js.",
      rationale: "These rows validate raw PDF.js helper functions for inherited properties, Roman numerals, PDF name/string escaping, URL parsing, font CSS parsing, object equality, and ArrayBuffer concatenation; pdfplumber.js exposes public extraction objects and has separate public/native gates for page labels, metadata strings, fonts, geometry, and parser behavior."
    };
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

  if (sourceFile.endsWith("util_spec.js")) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js generic utility helpers and exception classes are not exposed by pdfplumber.js.",
      rationale: "These rows validate raw PDF.js helpers for PDF string decoding, URL filtering, date formatting, UUIDs, array conversion, string conversion, and exception class construction; pdfplumber.js exposes public extraction behavior and has separate metadata, robustness, and browser input gates."
    };
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

  if (sourceFile.endsWith("pdf_find_controller_spec.js")) {
    return {
      scope: "excluded",
      subsystem: subsystem === "search" || subsystem === "text" ? "search" : "viewer-ui",
      status: "excluded",
      js: "PDF.js viewer find-controller behavior is not exposed by pdfplumber.js; public page.search() is covered by pdfplumber compatibility tests.",
      rationale: "pdfplumber.js implements search over extracted chars/text maps rather than exposing PDF.js viewer find events, match navigation, normalization, or controller state."
    };
  }

  if (sourceFile.endsWith("pdf_find_utils_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js viewer find utility behavior is not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js implements search over extracted chars/text maps rather than exposing PDF.js viewer character-classification helpers."
    };
  }

  if (sourceFile.endsWith("bidi_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js bidi string-reordering helpers are not exposed by pdfplumber.js; public text direction behavior is covered by pdfplumber compatibility tests.",
      rationale: "These rows exercise PDF.js bidi() thresholding and Unicode Bidi test-data reordering. pdfplumber.js exposes pdfplumber-style word/text direction options and geometry-driven ordering through test/compat/pdfplumber.compat.test.ts scenarios text-render-directions, text-rotation, text-rotation-layout, and words-directions-and-extra-attrs."
    };
  }

  if (sourceFile.endsWith("encodings_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js getEncoding() lookup arrays are not exposed by pdfplumber.js; native encoding behavior is covered by pdfminer-backed glyph and CMap tests.",
      rationale: "The public API exposes decoded chars/text rather than raw PDF.js 256-entry encoding tables. pdfminer-compatible Encoding Differences, Adobe glyph names, predefined CMaps, and CMap font extraction are covered by low-level native tests."
    };
  }

  if (sourceFile.endsWith("cff_parser_spec.js") || sourceFile.endsWith("type1_parser_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js CFF/Type1 font-program parser internals are not exposed by pdfplumber.js; public font/text extraction behavior is covered by pdfminer-backed native tests.",
      rationale: "These rows validate raw PDF.js CFF and Type1 parser helpers for headers, indexes, dictionaries, charstrings, charsets, encodings, FDSelect data, compiler output, and Type1 token/header parsing. pdfplumber.js exposes decoded chars/text, font names, widths, and geometry rather than PDF.js font parser classes; pdfminer-compatible glyph, CMap, width, and font-resource behavior is covered by test/lowlevel/strings-glyphs-compat.test.ts, test/lowlevel/cmap-font-compat.test.ts, test/lowlevel/fonts.test.ts, and the public compat/parity suites."
    };
  }

  if (sourceFile.endsWith("cmap_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js CMapFactory, CMap, IdentityCMap, and cMapUrl loader internals are not exposed by pdfplumber.js; public CMap extraction is covered by pdfminer-backed tests.",
      rationale: "These rows validate PDF.js private CMap parser tables, codespace lookup, usecmap attachment, CMapName/WMode properties, built-in bcmap loading, and API-parameter error messages. pdfplumber.js exposes decoded chars/text, font names, writing mode, and geometry rather than PDF.js CMap classes; pdfminer-compatible CMapDB loading, Identity CMap decoding, predefined CMap Unicode mapping, CMap font extraction, ToUnicode extraction, and CMap-name writing mode behavior are covered by test/lowlevel/cmapdb-compat.test.ts, test/lowlevel/strings-glyphs-compat.test.ts, test/lowlevel/fonts.test.ts, test/lowlevel/cmap-font-compat.test.ts, test/lowlevel/to-unicode-compat.test.ts, and high-level Python-golden extraction suites."
    };
  }

  if (sourceFile.endsWith("/parser_spec.js")) {
    return {
      scope: "excluded",
      subsystem: subsystem === "search" ? "parser" : subsystem,
      status: "excluded",
      js: "PDF.js Parser, Lexer, and Linearization helper APIs are not exposed by pdfplumber.js; native parser behavior is covered by pdfminer-backed parser, object, stream, and public extraction tests.",
      rationale: "These rows validate raw PDF.js token cursor state, number/string/name recovery, inline-image EI scanning, and linearization dictionary helpers. pdfplumber.js does not expose those PDF.js classes or linearized streaming hints; it exposes parsed metadata, pages, objects, streams, text, vectors, and annotations. The supported native parsing surface is covered through test/lowlevel/psparser-compat.test.ts, test/lowlevel/pdf-objects.test.ts, test/lowlevel/streams.test.ts, test/lowlevel/content-stream.test.ts, and public Python-golden extraction suites."
    };
  }

  if (sourceFile.endsWith("evaluator_spec.js") && /^should handle invalid dash stuff$/.test(lowerBehavior)) {
    return {
      scope: "native-engine",
      subsystem: "vectors",
      status: "passed",
      js: "test/lowlevel/path-paint-compat.test.ts",
      rationale: "The Python-backed vector extraction test builds a diagnostic PDF with the invalid dash operator pattern from the PDF.js evaluator row and verifies public line dash output against pdfplumber, preserving pdfminer PSKeyword-style dash values rather than PDF.js's normalized empty dash array."
    };
  }

  if (sourceFile.endsWith("evaluator_spec.js") && /^should convert negative line width to absolute value in the graphic state$/.test(lowerBehavior)) {
    return {
      scope: "native-engine",
      subsystem: "vectors",
      status: "passed",
      js: "test/lowlevel/path-paint-compat.test.ts",
      rationale: "The Python-backed vector extraction test builds a diagnostic PDF with a negative line-width operator and verifies public line linewidth output against pdfminer/pdfplumber, preserving pdfminer-compatible signed linewidth rather than PDF.js's absolute-value graphic-state normalization."
    };
  }

  if (sourceFile.endsWith("evaluator_spec.js")) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js PartialEvaluator and OperatorList internals are not exposed by pdfplumber.js; observable extraction behavior is covered by public and native pdfminer-backed tests.",
      rationale: "These rows validate PDF.js private operator splitting, arity recovery, XObject handling, worker aborts, and OperatorList flushing. pdfplumber.js consumes PDF.js operator lists only as an internal aid while exposing pdfplumber-compatible text, chars, vectors, images, marked content, and metadata; retained observable behavior is covered by Python-golden public suites plus native content/vector/color/image/marked-content tests."
    };
  }

  if (sourceFile.endsWith("stream_spec.js") && lowerBehavior === "should decode simple predictor data") {
    return {
      scope: "native-engine",
      subsystem: "streams",
      status: "passed",
      js: "test/lowlevel/streams.test.ts",
      rationale: "The native stream decoder now applies Flate DecodeParms PNG predictors using the pdfminer apply_png_predictor rule, with the adapted test deriving the expected bytes from pdfminer rather than hard-coded PDF.js output."
    };
  }

  if (sourceFile.endsWith("operator_list_dependencies_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js operator-list dependency tracking for rendering debug instrumentation is not exposed by pdfplumber.js.",
      rationale: "This row uses PDF.js StepperManager, page.render(), recordOperations, canvas rendering, and dependency maps for pattern painting; pdfplumber.js exposes extracted geometry/colors rather than renderer operation dependency graphs."
    };
  }

  if (sourceFile.endsWith("colorspace_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "colors",
      status: "excluded",
      js: "PDF.js ColorSpace renderer conversion and cache internals are not exposed by pdfplumber.js; public extracted colors are covered by pdfminer-backed native tests.",
      rationale: "These rows exercise PDF.js ColorSpace.isDefaultDecode(), ColorSpaceUtils.parse() caching, getRgb/fillRgb pixel conversion, calibrated/indexed/alternate renderer conversion, and indirect-object parsing. pdfplumber.js exposes pdfminer-style raw stroking/non-stroking color values and pattern names on extracted chars/vectors through test/lowlevel/color-compat.test.ts, not PDF.js renderer ColorSpace objects or RGB pixel conversion APIs."
    };
  }

  if (sourceFile.endsWith("crypto_spec.js") && /\bpassword\b/.test(lowerBehavior)) {
    return {
      scope: "robustness-corpus",
      subsystem: "security",
      status: "passed",
      js: "test/smoke/open-robustness.test.ts; test/compat/pdfplumber.compat.test.ts (password-open)",
      rationale: "The public gates verify encrypted PDF outcomes through open(): missing and wrong passwords raise stable PasswordException errors, correct PDF.js fixture passwords open, and the Python pdfplumber password fixture extracts through the public API."
    };
  }

  if (sourceFile.endsWith("crypto_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "security",
      status: "excluded",
      js: "Raw PDF.js crypto primitive and CipherTransformFactory APIs are not exposed by pdfplumber.js.",
      rationale: "These rows validate PDF.js MD5/SHA helpers, ARCFour/AES block APIs, PDF 1.7/2.0 key derivation helpers, and internal encrypt/decrypt transform lengths. pdfplumber.js exposes encrypted-document opening and extraction, not standalone crypto classes; native pdfminer-style RC4/AES padding/object decryption coverage lives in test/lowlevel/crypto-compat.test.ts and test/lowlevel/pdfdocument-compat.test.ts."
    };
  }

  if (sourceFile.endsWith("to_unicode_map_spec.js")) {
    return {
      scope: "pdfjs-capability",
      subsystem: "text",
      status: "passed",
      js: "test/lowlevel/to-unicode-compat.test.ts",
      rationale: "The adapted test builds a tiny Type0-font PDF with a ToUnicode CMap mapping one CID to a Unicode Extension B character, then compares public JS extractText() and chars output against Python pdfplumber; this covers the retained ToUnicode high-plane decoding behavior without exposing PDF.js ToUnicodeMap internals."
    };
  }

  if (sourceFile.endsWith("unicode_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js Unicode helper tables are not exposed by pdfplumber.js; public glyph and ToUnicode extraction behavior is covered by pdfminer-backed native tests.",
      rationale: "These rows exercise raw PDF.js helpers for private-use remapping, whitespace/diacritic category flags, glyph-list lookup, and PDF.js Unicode-range table IDs. pdfplumber.js exposes decoded chars/text rather than PDF.js unicode.js helper functions; pdfminer-compatible glyph-name recovery is covered by test/lowlevel/strings-glyphs-compat.test.ts and high-plane ToUnicode extraction is covered by test/lowlevel/to-unicode-compat.test.ts. PDF.js glyph-list semantics intentionally are not treated as the oracle where they differ from pdfminer."
    };
  }

  if (sourceFile.endsWith("ui_utils_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js viewer UI utility helpers are not exposed by pdfplumber.js.",
      rationale: "These rows cover viewer orientation, query parsing, scrolling, visibility, and display-angle helpers; the extraction API exposes page boxes and text objects through separate public tests."
    };
  }

  if (sourceFile.endsWith("annotation_storage_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "annotations",
      status: "excluded",
      js: "PDF.js annotation storage mutation state is not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js extracts existing annotation objects and hyperlinks; it does not expose viewer/editor annotation storage, dirty-state callbacks, or form-value mutation APIs."
    };
  }

  if (sourceFile.endsWith("autolinker_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js viewer autolinker behavior is not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js returns PDF-authored annotations and hyperlinks; it does not synthesize viewer link annotations from visible text."
    };
  }

  if (sourceFile.endsWith("default_appearance_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js annotation default-appearance parsing for rendering/editing is not exposed by pdfplumber.js.",
      rationale: "The extraction API exposes annotation object geometry and simple fields, not PDF.js FreeText/widget appearance rendering or editor appearance serialization."
    };
  }

  if (sourceFile.endsWith("struct_tree_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "marked-content",
      status: "excluded",
      js: "PDF.js full structure-tree traversal is not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes marked-content mcid/tag fields on extracted objects; full accessibility structure trees, table/list role collection, and associated-file MathML traversal are outside the documented public API."
    };
  }

  if (sourceFile.endsWith("pdf_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "general",
      status: "excluded",
      js: "PDF.js package/API self-checks are not exposed by pdfplumber.js.",
      rationale: "These rows verify PDF.js build exports and library constants rather than pdfplumber.js public extraction behavior."
    };
  }

  if (sourceFile.endsWith("pdf.image_decoders_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "images",
      status: "excluded",
      js: "PDF.js image-decoder package/API self-checks are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes image placement and metadata, not the standalone PDF.js image decoder API or its library constants."
    };
  }

  if (sourceFile.endsWith("custom_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "runtime",
      status: "excluded",
      js: "PDF.js DOM font-loading customization hooks are not exposed by pdfplumber.js.",
      rationale: "The browser package is verified through public extraction behavior; callers do not configure PDF.js font loading documents or CSS-rule injection through the pdfplumber.js API."
    };
  }

  if (sourceFile.endsWith("xml_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "metadata",
      status: "excluded",
      js: "PDF.js XML parser tree utilities are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js documents Info metadata extraction and does not expose raw XML/XMP tree parsing, searching, or dumping APIs."
    };
  }

  if (sourceFile.endsWith("metadata_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "metadata",
      status: "excluded",
      js: "PDF.js XMP Metadata and MetadataParser helper APIs are not exposed by pdfplumber.js; public Info metadata is covered by Python-backed API tests.",
      rationale: "These rows exercise standalone PDF.js XMP/XML metadata parsing, repair, iteration, and entity-expansion behavior. The stable public metadata API exposes pdfplumber-compatible Info dictionary values through test/lowlevel/pdfjs-api-public-compat.test.ts, not PDF.js Metadata helper objects."
    };
  }

  if (sourceFile.endsWith("event_utils_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js EventBus and viewer event helpers are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js has no public viewer event layer; browser compatibility is verified through public open() extraction tests."
    };
  }

  if (sourceFile.endsWith("pattern_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js shading/pattern renderer internals are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js preserves extractable color and pattern names on objects, but it does not expose PDF.js Type 1 shading mesh sampling, packed renderer IR, or binary renderer serialization."
    };
  }

  if (sourceFile.endsWith("name_number_tree_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "metadata",
      status: "excluded",
      js: "PDF.js name/number tree helper internals are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js does not expose raw PDF.js NameTree/NumberTree APIs, destinations, or attachment/name-tree traversal as public extraction capabilities."
    };
  }

  if (sourceFile.endsWith("test_utils.js")) {
    return {
      scope: "excluded",
      subsystem: "general",
      status: "excluded",
      js: "PDF.js test harness utilities are not exposed by pdfplumber.js.",
      rationale: "Upstream test helper constants and harness behavior are not runtime extraction behavior."
    };
  }

  if (sourceFile.endsWith("message_handler_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "runtime",
      status: "excluded",
      js: "PDF.js worker message-handler stream internals are not exposed by pdfplumber.js.",
      rationale: "The public runtime contract is browser and Node extraction through open(); raw PDF.js worker transport streams are implementation details."
    };
  }

  if (sourceFile.endsWith("writer_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js PDF writer/save internals are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js is an extraction library and does not expose PDF editing, incremental save, object writing, or AcroForm serialization APIs."
    };
  }

  if (sourceFile.endsWith("obj_bin_transform_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js object binary transform serialization internals are not exposed by pdfplumber.js.",
      rationale: "These rows cover renderer/cache serialization of font, pattern, and path objects rather than public extraction behavior."
    };
  }

  if (sourceFile.endsWith("murmurhash3_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "general",
      status: "excluded",
      js: "PDF.js MurmurHash3 utility internals are not exposed by pdfplumber.js.",
      rationale: "Hash implementation details are not part of the public extraction API or a named retained pdfplumber.js capability."
    };
  }

  if (sourceFile.endsWith("image_utils_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "images",
      status: "excluded",
      js: "PDF.js pixel conversion helpers are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes image placement and metadata such as name, source size, colorspace, and bit depth; it does not expose decoded RGBA pixel conversion."
    };
  }

  if (sourceFile.endsWith("font_substitutions_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "viewer-ui",
      status: "excluded",
      js: "PDF.js browser font substitution tables are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes extracted font names and text geometry, not PDF.js renderer fallback font-family choices for missing or substituted browser fonts."
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

  if (sourceFile.endsWith("postscript_spec.js")) {
    return {
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js PostScript function compiler/evaluator internals are not exposed by pdfplumber.js.",
      rationale: "These rows validate rendering-time PostScript function evaluation and optimization; pdfplumber.js exposes extracted objects and raw color values rather than PDF.js function rendering internals."
    };
  }

  if (sourceFile.endsWith("primitives_spec.js")) {
    return {
      scope: "excluded",
      subsystem: "parser",
      status: "excluded",
      js: "PDF.js primitive object classes are not exposed by pdfplumber.js.",
      rationale: "These rows validate raw PDF.js Dict, Ref, Name, Cmd, cache, and XRef helper APIs; pdfplumber.js exposes public extraction objects and has separate native pdfminer-backed parser tests for its own low-level behavior."
    };
  }

  if (
    sourceFile.endsWith("annotation_spec.js") &&
    /\b(?:render|printing|print|save|compress and save|create a new|update an existing|added|new free|annotation storage|js sandbox|extract the text from a freetext annotation)\b/.test(lowerBehavior)
  ) {
    return {
      scope: "excluded",
      subsystem: /render|printing|print/.test(lowerBehavior) ? "viewer-ui" : "annotations",
      status: "excluded",
      js: "PDF.js annotation rendering, editing, form-save, and sandbox helper behavior is not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes extracted annotation objects and hyperlinks, not PDF.js annotation-layer appearance rendering, editor creation/update flows, saved PDF serialization, or scripting sandbox field objects."
    };
  }

  if (sourceFile.endsWith("annotation_spec.js") && /^should correctly parse a uri action/.test(lowerBehavior)) {
    return passedPdfjsPublicApiGate(
      "annotations",
      "The Python-backed public API test builds URI link annotations for absolute, protocol-less, and UTF-8 literal-string URLs, then verifies JS annots and hyperlinks match Python pdfplumber uri, contents, title, and geometry."
    );
  }

  if (
    sourceFile.endsWith("annotation_spec.js") &&
    /^(?:should set and get valid contents|should set and get a valid rectangle)$/.test(lowerBehavior)
  ) {
    return passedPdfjsPublicApiGate(
      "annotations",
      "The Python-backed public API test verifies authored annotation contents, title, and rectangle-derived geometry through the pdfplumber-shaped annots and hyperlinks objects."
    );
  }

  if (
    sourceFile.endsWith("annotation_spec.js") &&
    /\b(?:goto(?:r)? action|launch action|javascript actions?|named action|simple dest|parse a dest|push buttons|fallback ids?|get id for annotation|missing \/subtype|invalid contents|invalid rectangle|parent properties|inherit properties|inherit contents|group-master)\b/.test(lowerBehavior)
  ) {
    return {
      scope: "excluded",
      subsystem: "annotations",
      status: "excluded",
      js: "PDF.js annotation factory IDs, destinations, viewer actions, widget action URLs, popup inheritance, and internal setter validation are not exposed by pdfplumber.js.",
      rationale: "The public annotation API exposes pdfplumber-compatible geometry plus simple uri/title/contents fields. It does not expose PDF.js annotation IDs, destination/action objects, JavaScript URL recovery, widget button actions, popup reply inheritance internals, or Annotation class setter/defaulting behavior."
    };
  }

  if (
    sourceFile.endsWith("annotation_spec.js") &&
    (/\b(?:quadpoints?|field names?|text alignment|maximum length|comb fields?|checkbox(?:es)?|radio buttons?|option arrays?|form values?|field value|flags|viewable|printable|modification date|creation date|irt|\/rt|state model|state when|color|width|style|dash array|corner radius|line coordinates|line endings|ink lists?|file attachment)\b/.test(lowerBehavior) ||
      /^should ignore (?:non-array values|arrays where the length is not a multiple of eight)$/.test(lowerBehavior) ||
      (lowerBehavior.includes("push buttons") && !lowerBehavior.includes("url")))
  ) {
    return {
      scope: "excluded",
      subsystem: "annotations",
      status: "excluded",
      js: "Detailed PDF.js annotation/form widget properties are not exposed by pdfplumber.js.",
      rationale: "The public annotation API exposes geometry plus simple uri/title/contents fields; it does not expose PDF.js quadpoints, form field state, widget choices, border/style/color/date/reply-state, line/ink detail, or file-attachment payload parsing."
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
    if (/\b(non-existent url|invalid pdf|bad xref|bad \/pages|circular references?|incomplete trailer|bad \/resources|password protected|protected with|empty typedarray)\b/.test(lowerBehavior)) {
      return passedRobustnessGate(subsystem);
    }
    if (/^gets metadata(?:, with custom info dict entries|, with missing pdf header \(bug 1606566\))?$/.test(lowerBehavior)) {
      return passedPdfjsPublicApiGate(
        "metadata",
        "The Python-backed public API test verifies pdfplumber-compatible Info metadata extraction for the PDF.js basic API, custom-info, and missing-header fixtures, including custom keys that PDF.js stores separately from standard info fields."
      );
    }
    if (/^gets metadata, with corrupt \/metadata xref entry$/.test(lowerBehavior)) {
      return {
        scope: "robustness-corpus",
        subsystem: "metadata",
        status: "passed",
        js: "test/lowlevel/pdfjs-api-public-compat.test.ts",
        rationale: "The Python-backed public API test verifies that this corrupt metadata-stream fixture raises the same stable PdfminerException as Python pdfplumber instead of exposing PDF.js metadata recovery internals."
      };
    }
    if (/^gets outline\b/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem: "metadata",
        status: "excluded",
        js: "PDF.js outline/bookmark convenience APIs are not exposed by pdfplumber.js.",
        rationale: "pdfplumber.js documents Info metadata and extraction objects, not raw PDF.js outline, destination, named-action, optional-content, or structure-element bookmark APIs."
      };
    }
    if (/^gets (?:number of pages|page|page number)$/.test(lowerBehavior)) {
      return passedPublicPageGate();
    }
    if (/^gets (?:page index|invalid page index|page multiple time|page index,|non-existent page)/.test(lowerBehavior)) {
      return {
        scope: "excluded",
        subsystem: "parser",
        status: "excluded",
        js: "Raw PDF.js page lookup, page-index, and cache APIs are not exposed by pdfplumber.js.",
        rationale: "pdfplumber.js exposes loaded pages as a public pages array; it does not expose PDF.js getPageIndex, non-existent page errors, or page cache behavior."
      };
    }
    if (/^gets annotations$/.test(lowerBehavior)) {
      return passedPdfjsPublicApiGate(
        "annotations",
        "The Python-backed public API test verifies annotation count, geometry, contents, uri, title, and hyperlink aggregation for the PDF.js basic API fixture through pdfplumber-shaped public objects."
      );
    }
    if (/^gets annotations containing relative urls/.test(lowerBehavior)) {
      return passedPdfjsPublicApiGate(
        "annotations",
        "The Python-backed public API test verifies that PDF.js GoToR unsafeUrl details stay out of pdfplumber-compatible public annotation uri and hyperlink fields."
      );
    }
    if (/^gets annotations containing gotoe action/.test(lowerBehavior)) {
      return passedPdfjsPublicApiGate(
        "annotations",
        "The Python-backed public API test verifies that PDF.js GoToE embedded-file details remain unexposed while the public annotation object matches pdfplumber geometry and hyperlink semantics."
      );
    }
    if (/^gets annotations containing \/launch action with \/filespec dictionary/.test(lowerBehavior)) {
      return passedPdfjsPublicApiGate(
        "annotations",
        "The Python-backed public API test verifies that PDF.js Launch FileSpec unsafeUrl details remain unexposed while the public annotation object matches pdfplumber geometry and hyperlink semantics."
      );
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
      scope: "excluded",
      subsystem,
      status: "excluded",
      js: "PDF.js document-core form-info and idFactory internals are not exposed by pdfplumber.js.",
      rationale: "pdfplumber.js exposes extracted pages, objects, annotations, and metadata; it does not expose PDF.js AcroForm summary APIs, calculation-order arrays, raw field-object arrays, field-action checks, XFA form info, or object/font id factories."
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
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_highlevel_extracttext.py") &&
      (
        lowerBehavior.includes("testextracttext.test simple1 with string") ||
        lowerBehavior.includes("testextracttext.test simple1 no boxes flow") ||
        lowerBehavior.includes("testextracttext.test simple2 with string") ||
        lowerBehavior.includes("testextracttext.test simple3 with string") ||
        lowerBehavior.includes("testextracttext.test simple4 with string") ||
        lowerBehavior.includes("testextracttext.test simple5 with string") ||
        lowerBehavior.includes("testextracttext.test simple1 with file") ||
        lowerBehavior.includes("testextracttext.test simple2 with file") ||
        lowerBehavior.includes("testextracttext.test simple3 with file") ||
        lowerBehavior.includes("testextracttext.test simple4 with file") ||
        lowerBehavior.includes("testextracttext.test simple5 with file") ||
        lowerBehavior.includes("testextracttext.test zlib corrupted") ||
        lowerBehavior.includes("testextracttext.test issue 495 pdfobjref iterable") ||
        lowerBehavior.includes("testextracttext.test issue 566 cmap bytes") ||
        lowerBehavior.includes("testextracttext.test issue 566 cid range") ||
        lowerBehavior.includes("testextracttext.test issue 625 identity cmap") ||
        lowerBehavior.includes("testextracttext.test issue 791 non unicode cmap") ||
        lowerBehavior.includes("ensure that we can support arbitrary width integers in xref streams")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/highlevel-extracttext-compat.test.ts",
        "The low-level native high-level text tests reconstruct pdfminer extract_text output from JS laparams text boxes, compare exact simple fixture output for path and file-style oracles, including predefined vertical Adobe-Japan1 CMaps, and verify the upstream corrupted-zlib, pdfobjref iterable, CMap bytes/ranges, Identity CMap, non-Unicode CMap, and xref-stream-width invariants against live pdfminer.six."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_highlevel_extracttext.py") &&
      (
        lowerBehavior.includes("testextractpages.test line margin") ||
        lowerBehavior.includes("testextractpages.test no boxes flow")
      )
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/layout-compat.test.ts",
        "The low-level native layout tests compare JS laparams text boxes against pdfminer extract_pages output for simple4 line_margin thresholds and boxes_flow=None."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_layout.py") &&
      (lowerBehavior.includes("ltlayoutcontainer.group_textlines() should return all the lines") ||
        lowerBehavior.includes("testfindneigbors.test find neighbors horizontal") ||
        lowerBehavior.includes("testfindneigbors.test find neighbors vertical"))
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/layout-neighbor-compat.test.ts",
        "The low-level native layout neighbor tests compare JS line-neighbor selection and non-clipped line grouping against live pdfminer.six oracles for the synthetic upstream LTTextLine cases."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_layout.py") &&
      lowerBehavior.includes("regression test for issue #449")
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/layout-compat.test.ts",
        "The low-level native layout tests compare JS laparams text boxes against pdfminer extract_pages output for issue-449 horizontal and vertical empty-character fixtures."
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
    if (lowerSourceFile.includes("pdfminer-six/tests/test_pdfminer_ccitt.py")) {
      return passedNativeCompatGate(
        "images",
        "test/lowlevel/ccitt-compat.test.ts",
        "The low-level native CCITT tests compare JS G4 line state transitions and fax output-line byte packing against live pdfminer.six oracles for every upstream CCITTG4Parser and CCITTFaxDecoder case."
      );
    }
    if (
      lowerSourceFile.includes("pdfminer-six/tests/test_pdfminer_psparser.py") &&
      lowerBehavior.includes("token that crosses a")
    ) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/psparser-compat.test.ts",
        "The low-level native PS parser test compares JS keyword tokenization against a live pdfminer.six oracle for a beginbfchar token starting at byte 4093 and crossing pdfminer's 4096-byte parser buffer boundary."
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
    if (lowerSourceFile.includes("pdfminer-six/tests/test_pdffont.py") && lowerBehavior.includes("test if cmap file is read from pdfminer/cmap")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/cmapdb-compat.test.ts",
        "The low-level native CMapDB test verifies pdfminer-compatible loading of the UniGB-UCS2-H cmap asset, restored CMapName metadata, integer CODE2CID keys, writing mode, and byte decoding."
      );
    }
    if (lowerSourceFile.includes("pdfminer-six/tests/test_pdffont.py") && lowerBehavior.includes("test cmap font 12")) {
      return passedNativeCompatGate(
        subsystem,
        "test/lowlevel/cmap-font-compat.test.ts",
        "The low-level native CMap font fixture test compares JS extraction against pdfminer extract_pages output for the issue-598 CMap-font PDF using detect_vertical, char_margin, all_texts, and boxes_flow settings."
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
    const manifestId = source.includes("#") ? source.slice(source.indexOf("#") + 1) : "";
    if (passedPdfjsManifestLoadRobustnessIds.has(manifestId)) {
      return passedRobustnessGate(subsystem);
    }
    if (passedPdfjsManifestTextPublicIds.has(manifestId)) {
      return {
        scope: "pdfjs-capability",
        subsystem: "text",
        status: "passed",
        js: "test/lowlevel/pdfjs-text-manifest-compat.test.ts",
        rationale: "The public extraction API test compares selected PDF.js text/extract manifest fixtures against Python pdfplumber for page dimensions, extracted text, char counts, and word counts."
      };
    }
    if (classifiedPdfjsManifestTextBackendGaps.has(manifestId)) {
      const category = classifiedPdfjsManifestTextBackendGaps.get(manifestId);
      return {
        scope: "pdfjs-capability",
        subsystem: "text",
        status: "backend-gap",
        js: "test/lowlevel/pdfjs-text-manifest-gaps.test.ts",
        rationale: `Classified retained pdf.js text capability mismatch (${category}) against the Python pdfplumber/pdfminer oracle; the diagnostic test records current char/word divergence so the row is no longer unknown, but it remains non-complete until the native text path matches the oracle or the capability is removed.`
      };
    }
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
      scope: "excluded",
      subsystem: "text",
      status: "excluded",
      js: "PDF.js font-test TTX/OpenType sanitizer outputs are not exposed by pdfplumber.js; public font/text extraction is covered by pdfminer-backed tests.",
      rationale: "These rows validate PDF.js test-harness font conversion and OpenType table repair details such as fpgm, OS/2, and post table normalization. pdfplumber.js does not expose generated font binaries or TTX output; the stable API exposes extracted text/chars/font names/geometry, covered by native glyph/font/CMap tests and public Python goldens."
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
  "Rows live in [`dashboard.tsv`](./dashboard.tsv). The generator keeps classifications conservative: rows marked `needs-adapted-js-test`, `needs-classification`, or `backend-gap` are not considered complete contract coverage.",
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
