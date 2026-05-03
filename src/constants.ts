export const DEFAULT_X_TOLERANCE = 3;
export const DEFAULT_Y_TOLERANCE = 3;
export const DEFAULT_X_DENSITY = 7.25;
export const DEFAULT_Y_DENSITY = 13;

export const LIGATURES: Record<string, string> = {
  "\ufb00": "ff",
  "\ufb03": "ffi",
  "\ufb04": "ffl",
  "\ufb01": "fi",
  "\ufb02": "fl",
  "\ufb06": "st",
  "\ufb05": "st"
};

export const METADATA_KEYS = ["Author", "Creator", "Producer", "Title", "Subject", "Keywords", "CreationDate", "ModDate", "Copies"];
export const PUNCTUATION = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
export const STANDARD_FONT_ALIASES: Record<string, string> = {
  Arial: "Helvetica",
  "Arial,Bold": "Helvetica-Bold",
  "Arial,Italic": "Helvetica-Oblique",
  "Arial,BoldItalic": "Helvetica-BoldOblique"
};
export const STANDARD_DESCENTS: Record<string, number> = {
  Helvetica: -0.207,
  "Helvetica-Bold": -0.207,
  "Helvetica-Oblique": -0.207,
  "Helvetica-BoldOblique": -0.207,
  "Times-Roman": -0.217,
  "Times-Bold": -0.205,
  "Times-Italic": -0.217,
  "Times-BoldItalic": -0.205
};
