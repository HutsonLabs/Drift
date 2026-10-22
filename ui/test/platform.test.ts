// RESP-2 Red: the pages know whether they run in the iOS app, so the stylesheet can drop the
// Mac's traffic-light space and size things for touch (ADR `RESP-2-touch-platform`).
import { describe, expect, test } from "bun:test";
import { applyPlatform, detectPlatform } from "../src/platform";

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";

describe("platform", () => {
  test("the Mac app's WKWebView is macos", () => {
    expect(detectPlatform({ userAgent: MAC, maxTouchPoints: 0 })).toBe("macos");
  });

  test("an iPhone is ios", () => {
    expect(detectPlatform({ userAgent: IPHONE, maxTouchPoints: 5 })).toBe("ios");
  });

  test("an iPad is ios, though iPadOS reports a Mac user agent", () => {
    expect(detectPlatform({ userAgent: MAC, maxTouchPoints: 5 })).toBe("ios");
  });

  test("applyPlatform sets data-platform on <html>", () => {
    expect(applyPlatform(document, { userAgent: IPHONE, maxTouchPoints: 5 })).toBe("ios");
    expect(document.documentElement.dataset.platform).toBe("ios");
    applyPlatform(document, { userAgent: MAC, maxTouchPoints: 0 });
    expect(document.documentElement.dataset.platform).toBe("macos");
  });
});
