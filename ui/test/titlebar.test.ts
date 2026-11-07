// UI-windows Red (boards 4, 5, 7): a session window's title bar. A centred identity capsule
// (mode glyph, name, host, status dot or spinner, greeter hint as tooltip) and the Show
// Connections and Statistics buttons. The title bar never keeps the keyboard.
import { describe, expect, test } from "bun:test";
import type { CommandError, WindowIdentity } from "../src/bindings";
import { type TitlebarApi, TitlebarApp } from "../src/titlebarApp";
import { button, flush, hasButton, identity, text } from "./helpers";

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };
const ok = <T>(data: T): Promise<Result<T>> => Promise.resolve({ status: "ok", data });

class FakeTitlebarApi {
  calls: unknown[][] = [];
  constructor(public current: WindowIdentity = identity()) {}
  api(): TitlebarApi {
    const log = (...c: unknown[]) => this.calls.push(c);
    return {
      windowIdentity: () => (log("windowIdentity"), ok(this.current)),
      showConnections: (edit: string | null) => (log("showConnections", edit), ok(null)),
      toggleStats: () => (log("toggleStats"), ok(null)),
      focusContent: () => (log("focusContent"), ok(null)),
    } as unknown as TitlebarApi;
  }
}

async function start(fake = new FakeTitlebarApi()) {
  const root = document.createElement("div");
  root.id = "titlebar";
  document.body.replaceChildren(root);
  const app = new TitlebarApp(root, fake.api());
  await app.start();
  await flush();
  return { root, app, fake };
}

describe("identity capsule", () => {
  test("pulls the identity on load: mode glyph, name, mode and a live dot", async () => {
    const { root, fake, app } = await start();
    expect(fake.calls).toContainEqual(["windowIdentity"]);
    const capsule = root.querySelector(".ident") as HTMLElement;
    expect(capsule.querySelector(".glyph-remote-login")).not.toBeNull();
    expect(text(capsule.querySelector(".name"))).toBe("Homelab");
    expect(text(capsule.querySelector(".detail"))).toBe("Remote Login");
    expect(capsule.querySelector(".status.live")).not.toBeNull();
    expect(capsule.querySelector(".spin")).toBeNull();
    // VoiceOver hears the status, not a coloured dot.
    expect(capsule.getAttribute("aria-label")).toBe("Homelab, Remote Login, Live");
    app.dispose();
  });

  test("the capsule names the mode", async () => {
    const { root, app } = await start();
    const names = { headless: "Headless", "desktop-sharing": "Desktop Sharing", "remote-login": "Remote Login" } as const;
    for (const mode of ["headless", "desktop-sharing", "remote-login"] as const) {
      app.onIdentity(identity({ mode }));
      expect(text(root.querySelector(".ident .detail"))).toBe(names[mode]);
    }
    app.dispose();
  });

  test("a spinner while connecting, amber while reconnecting, red when failed", async () => {
    const { root, app } = await start();
    app.onIdentity(identity({ status: "connecting" }));
    expect(root.querySelector(".ident .spin")).not.toBeNull();
    expect(root.querySelector(".ident .status")).toBeNull();
    expect(root.querySelector(".ident")?.getAttribute("aria-label")).toBe("Homelab, Remote Login, Connecting");
    app.onIdentity(identity({ status: "reconnecting" }));
    expect(root.querySelector(".ident .status.reconnecting")).not.toBeNull();
    app.onIdentity(identity({ status: "failed" }));
    expect(root.querySelector(".ident .status.failed")).not.toBeNull();
    expect(root.querySelector(".ident .spin")).toBeNull();
    app.dispose();
  });

  test("the greeter hint is the capsule's tooltip; otherwise the host", async () => {
    const { root, app } = await start();
    expect(root.querySelector(".ident")?.getAttribute("title")).toBe("gnome.local");
    app.onIdentity(identity({ hint: "Log in as “alice” on the GNOME login screen." }));
    expect(root.querySelector(".ident")?.getAttribute("title")).toBe("Log in as “alice” on the GNOME login screen.");
    app.dispose();
  });

  test("the whole bar drags the window", async () => {
    const { root, app } = await start();
    expect(root.querySelector(".titlebar")?.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(root.querySelector(".ident")?.hasAttribute("data-tauri-drag-region")).toBe(true);
    app.dispose();
  });
});

describe("buttons", () => {
  test("Show Connections brings the gallery forward", async () => {
    const { root, fake, app } = await start();
    const grid = button(root, "Show Connections");
    expect(grid.querySelector("svg.icon-grid")).not.toBeNull();
    grid.click();
    await flush();
    expect(fake.calls).toContainEqual(["showConnections", null]);
    app.dispose();
  });

  test("Statistics is a toggle pressed while the HUD is on", async () => {
    const { root, fake, app } = await start();
    const gauge = button(root, "Statistics");
    expect(gauge.querySelector("svg.icon-gauge")).not.toBeNull();
    expect(gauge.getAttribute("aria-pressed")).toBe("false");
    gauge.click();
    await flush();
    expect(fake.calls).toContainEqual(["toggleStats"]);
    app.onIdentity(identity({ show_stats: true }));
    expect(button(root, "Statistics").getAttribute("aria-pressed")).toBe("true");
    app.dispose();
  });

  test("Statistics is only offered while there is a picture", async () => {
    const { root, app } = await start();
    app.onIdentity(identity({ status: "connecting" }));
    expect(hasButton(root, "Statistics")).toBe(false);
    expect(hasButton(root, "Show Connections")).toBe(true);
    app.onIdentity(identity({ status: "failed" }));
    expect(hasButton(root, "Statistics")).toBe(false);
    app.onIdentity(identity({ status: "reconnecting" }));
    expect(hasButton(root, "Statistics")).toBe(true);
    app.dispose();
  });

  test("the title bar hands the keyboard back whenever it gains focus", async () => {
    const { fake, app } = await start();
    window.dispatchEvent(new Event("focus"));
    await flush();
    expect(fake.calls.filter((c) => c[0] === "focusContent").length).toBe(1);
    app.dispose();
    window.dispatchEvent(new Event("focus"));
    await flush();
    expect(fake.calls.filter((c) => c[0] === "focusContent").length).toBe(1);
  });
});
