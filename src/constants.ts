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

export const FONT_UNITS_PER_EM = 1000;
export const DEFAULT_FONT_ASCENT = 0.8;
export const DEFAULT_FONT_DESCENT = -0.2;

export interface StandardFontMetrics {
  fontName: string;
  ascent?: number;
  descent?: number;
}

export const STANDARD_FONT_METRICS: Record<string, StandardFontMetrics> = {
  Courier: { fontName: "Courier", ascent: 0.627, descent: -0.194 },
  "Courier-Bold": { fontName: "Courier-Bold", ascent: 0.627, descent: -0.194 },
  "Courier-Oblique": { fontName: "Courier-Oblique", ascent: 0.627, descent: -0.194 },
  "Courier-BoldOblique": { fontName: "Courier-BoldOblique", ascent: 0.627, descent: -0.194 },
  CourierNew: { fontName: "Courier", ascent: 0.627, descent: -0.194 },
  "CourierNew,Bold": { fontName: "Courier-Bold", ascent: 0.627, descent: -0.194 },
  "CourierNew,Italic": { fontName: "Courier-Oblique", ascent: 0.627, descent: -0.194 },
  "CourierNew,BoldItalic": { fontName: "Courier-BoldOblique", ascent: 0.627, descent: -0.194 },
  Helvetica: { fontName: "Helvetica", ascent: 0.718, descent: -0.207 },
  "Helvetica-Bold": { fontName: "Helvetica-Bold", ascent: 0.718, descent: -0.207 },
  "Helvetica-Oblique": { fontName: "Helvetica-Oblique", ascent: 0.718, descent: -0.207 },
  "Helvetica-BoldOblique": { fontName: "Helvetica-BoldOblique", ascent: 0.718, descent: -0.207 },
  "Times-Roman": { fontName: "Times-Roman", ascent: 0.683, descent: -0.217 },
  "Times-Bold": { fontName: "Times-Bold", ascent: 0.683, descent: -0.217 },
  "Times-Italic": { fontName: "Times-Italic", ascent: 0.683, descent: -0.217 },
  "Times-BoldItalic": { fontName: "Times-BoldItalic", ascent: 0.683, descent: -0.217 },
  Symbol: { fontName: "Symbol" },
  ZapfDingbats: { fontName: "ZapfDingbats" },
  Arial: { fontName: "Helvetica", ascent: 0.718, descent: -0.207 },
  "Arial,Bold": { fontName: "Helvetica-Bold", ascent: 0.718, descent: -0.207 },
  "Arial,Italic": { fontName: "Helvetica-Oblique", ascent: 0.718, descent: -0.207 },
  "Arial,BoldItalic": { fontName: "Helvetica-BoldOblique", ascent: 0.718, descent: -0.207 },
  TimesNewRoman: { fontName: "Times-Roman", ascent: 0.683, descent: -0.217 },
  "TimesNewRoman,Bold": { fontName: "Times-Bold", ascent: 0.683, descent: -0.217 },
  "TimesNewRoman,Italic": { fontName: "Times-Italic", ascent: 0.683, descent: -0.217 },
  "TimesNewRoman,BoldItalic": { fontName: "Times-BoldItalic", ascent: 0.683, descent: -0.217 }
};
