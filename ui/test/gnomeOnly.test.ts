// GNOME-only Red (docs/adr/GNOME-only.md): every target is a GNOME computer. No kind filter, no
// kind-first add sheet, no Mac tiles or badges; "+", ⌘N, the dashed card and first launch open
// the New Connection form directly, and a `drift-connect:` link from `host/drift-gnome-setup`
// opens it prefilled, certificate already pinned, with nothing saved until Add.
import { describe, expect, test } from "bun:test";
import { FakeConnectionsApi, startConnections } from "./fakes";
import { button, entry, field, flush, hasButton, openConnection, profile, text, type } from "./helpers";

const FEDORA = profile("headless", { name: "Fedora Server", host: "fedora.lan" });
const OFFICE = profile("remote-login", { name: "Office Desktop", host: "10.0.4.21" });

/** What `take_connect_link` / the prefill intent carry: a fresh, unsaved profile. */
const LINKED = profile("headless", {
  id: "22222222-0000-0000-0000-000000000001",
  name: "Workstation",
  host: "workstation.lan",
  port: 3390,
  rdp_username: "drift",
  cert_pin: "f3:e7:a2:68",
});

function board(): FakeConnectionsApi {
  const fake = new FakeConnectionsApi();
  fake.entries = [FEDORA, OFFICE].map((p) => entry(p));
  fake.open = [openConnection(FEDORA.id, "live", { live_secs: 720 })];
  return fake;
}

function card(root: ParentNode, name: string): HTMLElement {
  const c = Array.from(root.querySelectorAll<HTMLElement>("article.card")).find((a) => text(a.querySelector(".item-name")) === name);
  if (!c) throw new Error(`no card "${name}"`);
  return c;
}

function sheet(root: ParentNode): HTMLElement {
  const s = root.querySelector<HTMLElement>("[role=dialog]");
  if (!s) throw new Error("no sheet");
  return s;
}

function value(root: ParentNode, id: string): string {
  return root.querySelector<HTMLInputElement>(`[id="${id}"]`)?.value ?? "";
}

/** The sheet is the New Connection form, not a chooser. */
function expectNewConnection(root: ParentNode): void {
  expect(root.querySelectorAll("[role=dialog]").length).toBe(1);
  expect(root.querySelector("[role=dialog][aria-label='Add a Connection']")).toBeNull();
  expect(text(sheet(root).querySelector("h2"))).toBe("New Connection");
}

describe("gallery", () => {
  test("the filter is All / Open only", async () => {
    const { root } = await startConnections(board());
    const bar = root.querySelector("header.toolbar") as HTMLElement;
    const seg = Array.from(bar.querySelectorAll(".seg button")).map(text);
    expect(seg).toEqual(["All", "Open"]);
    expect(hasButton(bar, "Macs")).toBe(false);
    expect(hasButton(bar, "GNOME")).toBe(false);
  });

  test("the + button says what it adds", async () => {
    const { root } = await startConnections(board());
    expect(button(root.querySelector("header.toolbar")!, "New Connection").getAttribute("title")).toBe("Add a GNOME computer (⌘N)");
  });

  test("a card: the mode's glyph and gradient, host · mode, no kind badge", async () => {
    const { root } = await startConnections(board());
    const office = card(root, "Office Desktop");
    expect(office.querySelector(".card-info .glyph-remote-login")).not.toBeNull();
    expect(office.querySelector(".thumb .kind")).toBeNull();
    expect(text(office.querySelector(".item-detail"))).toBe("10.0.4.21 · Remote Login");
    expect(office.querySelector(".thumb")?.classList.contains("thumb-remote-login")).toBe(true);
    expect(office.getAttribute("aria-label")).toBe("Office Desktop, Remote Login, 10.0.4.21, Not connected");
  });
});

describe("adding", () => {
  test("+ opens the New Connection form directly", async () => {
    const { root, fake } = await startConnections(board());
    button(root.querySelector("header.toolbar")!, "New Connection").click();
    await flush();
    expectNewConnection(root);
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
  });

  test("⌘N (the new intent) and the dashed card open the form directly", async () => {
    const { root, app } = await startConnections(board());
    await app.onIntent({ kind: "new" });
    await flush();
    expectNewConnection(root);
    button(sheet(root), "Cancel").click();
    const add = root.querySelector<HTMLButtonElement>("button.card.add")!;
    expect(add.getAttribute("aria-label")).toBe("Add a GNOME computer");
    add.click();
    await flush();
    expectNewConnection(root);
  });
});

describe("drift-connect: links", () => {
  test("the prefill intent opens New Connection with the link's host, port and user", async () => {
    const { root, app, fake } = await startConnections(board());
    await app.onIntent({ kind: "prefill", profile: LINKED });
    await flush();
    expectNewConnection(root);
    expect(value(root, "profile-name")).toBe("Workstation");
    expect(value(root, "host")).toBe("workstation.lan");
    expect(value(root, "port")).toBe("3390");
    expect(value(root, "rdp-user")).toBe("drift");
    expect(text(sheet(root).querySelector(".fingerprint"))).toBe("f3:e7:a2:68");
    // Nothing is created or saved until Add.
    expect(fake.names()).not.toContain("newProfile");
    expect(fake.names()).not.toContain("saveProfile");
  });

  test("the prefill intent replaces a sheet that is already open", async () => {
    const { root, app } = await startConnections(board());
    await app.onIntent({ kind: "edit", profile_id: OFFICE.id });
    expect(text(sheet(root).querySelector("h2"))).toBe("Office Desktop");
    await app.onIntent({ kind: "prefill", profile: LINKED });
    await flush();
    expectNewConnection(root);
    expect(value(root, "host")).toBe("workstation.lan");
  });

  test("a link that launched the app opens on start, even with saved connections", async () => {
    const fake = board();
    fake.pendingLink = LINKED;
    const { root } = await startConnections(fake);
    expect(fake.names()).toContain("takeConnectLink");
    expectNewConnection(root);
    expect(value(root, "host")).toBe("workstation.lan");
  });

  test("first launch with a link opens it instead of an empty form", async () => {
    const fake = new FakeConnectionsApi();
    fake.pendingLink = LINKED;
    const { root } = await startConnections(fake);
    expectNewConnection(root);
    expect(value(root, "host")).toBe("workstation.lan");
    expect(fake.names()).not.toContain("newProfile");
  });

  test("Add saves the prefilled profile with its id and pinned certificate", async () => {
    const { root, app, fake } = await startConnections(board());
    await app.onIntent({ kind: "prefill", profile: LINKED });
    await flush();
    type(field(sheet(root), "RDP password"), "s3cret");
    await flush();
    button(sheet(root), "Add").click();
    await flush();
    const save = fake.calls.find((c) => c[0] === "saveProfile");
    expect(save?.[1]).toMatchObject({ id: LINKED.id, host: "workstation.lan", port: 3390, rdp_username: "drift", cert_pin: "f3:e7:a2:68" });
    expect(save?.[2]).toMatchObject({ rdp_password: "s3cret" });
    expect(fake.names()).not.toContain("connect");
  });

  test("Forget Certificate on an unsaved prefilled profile only drops the pin from the form", async () => {
    const { root, app, fake } = await startConnections(board());
    await app.onIntent({ kind: "prefill", profile: LINKED });
    await flush();
    button(sheet(root), "Forget Certificate").click();
    await flush();
    expect(fake.names()).not.toContain("forgetCertificate");
    expect(sheet(root).querySelector(".fingerprint")).toBeNull();
    expect(value(root, "host")).toBe("workstation.lan");
  });
});
