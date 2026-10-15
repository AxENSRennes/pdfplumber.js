/** Node entry point of pdfplumber-js. */

import { nodePlatform } from "./platform/node.js";
import { setPlatform } from "./platform/platform.js";

setPlatform(nodePlatform);

export * from "./exports.js";
