/** Browser entry point of pdfplumber-js (`dist/browser/index.js`). */

import { browserPlatform } from "./platform/browser.js";
import { setPlatform } from "./platform/platform.js";

setPlatform(browserPlatform);

export * from "./exports.js";
