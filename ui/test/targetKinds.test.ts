// UI-kinds Red (docs/design/mockup-targets.html, part 2): two kinds of target, a Mac running
// Drift Host and a GNOME computer. The kind (`paired_host` set or not) drives the gallery filter,
// the add flow, the cards, the configuration sheet, the title bar and the iPhone key row.
import { describe, expect, test } from "bun:test";
import type { WindowIdentity } from "../src/bindings";
import { kindOf } from "../src/kinds";
import { type SessionApi, SessionApp } from "../src/sessionApp";
import { TitlebarApp, type TitlebarApi } from "../src/titlebarApp";
import { renderKeyRow } from "../src/views/keyRow";
import { FakeConnectionsApi, startConnections } from "./fakes";
import { button, entry, flush, hasButton, identity, labels, openConnection, profile, sessionView, text } from "./helpers";

const MINI = profile("headless", { name: "Mac mini", host: "mac-mini.local", paired_host: "11111111-0000-0000-0000-000000000001", rdp_username: "hutson", cert_pin: "7a:1c" });
const MBP = profile("headless", { name: "MacBook Pro", host: "macbook-pro.local", paired_host: "11111111-0000-0000-0000-000000000002" });
const FEDORA = profile("headless", { name: "Fedora Server", host: "fedora.lan" });
const OFFICE = profile("remote-login", { name: "Office Desktop", host: "10.0.4.21" });

function board(): FakeConnectionsApi {
  const fake = new FakeConnectionsApi();
  fake.entries = [FEDORA, MBP, MINI, OFFICE].map((p) => entry(p));
  fake.open = [openConnection(MBP.id, "live", { live_secs: 720 })];
  return fake;
}

function card(root: ParentNode, name: string): HTMLElement {
  const c = Array.from(root.querySelectorAll<HTMLElement>("article.card")).find((a) => text(a.querySelector(".item-name")) === name);
  if (!c) throw new Error(`no card "${name}"`);
  return c;
}

function names(root: ParentNode): string[] {
  return Array.from(root.querySelectorAll("article.card .item-name")).map(text);
}

function dialog(root: ParentNode, label: string): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[role=dialog][aria-label="${label}"]`);
}

describe("the kind", () => {
  test("a paired profile is a Mac; any other is a GNOME computer", () => {
    expect(kindOf(MINI)).toBe("mac");
    expect(kindOf(FEDORA)).toBe("gnome");
    expect(kindOf({ ...FEDORA, paired_host: null })).toBe("gnome");
  });
});

describe("gallery (A–D)", () => {
  test("the filter is All / Macs / GNOME / Open and filters by kind", async () => {
    const { root } = await startConnections(board());
    const bar = root.querySelector("header.toolbar") as HTMLElement;
    for (const label of ["All", "Macs", "GNOME", "Open"]) expect(hasButton(bar, label)).toBe(true);
    button(bar, "Macs").click();
    expect(button(bar, "Macs").getAttribute("aria-pressed")).toBe("true");
    expect(names(root)).toEqual(["MacBook Pro", "Mac mini"]);
    button(bar, "GNOME").click();
    expect(names(root)).toEqual(["Fedora Server", "Office Desktop"]);
    button(bar, "All").click();
    expect(names(root).length).toBe(4);
  });

  test("one add button: no separate Pair a Mac icon in the toolbar", async () => {
    const { root } = await startConnections(board());
    const bar = root.querySelector("header.toolbar") as HTMLElement;
    expect(hasButton(bar, "Pair a Mac")).toBe(false);
    expect(hasButton(bar, "New Connection")).toBe(true);
  });

  test("a Mac card: Mac glyph and badge, Drift Host detail, a Mac tile while idle", async () => {
    const { root } = await startConnections(board());
    const mini = card(root, "Mac mini");
    expect(mini.querySelector(".card-info .glyph-mac")).not.toBeNull();
    expect(text(mini.querySelector(".thumb .kind"))).toBe("Mac");
    expect(text(mini.querySelector(".item-detail"))).toBe("Drift Host · mac-mini.local");
    expect(mini.querySelector(".thumb")?.classList.contains("thumb-mac")).toBe(true);
    expect(mini.getAttribute("aria-label")).toBe("Mac mini, Mac running Drift Host, mac-mini.local, Not connected");
  });

  test("a GNOME card: GNOME glyph and badge, host · mode, the mode's gradient", async () => {
    const { root } = await startConnections(board());
    const office = card(root, "Office Desktop");
    expect(office.querySelector(".card-info .glyph-gnome")).not.toBeNull();
    expect(text(office.querySelector(".thumb .kind"))).toBe("GNOME");
    expect(text(office.querySelector(".item-detail"))).toBe("10.0.4.21 · Remote Login");
    expect(office.querySelector(".thumb")?.classList.contains("thumb-remote-login")).toBe(true);
  });
});

describe("add flow (B)", () => {
  test("+ asks which kind first; GNOME opens the form, Mac opens pairing", async () => {
    const { root, fake } = await startConnections(board());
    button(root.querySelector("header.toolbar")!, "New Connection").click();
    await flush();
    const chooser = dialog(root, "Add a Connection");
    expect(chooser).not.toBeNull();
    expect(text(chooser)).toContain("What are you connecting to?");
    expect(text(chooser)).toContain("A Mac running Drift Host");
    expect(text(chooser)).toContain("A GNOME computer");
    expect(fake.names()).not.toContain("newProfile");

    button(chooser!, "Add GNOME Computer…").click();
    await flush();
    expect(dialog(root, "Add a Connection")).toBeNull();
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
    expect(text(root.querySelector("[role=dialog] h2"))).toBe("New Connection");
  });

  test("Pair a Mac… opens the pairing sheet and starts discovery", async () => {
    const { root, fake } = await startConnections(board());
    button(root.querySelector("header.toolbar")!, "New Connection").click();
    await flush();
    button(dialog(root, "Add a Connection")!, "Pair a Mac…").click();
    await flush();
    expect(dialog(root, "Pair a Mac")).not.toBeNull();
    expect(fake.names()).toContain("startHostDiscovery");
  });

  test("Esc and Cancel close the chooser", async () => {
    const { root } = await startConnections(board());
    button(root.querySelector("header.toolbar")!, "New Connection").click();
    await flush();
    button(dialog(root, "Add a Connection")!, "Cancel").click();
    expect(dialog(root, "Add a Connection")).toBeNull();
    button(root.querySelector("header.toolbar")!, "New Connection").click();
    await flush();
    dialog(root, "Add a Connection")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(dialog(root, "Add a Connection")).toBeNull();
  });

  test("⌘N (the new intent) and the dashed card open the chooser", async () => {
    const { root, app } = await startConnections(board());
    await app.onIntent({ kind: "new" });
    await flush();
    expect(dialog(root, "Add a Connection")).not.toBeNull();
    button(dialog(root, "Add a Connection")!, "Cancel").click();
    const add = root.querySelector<HTMLButtonElement>("button.card.add")!;
    expect(text(add)).toContain("Add a Mac or GNOME computer");
    add.click();
    await flush();
    expect(dialog(root, "Add a Connection")).not.toBeNull();
  });

  test("first launch: the welcome offers both kinds and the chooser opens", async () => {
    const { root, fake } = await startConnections(new FakeConnectionsApi());
    expect(text(root.querySelector(".welcome h2"))).toBe("Add a Mac or GNOME computer");
    expect(dialog(root, "Add a Connection")).not.toBeNull();
    expect(fake.names()).not.toContain("newProfile");
  });
});

describe("sheet (E)", () => {
  async function edit(p: typeof MINI) {
    const started = await startConnections(board());
    await started.app.onIntent({ kind: "edit", profile_id: p.id });
    await flush();
    return { ...started, sheet: started.root.querySelector<HTMLElement>("[role=dialog]")! };
  }

  test("a Mac's sheet: pairing and Mac settings, none of the GNOME ones", async () => {
    const { sheet } = await edit(MINI);
    expect(sheet.querySelector(".cfg-head .glyph-mac")).not.toBeNull();
    expect(text(sheet.querySelector(".cfg-head .meta"))).toContain("Drift Host · mac-mini.local");
    const all = text(sheet);
    expect(all).toContain("Pairing");
    expect(all).toContain("Paired as “hutson”");
    expect(all).toContain("Host key (SHA-256)");
    expect(all).toContain("7a:1c");
    expect(all).not.toContain("grdctl");
    expect(all).not.toContain("Connection type");
    expect(all).not.toContain("GNOME");
    expect(sheet.querySelector("input[name=mode]")).toBeNull();
    expect(sheet.querySelector("#rdp-password")).toBeNull();
    expect(sheet.querySelector("#cmd-as")).toBeNull();
    expect(sheet.querySelector("#mac-layout")).toBeNull();
    const l = labels(sheet);
    expect(l).toContain("Name");
    expect(l).toContain("Fit the Mac’s screen to this window");
    expect(l).toContain("Use Retina resolution");
    expect(l).toContain("Clipboard");
    expect(l).toContain("Touch on iPhone and iPad");
    expect(all).toContain("⌘ ⌥ ⌃ ⇧ and the keyboard layout go to the Mac unchanged.");
    expect(hasButton(sheet, "Unpair…")).toBe(true);
    expect(hasButton(sheet, "Forget Certificate")).toBe(false);
  });

  test("Unpair… asks, then removes the Mac", async () => {
    const { root, sheet, fake } = await edit(MINI);
    button(sheet, "Unpair…").click();
    const confirm = root.querySelector<HTMLElement>(".confirm-layer [role=alertdialog], .confirm-layer [role=dialog]")!;
    expect(text(confirm)).toContain("Unpair “Mac mini”?");
    button(confirm, "Unpair").click();
    await flush();
    expect(fake.calls).toContainEqual(["deleteProfile", MINI.id]);
  });

  test("saving a Mac keeps its pairing, user and mode", async () => {
    const { sheet, fake } = await edit(MINI);
    const name = sheet.querySelector<HTMLInputElement>("#profile-name")!;
    name.value = "Mini";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    sheet.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await flush();
    const save = fake.calls.find((c) => c[0] === "saveProfile");
    expect(save?.[1]).toMatchObject({ name: "Mini", paired_host: MINI.paired_host, rdp_username: "hutson", mode: "headless", host: "mac-mini.local" });
  });

  test("a GNOME sheet: “GNOME session” picker and “Render at 2× (Retina)”", async () => {
    const { sheet } = await edit(FEDORA);
    expect(sheet.querySelector(".cfg-head .glyph-gnome")).not.toBeNull();
    expect(text(sheet.querySelector("fieldset.mode legend"))).toBe("GNOME session");
    expect(labels(sheet)).toContain("Render at 2× (Retina)");
    expect(sheet.querySelector("#rdp-password")).not.toBeNull();
    expect(hasButton(sheet, "Delete…")).toBe(true);
  });
});

class FakeTitlebarApi {
  constructor(public current: WindowIdentity) {}
  api(): TitlebarApi {
    const ok = <T>(data: T) => Promise.resolve({ status: "ok" as const, data });
    return {
      windowIdentity: () => ok(this.current),
      showConnections: () => ok(null),
      toggleStats: () => ok(null),
      focusContent: () => ok(null),
    } as unknown as TitlebarApi;
  }
}

async function titlebar(id: WindowIdentity) {
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  const app = new TitlebarApp(root, new FakeTitlebarApi(id).api());
  await app.start();
  await flush();
  return { root, app };
}

describe("title bar (F)", () => {
  test("GNOME: GNOME glyph, name and the GNOME session mode", async () => {
    const { root, app } = await titlebar(identity({ kind: "gnome", mode: "headless", name: "Studio Workstation", host: "studio.lan" }));
    const capsule = root.querySelector(".ident") as HTMLElement;
    expect(capsule.querySelector(".glyph-gnome")).not.toBeNull();
    expect(text(capsule.querySelector(".detail"))).toBe("Headless");
    expect(capsule.getAttribute("aria-label")).toBe("Studio Workstation, GNOME, Headless, Live");
    app.dispose();
  });

  test("Mac: Mac glyph, name and the host's live quality step", async () => {
    const { root, app } = await titlebar(identity({ kind: "mac", name: "MacBook Pro", host: "macbook-pro.local", quality: "best" }));
    const capsule = root.querySelector(".ident") as HTMLElement;
    expect(capsule.querySelector(".glyph-mac")).not.toBeNull();
    expect(text(capsule.querySelector(".detail"))).toBe("Sharp");
    app.onIdentity(identity({ kind: "mac", name: "MacBook Pro", quality: "medium" }));
    expect(text(root.querySelector(".ident .detail"))).toBe("Smooth");
    app.onIdentity(identity({ kind: "mac", name: "MacBook Pro", quality: null }));
    expect(text(root.querySelector(".ident .detail"))).toBe("Drift Host");
    expect(root.querySelector(".ident")?.getAttribute("aria-label")).toBe("MacBook Pro, Mac, Drift Host, Live");
    app.dispose();
  });
});

function row(kind: "mac" | "gnome") {
  const root = document.createElement("main");
  document.body.replaceChildren(root);
  const calls: unknown[][] = [];
  renderKeyRow(
    root,
    "trackpad",
    {
      sendKeys: (keys) => calls.push(keys),
      typeText: () => {},
      setTouchMode: () => {},
    },
    kind,
  );
  return { root, calls };
}

describe("iPhone key row (I)", () => {
  test("GNOME: Ctrl Alt ⇧ Super and an Activities key", () => {
    const { root, calls } = row("gnome");
    const keys = Array.from(root.querySelectorAll(".key")).map(text);
    expect(keys.slice(0, 5)).toEqual(["Ctrl", "Alt", "⇧", "Super", "Activities"]);
    button(root, "Activities").click();
    button(root, "Shift").click();
    button(root, "Tab").click();
    expect(calls).toEqual([["super"], ["shift", "tab"]]);
  });

  test("Mac: ⌃ ⌥ ⇧ ⌘ with ⌘Space and ⌘Tab", () => {
    const { root, calls } = row("mac");
    const keys = Array.from(root.querySelectorAll(".key")).map(text);
    expect(keys.slice(0, 6)).toEqual(["⌃", "⌥", "⇧", "⌘", "⌘Space", "⌘Tab"]);
    expect(hasButton(root, "Option")).toBe(true);
    expect(hasButton(root, "Command")).toBe(true);
    button(root, "Command-Space").click();
    button(root, "Command-Tab").click();
    button(root, "Option").click();
    button(root, "Left").click();
    expect(calls).toEqual([["super", "space"], ["super", "tab"], ["alt", "left"]]);
  });

  test("the session page picks the row from the window's kind", async () => {
    const ok = <T>(data: T) => Promise.resolve({ status: "ok" as const, data });
    const api = {
      windowIdentity: () => ok(identity({ kind: "mac" })),
      phoneControls: () => Promise.resolve({ visible: true, touch: "trackpad" }),
      sendKeys: () => ok(null),
    } as unknown as SessionApi;
    const root = document.createElement("main");
    document.body.replaceChildren(root);
    const app = new SessionApp(root, api);
    await app.start();
    app.onSessionView(sessionView("live", { state: "connected", desktop: { width: 1280, height: 800 }, scale: 100 }));
    expect(hasButton(root, "Command-Space")).toBe(true);
  });
});
