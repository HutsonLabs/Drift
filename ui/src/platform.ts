// RESP-2: which app the page runs in, as `data-platform` on <html> for the stylesheet (ADR
// `RESP-2-touch-platform`). The Mac app's title bar keeps 88 points for the traffic lights; the
// iOS app has none. Touch sizing itself follows `(pointer: coarse)`, not this.

/** The app the page runs in. */
export type Platform = "ios" | "macos";

/** What `detectPlatform` reads from `navigator`. */
export interface PlatformProbe {
  userAgent: string;
  maxTouchPoints: number;
}

/**
 * `ios` on iPhone and iPad. iPadOS's WKWebView reports a Mac user agent, but a Mac's has no
 * touch points.
 */
export function detectPlatform(probe: PlatformProbe): Platform {
  if (/iPhone|iPad|iPod/.test(probe.userAgent)) return "ios";
  return /Macintosh/.test(probe.userAgent) && probe.maxTouchPoints > 1 ? "ios" : "macos";
}

/** Sets `data-platform` on `doc`'s <html> and returns it. */
export function applyPlatform(doc: Document = document, probe: PlatformProbe = navigator): Platform {
  const platform = detectPlatform(probe);
  doc.documentElement.dataset.platform = platform;
  return platform;
}
