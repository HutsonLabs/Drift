// RESP-1: the breakpoint regression harness (docs/responsive/findings.md, "Method").
//
// Builds `harness.ts` with the production stylesheet, then loads every page state in Playwright
// WebKit (touch on, 2×) at each breakpoint, sized to the frame the iOS shell gives that page, and
// checks the rules below. Every failure names the finding it guards. Exit status 1 on any.
//
//   bun run responsive            # check
//   bun run responsive --shots    # also write responsive/shots/<breakpoint>/<state>.png
//   bun run responsive c-edit     # only states starting with "c-edit"
import { mkdir, rm } from "node:fs/promises";
import { webkit } from "playwright-core";

const HERE = new URL(".", import.meta.url).pathname;
const DIST = `${HERE}dist/`;
const SHOTS = `${HERE}shots/`;

/** `present::hud_frame` and `phone::phone_layout` (src-tauri), mirrored. */
const HUD_INSET = 16;
const BANNER = { width: 560, height: 76 };
const STATS = { width: 300, height: 56 };
/** The key row's strip: one row in landscape, two in portrait (`phone::KEY_ROW_HEIGHT*`). */
const KEY_ROW_HEIGHT = 56;
const KEY_ROWS_HEIGHT_PORTRAIT = 56;

const IPHONE_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";
const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const IPAD_UA = "Mozilla/5.0 (iPad; CPU OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";

interface Breakpoint {
  id: string;
  w: number;
  h: number;
  phone: boolean;
  landscape?: boolean;
  /** Left and right safe-area insets (landscape iPhone). */
  safeX?: number;
  /** The Mac app: a fine pointer and a Mac user agent; guards against touch rules leaking. */
  mac?: boolean;
}

export const BREAKPOINTS: Breakpoint[] = [
  { id: "B1-320", w: 320, h: 568, phone: false },
  { id: "B2-390", w: 390, h: 844, phone: true },
  { id: "B3-402", w: 402, h: 874, phone: true },
  { id: "B4-440", w: 440, h: 956, phone: true },
  { id: "B5-874x402", w: 874, h: 402, phone: true, landscape: true, safeX: 62 },
  { id: "B6-590", w: 590, h: 834, phone: false },
  { id: "B7-744", w: 744, h: 1133, phone: false },
  { id: "B8-834", w: 834, h: 1210, phone: false },
  { id: "B9-1210", w: 1210, h: 834, phone: false },
  { id: "B10-1376", w: 1376, h: 1032, phone: false },
  { id: "MAC-640", w: 640, h: 420, phone: false, mac: true },
  { id: "MAC-1100", w: 1100, h: 720, phone: false, mac: true },
];

const PAGES = [
  "c-welcome",
  "c-gallery",
  "c-search",
  "c-new",
  "c-new-mode",
  "c-edit",
  "c-menu",
  "c-confirm",
  "s-connecting",
  "s-certificate",
  "s-error",
  "s-reconnecting",
];
const STATES = [...PAGES, "s-banner", "s-stats", "s-keys"];

/** The frame the iOS shell gives `state`'s page at `bp`, or null if it has none there. */
function frame(bp: Breakpoint, state: string): { w: number; h: number } | null {
  if (PAGES.includes(state)) return { w: bp.w, h: bp.h };
  if (bp.mac && state === "s-keys") return null;
  if (state === "s-banner") return { w: Math.min(BANNER.width, bp.w - 2 * HUD_INSET), h: BANNER.height };
  if (state === "s-stats") return { w: STATS.width, h: STATS.height };
  // The key row is iPhone only; iOS 17's narrowest iPhone is 375 pt.
  if (state === "s-keys" && bp.phone) {
    return { w: bp.w - 2 * (bp.safeX ?? 0), h: bp.landscape ? KEY_ROW_HEIGHT : KEY_ROWS_HEIGHT_PORTRAIT };
  }
  return null;
}

/** Runs in the page: every rule, returning one line per failure. */
function check(state: string): string[] {
  const W = innerWidth;
  const H = innerHeight;
  const out: string[] = [];
  const fail = (finding: string, what: string) => out.push(`${finding} ${what}`);
  const visible = (e: Element) => {
    const r = e.getBoundingClientRect();
    const cs = getComputedStyle(e);
    return (
      r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.opacity !== "0" && !e.closest(".visually-hidden,.type-field,[hidden]")
    );
  };
  const name = (e: Element) => {
    const t = (e.getAttribute("aria-label") ?? e.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 30);
    const cls = typeof e.className === "string" && e.className.trim() ? `.${e.className.trim().split(/\s+/).join(".")}` : "";
    return `${e.tagName.toLowerCase()}${cls}${t ? ` "${t}"` : ""}`;
  };
  const px = (n: number) => Math.round(n);
  const lines = (e: Element) => {
    const cs = getComputedStyle(e);
    const lh = Number.parseFloat(cs.lineHeight) || Number.parseFloat(cs.fontSize) * 1.45;
    const box = Number.parseFloat(cs.paddingTop) + Number.parseFloat(cs.paddingBottom);
    return Math.round((e.getBoundingClientRect().height - box) / lh);
  };
  const coarse = matchMedia("(pointer: coarse)").matches;
  const ios = document.documentElement.dataset.platform === "ios";
  const interactive = Array.from(
    document.querySelectorAll("button, input:not([type=hidden]), select, summary, [role=menuitem]"),
  ).filter(visible);
  const controls = interactive.filter((e) => e.matches("input:not([type=checkbox]):not([type=radio]), select"));

  scrollTo(0, 0);
  const se = document.scrollingElement!;
  if (se.scrollWidth > W + 1) fail("F8/F13 overflow", `page scrolls sideways by ${se.scrollWidth - W} pt`);

  for (const e of interactive) {
    const r = e.getBoundingClientRect();
    if (r.right > W + 1 || r.left < -1) fail("F2/F3/F8 offscreen", `${name(e)} at x ${px(r.left)}..${px(r.right)} of ${W}`);
    const box = e.closest(".cfg, .sheet, .menu");
    if (box) {
      const b = box.getBoundingClientRect();
      if (r.right > b.right + 1 || r.left < b.left - 1) fail("F2/F3/F13 outside", `${name(e)} sticks out of ${name(box).split(" ")[0]}`);
    }
  }
  for (const e of controls) {
    // The port field is narrow by design (84 pt).
    if (e.closest(".cfg") && !e.closest(".port") && e.getBoundingClientRect().width < 120) {
      fail("F1 collapsed", `${name(e)} is ${px(e.getBoundingClientRect().width)} pt wide`);
    }
  }

  for (const s of Array.from(document.querySelectorAll(".scrim > .sheet, .cfg")).filter(visible)) {
    const r = s.getBoundingClientRect();
    if (r.top < -1) fail("F12 unreachable", `${name(s).split(" ")[0]} top at y ${px(r.top)}`);
    const bottom = s.matches(".cfg") ? H : Math.max(H, se.scrollHeight);
    if (r.bottom > bottom + 1) fail("F12 unreachable", `${name(s).split(" ")[0]} bottom at y ${px(r.bottom)} of ${bottom}`);
    if (ios && document.body.matches(".session") && r.top < 56 && r.left < 56) {
      fail("F17 switcher", `${name(s).split(" ")[0]} runs under the switcher button (top ${px(r.top)})`);
    }
  }

  if (coarse) {
    for (const e of interactive) {
      if (e.matches(".key-row .type-field")) continue;
      const target = e.matches("input[type=checkbox]") ? (e.closest("label") ?? e) : e;
      const r = target.getBoundingClientRect();
      if (r.width < 44 || r.height < 44) fail("F19 target", `${name(target)} is ${px(r.width)}×${px(r.height)}`);
    }
    for (const e of controls) {
      const fs = Number.parseFloat(getComputedStyle(e).fontSize);
      if (fs < 16) fail("F18 zoom", `${name(e)} is ${fs}px`);
    }
  }
  if (matchMedia("(any-hover: none)").matches) {
    for (const k of Array.from(document.querySelectorAll("kbd")).filter(visible)) fail("F11 kbd", `"${k.textContent}" shows without a keyboard`);
  }

  // Configuration sheet.
  const cfg = document.querySelector(".cfg");
  if (cfg && visible(cfg)) {
    if (W < 560 && cfg.getBoundingClientRect().width < W - 1) fail("F6 floating", `sheet is ${px(cfg.getBoundingClientRect().width)} of ${W} pt`);
    for (const n of Array.from(cfg.querySelectorAll(".cfg-foot .note")).filter(visible)) {
      if (lines(n) > 2) fail("F3 note", `${name(n)} wraps to ${lines(n)} lines`);
    }
    const h2 = cfg.querySelector<HTMLElement>(".cfg-head h2");
    if (h2 && h2.scrollWidth > h2.clientWidth + 1) fail("F4 title", `"${h2.textContent}" is cut off`);
    const meta = cfg.querySelector(".cfg-head .meta")?.getBoundingClientRect();
    for (const b of Array.from(cfg.querySelectorAll(".cfg-head button"))) {
      const r = b.getBoundingClientRect();
      if (meta && r.left < meta.right && r.right > meta.left && r.top < meta.bottom && r.bottom > meta.top) {
        fail("F4 overlap", `${name(b)} covers the host line`);
      }
    }
    const fp = cfg.querySelector(".pin .fingerprint");
    // 95 characters of monospace need about 650 pt on one line; 3 lines is the floor on a phone.
    if (fp && ios && W >= 390 && lines(fp) > 3) fail("F5 fingerprint", `wraps to ${lines(fp)} lines`);
    const summary = cfg.querySelector("details.advanced summary");
    if (summary && lines(summary) > 1 && summary.getBoundingClientRect().height > 50) fail("F7 summary", "wraps");
  }

  // Toolbar and gallery.
  if (state === "c-search") {
    const s = document.querySelector("input[type=search]");
    if (!s || !visible(s) || s.getBoundingClientRect().width < 200) {
      fail("F8 search", `search field is ${s && visible(s) ? px(s.getBoundingClientRect().width) : 0} pt wide`);
    }
  }
  if (ios && W >= 744) {
    for (const n of Array.from(document.querySelectorAll<HTMLElement>(".item-name")).filter(visible)) {
      if (n.scrollWidth > n.clientWidth + 1 || n.scrollHeight > n.clientHeight + 1) fail("F10 name", `"${n.textContent}" is cut off`);
    }
  }

  // Session HUDs and key row.
  for (const b of Array.from(document.querySelectorAll(".banner")).filter(visible)) {
    const r = b.getBoundingClientRect();
    if (r.bottom > H + 1 || r.right > W + 1) fail("F16 banner", `banner ends at ${px(r.right)}×${px(r.bottom)} in a ${W}×${H} frame`);
  }
  for (const k of Array.from(document.querySelectorAll(".key-row .key")).filter(visible)) {
    const r = k.getBoundingClientRect();
    if (r.right > W + 1 || r.bottom > H + 1) fail("F14 key", `${name(k)} is outside the ${W}×${H} strip`);
  }
  for (const k of Array.from(document.querySelectorAll('.key-row .key[aria-label="Keyboard"], .key-row .key.touch-mode'))) {
    if (!k.querySelector("svg.icon")) fail("F15 glyph", `${name(k)} is a text glyph, not an icon`);
  }
  return out;
}

async function build(): Promise<void> {
  await rm(DIST, { recursive: true, force: true });
  await mkdir(DIST, { recursive: true });
  const r = await Bun.build({ entrypoints: [`${HERE}harness.ts`], outdir: DIST, target: "browser" });
  if (!r.success) {
    for (const l of r.logs) console.error(l);
    process.exit(1);
  }
  await Bun.write(
    `${DIST}index.html`,
    '<!doctype html><html lang="en"><head><meta charset="UTF-8" />' +
      '<meta name="viewport" content="width=device-width, initial-scale=1.0" />' +
      '<link rel="stylesheet" href="./styles.css" /></head><body><main id="app"></main>' +
      '<script type="module" src="./harness.js"></script></body></html>',
  );
  await Bun.write(`${DIST}styles.css`, Bun.file(`${HERE}../src/styles.css`));
}

const args = process.argv.slice(2);
const shots = args.includes("--shots");
const only = args.find((a) => !a.startsWith("--"));

await build();
const server = Bun.serve({
  port: 0,
  fetch: (req) => {
    const path = new URL(req.url).pathname;
    return new Response(Bun.file(`${DIST}${path === "/" ? "index.html" : path.slice(1)}`));
  },
});
const browser = await webkit.launch();
const failures: string[] = [];
let runs = 0;
try {
  for (const bp of BREAKPOINTS) {
    for (const state of STATES) {
      if (only && !state.startsWith(only)) continue;
      const f = frame(bp, state);
      if (!f) continue;
      const ctx = await browser.newContext({
        viewport: { width: f.w, height: f.h },
        deviceScaleFactor: 2,
        hasTouch: !bp.mac,
        userAgent: bp.mac ? MAC_UA : bp.phone ? IPHONE_UA : IPAD_UA,
      });
      const page = await ctx.newPage();
      await page.goto(`http://localhost:${server.port}/#${state}`);
      await page.waitForSelector("html[data-ready]", { state: "attached" });
      const error = await page.getAttribute("html", "data-error");
      if (error) failures.push(`${bp.id} ${state} HARNESS ${error.split("\n")[0]}`);
      await page.waitForTimeout(100);
      if (shots) {
        await mkdir(`${SHOTS}${bp.id}`, { recursive: true });
        await page.screenshot({ path: `${SHOTS}${bp.id}/${state}.png` });
      }
      for (const line of await page.evaluate(check, state)) failures.push(`${bp.id} ${state} ${line}`);
      runs += 1;
      await ctx.close();
    }
  }
} finally {
  await browser.close();
  server.stop();
}
for (const f of failures) console.log(f);
console.log(`responsive: ${runs} page states, ${failures.length} failure(s)`);
process.exit(failures.length > 0 ? 1 : 0);
