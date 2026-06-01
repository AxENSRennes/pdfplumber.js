# Performance Gates

pdfplumber.js includes explicit performance gates for large-file behavior,
page-selective extraction, warning control, and browser parity.

## Node Benchmark

Run:

```sh
npm run perf:benchmark
npm run perf:check
```

`perf:benchmark` prints fresh measurements. `perf:check` compares the current
run against `test/fixtures/performance/baseline-node.json`.

The Node gate covers:

- a small pdfplumber fixture,
- a medium real annual report page,
- a late page in a long Federal Register PDF,
- an image-heavy fixture,
- the 31 MB PDF.js reference PDF on pages 1 and 100,
- a real table-heavy page from the parity corpus.

Time may regress by at most 10% and RSS delta by at most 15% from the committed
baseline, with small absolute floors of 25 ms and 5 MB to avoid flaky failures
on tiny fixtures. Public structured counts must match exactly.

## Browser Benchmark

Run:

```sh
npm run perf:browser
```

The browser gate runs the built ESM package in Chromium, Firefox, and WebKit.
It covers ArrayBuffer, Blob, and URL inputs, including selected pages from the
31 MB PDF.js reference PDF. Each browser compares extraction behavior and time
against `test/fixtures/performance/baseline-browser.json`.

## Page Selection

`open(input, { pages })` is page-selective. Unselected pages may be inspected
through lightweight page-tree metadata to compute pdfplumber-compatible
`doctop` offsets, but text content, operator lists, annotations, image
metadata, layout, and table inputs are loaded only for selected pages.

## Warning Policy

PDF.js warnings are silent by default. Use `logLevel: "warning"` or
`logLevel: "debug"` to opt into console output, or `onWarning` to collect
structured warnings without changing extraction results.

## Current Large-PDF Result

The 31 MB PDF.js reference PDF now uses the selected-page path in both tests and
benchmarks. The committed Node baseline records selected page RSS deltas of
about 163 MB for page 1 and 94 MB for page 100, replacing the previous
full-document smoke behavior that could exceed 1 GB on first open.
