// Line icons (24×24, 1.6 stroke), drawn as inline SVG so they follow `currentColor`. Every icon
// is decorative: the control or text next to it carries the name.
import type { ConnectMode } from "./bindings";

const PATHS = {
  login: ["M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4", "M9 16l4-4-4-4", "M13 12H4"],
  server: ["M6 4h12a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "M6 13h12a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2z", "M8 7.5h.01M8 16.5h.01"],
  display: ["M5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "M8 20h8M12 16v4"],
  search: ["M11 4.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13z", "M20 20l-4.2-4.2"],
  plus: ["M12 5v14M5 12h14"],
  shield: ["M12 3l7 3v5.5c0 4.4-3 8-7 9.5-4-1.5-7-5.1-7-9.5V6z", "M9 12l2.2 2.2L15.5 10"],
  check: ["M5 12.5l4.5 4.5L19 7.5"],
  warn: ["M12 4l9 16H3z", "M12 10v4M12 17h.01"],
  user: ["M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8z", "M4.5 20c1.4-3.6 4.2-5.5 7.5-5.5s6.1 1.9 7.5 5.5"],
  chevron: ["M9 5l7 7-7 7"],
  grid: [
    "M6 4h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z",
    "M15 4h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z",
    "M6 13h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2z",
    "M15 13h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2h-3a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2z",
  ],
  gauge: ["M4 18a8 8 0 1 1 16 0", "M12 18l4-6"],
  window: ["M5.5 5h13A2.5 2.5 0 0 1 21 7.5v9a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 16.5v-9A2.5 2.5 0 0 1 5.5 5z", "M3 9h18"],
} as const;

/** Filled glyphs (no stroke). */
const FILLED = {
  play: "M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z",
  more: "M6 10.4a1.6 1.6 0 1 1 0 3.2 1.6 1.6 0 0 1 0-3.2zM12 10.4a1.6 1.6 0 1 1 0 3.2 1.6 1.6 0 0 1 0-3.2zM18 10.4a1.6 1.6 0 1 1 0 3.2 1.6 1.6 0 0 1 0-3.2z",
} as const;

export type IconName = keyof typeof PATHS | keyof typeof FILLED;

const SVG = "http://www.w3.org/2000/svg";

/** An `aria-hidden` inline SVG icon. */
export function icon(name: IconName): SVGSVGElement {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.setAttribute("class", `icon icon-${name}`);
  const filled = name in FILLED;
  const ds: readonly string[] = filled ? [FILLED[name as keyof typeof FILLED]] : PATHS[name as keyof typeof PATHS];
  for (const d of ds) {
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  if (filled) svg.classList.add("filled");
  return svg;
}

/** The icon for a connection mode. */
export const MODE_ICONS: Record<ConnectMode, IconName> = {
  headless: "server",
  "desktop-sharing": "display",
  "remote-login": "login",
};

/** A rounded, mode-tinted tile around the mode's icon. */
export function modeGlyph(mode: ConnectMode, size: "" | "small" | "large" = ""): HTMLSpanElement {
  const span = document.createElement("span");
  span.className = ["glyph", `glyph-${mode}`, size].filter(Boolean).join(" ");
  span.setAttribute("aria-hidden", "true");
  span.append(icon(MODE_ICONS[mode]));
  return span;
}
