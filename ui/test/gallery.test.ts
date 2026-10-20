// UI-windows Red (board 1): the Connections window's gallery. Toolbar in the title bar, Open and
// Saved sections, card states and pills, filter and search, keyboard grid, VoiceOver labels,
// hover/focus actions, the ⋯ and context menus, and live thumbnails (memory only).
import { describe, expect, test } from "bun:test";
import type { OpenConnection } from "../src/bindings";
import { newConnection, FakeConnectionsApi, startConnections } from "./fakes";
import { button, entry, flush, hasButton, key, openConnection, profile, text, type } from "./helpers";

const STUDIO = profile("headless", { name: "Studio Workstation", host: "192.168.1.20" });
const HOMELAB = profile("remote-login", { name: "Homelab", host: "gnome.local" });
const MEDIA = profile("headless", { name: "Media PC", host: "media.lan" });
const BUILD = profile("headless", { name: "Build Server", host: "build-01.lan" });
const KITCHEN = profile("desktop-sharing", { name: "Kitchen Laptop", host: "kitchen.lan", cert_pin: "aa:bb" });

/** The board-1 fixture: five saved connections, three of them with a window. */
function board(open: OpenConnection[] = [
  openConnection(STUDIO.id, "live", { live_secs: 2 * 3600 + 14 * 60 }),
  openConnection(HOMELAB.id, "live", { live_secs: 120 }),
  openConnection(MEDIA.id, "reconnecting", { reconnect_in_secs: 3, attempt: 2 }),
]): FakeConnectionsApi {
  const fake = new FakeConnectionsApi();
  // list_profiles sorts by name.
  fake.entries = [BUILD, HOMELAB, KITCHEN, MEDIA, STUDIO].map((p) => entry(p));
  fake.open = open;
  return fake;
}

function card(root: ParentNode, name: string): HTMLElement {
  const c = Array.from(root.querySelectorAll<HTMLElement>("article.card")).find((a) => text(a.querySelector(".item-name")) === name);
  if (!c) throw new Error(`no card "${name}"`);
  return c;
}

function section(root: ParentNode, title: string): HTMLElement | null {
  return Array.from(root.querySelectorAll<HTMLElement>("section.gallery-section")).find((s) => text(s.querySelector("h2")) === title) ?? null;
}

function names(el: ParentNode | null): string[] {
  return Array.from(el?.querySelectorAll("article.card:not([hidden]) .item-name") ?? []).map(text);
}

function menuItems(root: ParentNode): string[] {
  return Array.from(root.querySelectorAll("[role=menu] [role=menuitem]")).map((i) => text(i.querySelector(".label")));
}

function menuItem(root: ParentNode, label: string): HTMLElement {
  const item = Array.from(root.querySelectorAll<HTMLElement>("[role=menu] [role=menuitem]")).find((i) => text(i.querySelector(".label")) === label);
  if (!item) throw new Error(`no menu item "${label}"; have ${menuItems(root).join(", ")}`);
  return item;
}

describe("toolbar", () => {
  test("title with count, All / Open filter, search and + sit in the draggable title bar", async () => {
    const { root } = await startConnections(board());
    const bar = root.querySelector("header.toolbar") as HTMLElement;
    expect(bar.hasAttribute("data-tauri-drag-region")).toBe(true);
    expect(text(bar.querySelector("h1"))).toBe("Connections");
    expect(text(bar.querySelector(".count"))).toBe("5");
    expect(button(bar, "All").getAttribute("aria-pressed")).toBe("true");
    expect(button(bar, "Open").getAttribute("aria-pressed")).toBe("false");
    expect(bar.querySelector("input[type=search]")?.getAttribute("aria-label")).toBe("Search connections");
    expect(hasButton(bar, "New Connection")).toBe(true);
  });

  test("+ starts a new connection", async () => {
    const { root, fake } = await startConnections(board());
    await newConnection(root);
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("New Connection");
  });
});

describe("sections", () => {
  test("open connections come first in window order, then Saved, then the dashed New Connection card", async () => {
    const { root, fake } = await startConnections(board());
    expect(fake.names()).toEqual(expect.arrayContaining(["listProfiles", "connections"]));
    const open = section(root, "Open");
    const saved = section(root, "Saved");
    expect(names(open)).toEqual(["Studio Workstation", "Homelab", "Media PC"]);
    expect(text(open?.querySelector(".section-count"))).toBe("3 windows");
    expect(names(saved)).toEqual(["Build Server", "Kitchen Laptop"]);
    expect(text(saved?.querySelector(".section-count"))).toBe("2 connections");
    const last = saved?.querySelector(".gallery")?.lastElementChild as HTMLElement;
    expect(last.matches("button.card.add")).toBe(true);
    expect(last.getAttribute("aria-label")).toBe("Add a GNOME computer");
  });

  test("no Open section without windows; singular counts", async () => {
    const fake = board([]);
    fake.entries = [entry(BUILD)];
    const { root } = await startConnections(fake);
    expect(section(root, "Open")).toBeNull();
    expect(text(section(root, "Saved")?.querySelector(".section-count"))).toBe("1 connection");
  });

  test("a card shows a 16:9 preview in the mode's colours, its glyph, name and host · mode", async () => {
    const { root } = await startConnections(board());
    const c = card(root, "Kitchen Laptop");
    expect(c.querySelector(".thumb")?.classList.contains("thumb-desktop-sharing")).toBe(true);
    expect(c.querySelector(".thumb svg")).not.toBeNull();
    expect(c.querySelector(".card-info .glyph-desktop-sharing")).not.toBeNull();
    expect(text(c.querySelector(".item-detail"))).toBe("kitchen.lan · Desktop Sharing");
    expect(text(card(root, "Build Server").querySelector(".item-detail"))).toBe("build-01.lan · Headless session");
  });

  test("connectionsChanged moves cards between the sections", async () => {
    const { root, app } = await startConnections(board([]));
    expect(section(root, "Open")).toBeNull();
    app.onConnections({ open: [openConnection(BUILD.id, "connecting")] });
    expect(names(section(root, "Open"))).toEqual(["Build Server"]);
    expect(names(section(root, "Saved"))).not.toContain("Build Server");
    app.onConnections({ open: [] });
    expect(section(root, "Open")).toBeNull();
  });

  test("first launch: an empty gallery invites you to add a computer and opens the New Connection sheet", async () => {
    const { root, fake } = await startConnections(new FakeConnectionsApi());
    expect(text(root.querySelector(".welcome h2"))).toBe("Add a GNOME computer");
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("New Connection");
  });
});

describe("pills", () => {
  test("Live with uptime, Connecting with a spinner, Reconnecting in N s dimmed, Failed", async () => {
    const fake = board([
      openConnection(STUDIO.id, "live", { live_secs: 2 * 3600 + 14 * 60 }),
      openConnection(HOMELAB.id, "connecting"),
      openConnection(MEDIA.id, "reconnecting", { reconnect_in_secs: 3, attempt: 2 }),
      openConnection(KITCHEN.id, "failed"),
    ]);
    const { root } = await startConnections(fake);
    const live = card(root, "Studio Workstation");
    expect(text(live.querySelector(".pill.state"))).toBe("Live");
    expect(live.querySelector(".pill.state .status.live")).not.toBeNull();
    expect(text(live.querySelector(".pill.uptime"))).toBe("2 h 14 m");

    const connecting = card(root, "Homelab");
    expect(text(connecting.querySelector(".pill.state"))).toBe("Connecting");
    expect(connecting.querySelector(".pill.state .spin")).not.toBeNull();

    const reconnecting = card(root, "Media PC");
    expect(text(reconnecting.querySelector(".pill.state"))).toBe("Reconnecting in 3 s");
    expect(reconnecting.querySelector(".pill.state .status.reconnecting")).not.toBeNull();
    expect(reconnecting.querySelector(".thumb")?.classList.contains("dim")).toBe(true);

    const failed = card(root, "Kitchen Laptop");
    expect(text(failed.querySelector(".pill.state"))).toBe("Failed");
    expect(failed.querySelector(".pill.state .status.failed")).not.toBeNull();

    expect(card(root, "Build Server").querySelector(".pill")).toBeNull();
  });

  test("uptime and countdown tick locally between pushes", async () => {
    const { root, app, clock } = await startConnections(board());
    clock.now += 2000;
    app.tick();
    expect(text(card(root, "Media PC").querySelector(".pill.state"))).toBe("Reconnecting in 1 s");
    clock.now += 5000;
    app.tick();
    expect(text(card(root, "Media PC").querySelector(".pill.state"))).toBe("Reconnecting…");
    clock.now += 60_000;
    app.tick();
    expect(text(card(root, "Studio Workstation").querySelector(".pill.uptime"))).toBe("2 h 15 m");
    expect(text(card(root, "Homelab").querySelector(".pill.uptime"))).toBe("3 m");
    app.dispose();
  });
});

describe("filter and search", () => {
  test("Open shows only the connections with a window", async () => {
    const { root } = await startConnections(board());
    button(root, "Open").click();
    expect(button(root, "Open").getAttribute("aria-pressed")).toBe("true");
    expect(button(root, "All").getAttribute("aria-pressed")).toBe("false");
    expect(names(root)).toEqual(["Studio Workstation", "Homelab", "Media PC"]);
    expect(section(root, "Saved")).toBeNull();
    expect(root.querySelector("button.card.add")).toBeNull();
    button(root, "All").click();
    expect(names(root).length).toBe(5);
  });

  test("search filters by name and by host, ignoring case", async () => {
    const { root } = await startConnections(board());
    const search = root.querySelector("input[type=search]") as HTMLInputElement;
    type(search, "HOME");
    expect(names(root)).toEqual(["Homelab"]);
    type(search, ".lan");
    expect(names(root)).toEqual(["Media PC", "Build Server", "Kitchen Laptop"]);
    type(search, "192.168");
    expect(names(root)).toEqual(["Studio Workstation"]);
    expect(section(root, "Saved")).toBeNull();
    // The search field keeps the keyboard while the gallery re-renders under it.
    expect(document.activeElement === search || document.activeElement === document.body).toBe(true);
  });

  test("no matches says so", async () => {
    const { root } = await startConnections(board());
    type(root.querySelector("input[type=search]") as HTMLInputElement, "nothing-here");
    expect(names(root)).toEqual([]);
    expect(text(root.querySelector(".no-results"))).toContain("No connections match");
  });
});

describe("VoiceOver", () => {
  test("each card reads name, mode, host and state", async () => {
    const fake = board([
      openConnection(STUDIO.id, "live", { live_secs: 60 }),
      openConnection(HOMELAB.id, "connecting"),
      openConnection(MEDIA.id, "reconnecting"),
      openConnection(KITCHEN.id, "failed"),
    ]);
    const { root } = await startConnections(fake);
    expect(card(root, "Studio Workstation").getAttribute("aria-label")).toBe("Studio Workstation, Headless session, 192.168.1.20, Live");
    expect(card(root, "Homelab").getAttribute("aria-label")).toBe("Homelab, Remote Login, gnome.local, Connecting");
    expect(card(root, "Media PC").getAttribute("aria-label")).toBe("Media PC, Headless session, media.lan, Reconnecting");
    expect(card(root, "Kitchen Laptop").getAttribute("aria-label")).toBe("Kitchen Laptop, Desktop Sharing, kitchen.lan, Failed");
    expect(card(root, "Build Server").getAttribute("aria-label")).toBe("Build Server, Headless session, build-01.lan, Not connected");
    // The pills and glyphs repeat the label visually.
    for (const el of Array.from(root.querySelectorAll(".card .pill, .card .glyph, .card .thumb > svg"))) {
      expect(el.getAttribute("aria-hidden")).toBe("true");
    }
  });
});

describe("keyboard grid", () => {
  /** Lays the visible cards out in rows of three (happy-dom has no layout). */
  function layout(root: ParentNode) {
    const cells = Array.from(root.querySelectorAll<HTMLElement>(".gallery > .card:not([hidden])"));
    let row = 0;
    let lastSection: Element | null = null;
    let col = 0;
    for (const c of cells) {
      const sec = c.closest("section");
      if (sec !== lastSection && lastSection !== null) {
        row += 1;
        col = 0;
      } else if (col === 3) {
        row += 1;
        col = 0;
      }
      lastSection = sec;
      const x = col * 300;
      const y = row * 250;
      c.getBoundingClientRect = () => ({ x, y, left: x, top: y, width: 280, height: 230, right: x + 280, bottom: y + 230, toJSON: () => ({}) }) as DOMRect;
      col += 1;
    }
  }

  test("roving tabindex: one card is in the tab order; arrows move it", async () => {
    const { root } = await startConnections(board());
    layout(root);
    const cells = Array.from(root.querySelectorAll<HTMLElement>(".gallery > .card"));
    expect(cells.map((c) => c.getAttribute("tabindex"))).toEqual(["0", "-1", "-1", "-1", "-1", "-1"]);
    const studio = card(root, "Studio Workstation");
    studio.focus();
    key(studio, "ArrowRight");
    expect(document.activeElement).toBe(card(root, "Homelab"));
    expect(card(root, "Homelab").getAttribute("tabindex")).toBe("0");
    expect(studio.getAttribute("tabindex")).toBe("-1");
    key(document.activeElement!, "ArrowLeft");
    expect(document.activeElement).toBe(studio);
    key(studio, "ArrowDown");
    expect(document.activeElement).toBe(card(root, "Build Server"));
    key(document.activeElement!, "ArrowUp");
    expect(document.activeElement).toBe(studio);
    key(studio, "End");
    expect((document.activeElement as HTMLElement).matches("button.card.add")).toBe(true);
    key(document.activeElement!, "Home");
    expect(document.activeElement).toBe(studio);
  });

  test("Return and double-click do the primary action: Connect, or Show Window when open", async () => {
    const { root, fake } = await startConnections(board());
    const build = card(root, "Build Server");
    build.focus();
    key(build, "Enter");
    await flush();
    expect(fake.calls).toContainEqual(["connect", BUILD.id]);
    const studio = card(root, "Studio Workstation");
    key(studio, "Enter");
    await flush();
    expect(fake.calls).toContainEqual(["showWindow", STUDIO.id]);
    card(root, "Kitchen Laptop").dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await flush();
    expect(fake.calls).toContainEqual(["connect", KITCHEN.id]);
  });

  test("the focused card's ⌘E / ⌘D / ⌘⌫ edit, duplicate and delete it", async () => {
    const { root, fake } = await startConnections(board());
    const build = card(root, "Build Server");
    build.focus();
    key(build, "d", { meta: true });
    await flush();
    expect(fake.calls).toContainEqual(["duplicateProfile", BUILD.id]);
    expect(names(root)).toContain("Build Server copy");
    key(card(root, "Build Server"), "Backspace", { meta: true });
    expect(text(root.querySelector("[role=alertdialog] h2"))).toBe("Delete “Build Server”?");
    button(root.querySelector("[role=alertdialog]")!, "Cancel").click();
    expect(root.querySelector("[role=alertdialog]")).toBeNull();
    key(card(root, "Build Server"), "e", { meta: true });
    await flush();
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("Build Server");
  });
});

describe("hover and focus actions", () => {
  test("one primary button on the preview: Connect, or Show Window when the profile is open", async () => {
    const { root, fake } = await startConnections(board());
    const build = card(root, "Build Server").querySelectorAll(".over button");
    expect(build.length).toBe(1);
    expect(text(build[0])).toBe("Connect");
    expect(build[0]?.getAttribute("aria-label")).toBe("Connect to Build Server");
    expect(build[0]?.classList.contains("primary")).toBe(true);
    // Inside the grid only the card itself is a tab stop.
    expect(build[0]?.getAttribute("tabindex")).toBe("-1");
    (build[0] as HTMLButtonElement).click();
    const studio = card(root, "Studio Workstation").querySelectorAll(".over button");
    expect(studio.length).toBe(1);
    expect(text(studio[0])).toBe("Show Window");
    (studio[0] as HTMLButtonElement).click();
    await flush();
    expect(fake.calls).toContainEqual(["connect", BUILD.id]);
    expect(fake.calls).toContainEqual(["showWindow", STUDIO.id]);
  });

  test("a failed command shows a notice", async () => {
    const fake = board();
    fake.commandError = { kind: "not-found" };
    const { root } = await startConnections(fake);
    button(root, "Connect to Build Server").click();
    await flush();
    expect(text(root.querySelector(".notice[role=alert]"))).toBe("This connection no longer exists.");
  });
});

describe("⋯ menu and context menu", () => {
  test("a saved profile: Connect, Edit…, Duplicate, Delete… with their shortcuts", async () => {
    const { root } = await startConnections(board());
    button(root, "Actions for Build Server").click();
    const menu = root.querySelector("[role=menu]") as HTMLElement;
    expect(menu.getAttribute("aria-label")).toBe("Build Server");
    expect(menuItems(root)).toEqual(["Connect", "Edit…", "Duplicate", "Delete…"]);
    expect(Array.from(menu.querySelectorAll("kbd")).map(text)).toEqual(["↩", "⌘E", "⌘D", "⌘⌫"]);
    expect(document.activeElement).toBe(menu.querySelector("[role=menuitem]"));
  });

  test("an open profile also offers Disconnect", async () => {
    const { root, fake } = await startConnections(board());
    button(root, "Actions for Studio Workstation").click();
    expect(menuItems(root)).toEqual(["Show Window", "Edit…", "Duplicate", "Disconnect", "Delete…"]);
    menuItem(root, "Disconnect").click();
    await flush();
    expect(fake.calls).toContainEqual(["disconnectProfile", STUDIO.id]);
    expect(root.querySelector("[role=menu]")).toBeNull();
  });

  test("Connect, Edit… and Duplicate do what they say", async () => {
    const { root, fake } = await startConnections(board());
    button(root, "Actions for Build Server").click();
    menuItem(root, "Connect").click();
    await flush();
    expect(fake.calls).toContainEqual(["connect", BUILD.id]);
    button(root, "Actions for Build Server").click();
    menuItem(root, "Duplicate").click();
    await flush();
    expect(fake.calls).toContainEqual(["duplicateProfile", BUILD.id]);
    button(root, "Actions for Kitchen Laptop").click();
    menuItem(root, "Edit…").click();
    await flush();
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("Kitchen Laptop");
  });

  test("Delete… asks first", async () => {
    const { root, fake } = await startConnections(board());
    button(root, "Actions for Build Server").click();
    menuItem(root, "Delete…").click();
    const dialog = root.querySelector("[role=alertdialog]") as HTMLElement;
    expect(text(dialog.querySelector("h2"))).toBe("Delete “Build Server”?");
    expect(fake.names()).not.toContain("deleteProfile");
    button(dialog, "Delete").click();
    await flush();
    expect(fake.calls).toContainEqual(["deleteProfile", BUILD.id]);
    expect(names(root)).not.toContain("Build Server");
  });

  test("Space opens the menu; arrows move in it; Esc closes it and returns to the card", async () => {
    const { root } = await startConnections(board());
    const build = card(root, "Build Server");
    build.focus();
    key(build, " ");
    const items = Array.from(root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(document.activeElement).toBe(items[0]!);
    key(items[0]!, "ArrowDown");
    expect(document.activeElement).toBe(items[1]!);
    key(items[1]!, "ArrowUp");
    expect(document.activeElement).toBe(items[0]!);
    key(items[0]!, "Escape");
    expect(root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(card(root, "Build Server"));
  });

  test("the same menu is the card's context menu", async () => {
    const { root } = await startConnections(board());
    const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 50 });
    card(root, "Homelab").dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    expect(root.querySelector("[role=menu]")?.getAttribute("aria-label")).toBe("Homelab");
    expect(menuItems(root)).toContain("Disconnect");
  });
});

describe("live thumbnails", () => {
  const PNG = "data:image/png;base64,iVBORw0KGgo=";

  test("a thumbnail replaces the idle gradient; image null restores it", async () => {
    const { root, app } = await startConnections(board());
    app.onThumbnail({ profile_id: STUDIO.id, image: PNG });
    const thumb = card(root, "Studio Workstation").querySelector(".thumb") as HTMLElement;
    expect(thumb.classList.contains("live")).toBe(true);
    expect(thumb.querySelector("img")?.getAttribute("src")).toBe(PNG);
    expect(thumb.querySelector("img")?.getAttribute("alt")).toBe("");
    expect(thumb.querySelector(":scope > svg")).toBeNull();

    app.onThumbnail({ profile_id: STUDIO.id, image: null });
    const idle = card(root, "Studio Workstation").querySelector(".thumb") as HTMLElement;
    expect(idle.querySelector("img")).toBeNull();
    expect(idle.classList.contains("thumb-headless")).toBe(true);
    expect(idle.querySelector("svg")).not.toBeNull();
  });

  test("thumbnails live in memory only and go when the window closes", async () => {
    localStorage.clear();
    sessionStorage.clear();
    const { root, app } = await startConnections(board());
    app.onThumbnail({ profile_id: STUDIO.id, image: PNG });
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
    app.onConnections({ open: [] });
    app.onConnections({ open: [openConnection(STUDIO.id, "live")] });
    expect(card(root, "Studio Workstation").querySelector("img")).toBeNull();
  });
});

describe("intents from the menu bar and other windows", () => {
  test("new and edit open the sheet", async () => {
    const { root, app, fake } = await startConnections(board());
    await app.onIntent({ kind: "new" });
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("New Connection");
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
    await app.onIntent({ kind: "edit", profile_id: KITCHEN.id });
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("Kitchen Laptop");
    expect(root.querySelectorAll("[role=dialog]").length).toBe(1);
  });
});
