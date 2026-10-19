// IOS-9 Red: "Pair a Mac" on the Connections page (ADR IOS-9). Discovered Drift Hosts are
// listed while the sheet is open; pairing takes a typed code with a chosen host, or a scanned
// (or pasted) QR payload on its own, plus the Mac account name; the new paired connection shows
// as Trusted. Test values only.
import { describe, expect, test } from "bun:test";
import type { HostAdvert } from "../src/bindings";
import { FakeConnectionsApi, pairMac, startConnections } from "./fakes";
import { button, field, flush, hasButton, text, type } from "./helpers";

const STUDIO: HostAdvert = {
  service_name: "Studio Mac",
  host_id: "00000000-0000-0000-0000-000000000077",
  name: "Studio Mac",
  host: "studio.local",
  port: 3389,
  fingerprint: "ab:ab",
};

async function pairSheet(fake = new FakeConnectionsApi()) {
  const started = await startConnections(fake);
  await pairMac(started.root);
  const sheet = started.root.querySelector<HTMLElement>("[role=dialog][aria-label='Pair a Mac']");
  if (!sheet) throw new Error("no pairing sheet");
  return { ...started, sheet };
}

describe("Pair a Mac", () => {
  test("opening the sheet starts discovery and closing it stops discovery", async () => {
    const { sheet, fake } = await pairSheet();
    expect(fake.names()).toContain("startHostDiscovery");
    expect(text(sheet)).toContain("Looking for Macs running Drift Host");
    button(sheet, "Cancel").click();
    await flush();
    expect(fake.names()).toContain("stopHostDiscovery");
  });

  test("discovered hosts are listed and one can be chosen", async () => {
    const { sheet, app } = await pairSheet();
    app.onHostsDiscovered([STUDIO]);
    await flush();
    const host = button(sheet, "Studio Mac studio.local");
    host.click();
    expect(host.getAttribute("aria-pressed")).toBe("true");
  });

  test("a typed code pairs the chosen host", async () => {
    const { sheet, app, fake } = await pairSheet();
    app.onHostsDiscovered([STUDIO]);
    await flush();
    button(sheet, "Studio Mac studio.local").click();
    type(field(sheet, "Code shown on the Mac"), "7654-3215");
    type(field(sheet, "Account on the Mac"), "mac-user");
    await flush();
    button(sheet, "Pair").click();
    await flush();
    const call = fake.calls.find((c) => c[0] === "pairHost")!;
    expect(call[1]).toEqual({ kind: "advert", advert: STUDIO, code: "7654 3215", username: "mac-user", password: null });
    expect(document.querySelector("[role=dialog][aria-label='Pair a Mac']")).toBeNull();
  });

  test("Pair stays disabled until a host, a valid code and an account are given", async () => {
    const { sheet, app } = await pairSheet();
    const pair = button(sheet, "Pair");
    expect(pair.disabled).toBe(true);
    app.onHostsDiscovered([STUDIO]);
    await flush();
    button(sheet, "Studio Mac studio.local").click();
    type(field(sheet, "Code shown on the Mac"), "7654 3216");
    type(field(sheet, "Account on the Mac"), "mac-user");
    await flush();
    expect(text(sheet)).toContain("That code has a typo.");
    expect(button(sheet, "Pair").disabled).toBe(true);
  });

  test("a pasted QR payload needs no host from the list", async () => {
    const { sheet, fake } = await pairSheet();
    type(field(sheet, "Code shown on the Mac"), "drift-pair:1?id=x");
    type(field(sheet, "Account on the Mac"), "mac-user");
    await flush();
    expect(text(sheet)).toContain("Studio Mac");
    button(sheet, "Pair").click();
    await flush();
    const call = fake.calls.find((c) => c[0] === "pairHost")!;
    expect(call[1]).toEqual({ kind: "qr", uri: "drift-pair:1?id=x", username: "mac-user", password: null });
  });

  test("Scan Code reads the QR payload from the camera", async () => {
    const fake = new FakeConnectionsApi();
    fake.scanned = "drift-pair:1?id=scanned";
    const { sheet } = await pairSheet(fake);
    button(sheet, "Scan Code").click();
    await flush();
    expect(fake.names()).toContain("readPairingCode");
    expect(field(sheet, "Code shown on the Mac").value).toBe("drift-pair:1?id=scanned");
  });

  test("a camera failure is shown, not thrown", async () => {
    const { sheet } = await pairSheet();
    button(sheet, "Scan Code").click();
    await flush();
    expect(text(sheet)).toContain("No camera.");
  });

  test("a refused pairing keeps the sheet open with the reason", async () => {
    const fake = new FakeConnectionsApi();
    fake.pairError = { kind: "pairing", message: "The Mac did not accept the code." };
    const { sheet } = await pairSheet(fake);
    type(field(sheet, "Code shown on the Mac"), "drift-pair:1?id=x");
    type(field(sheet, "Account on the Mac"), "mac-user");
    await flush();
    button(sheet, "Pair").click();
    await flush();
    expect(text(sheet)).toContain("The Mac did not accept the code.");
    expect(hasButton(sheet, "Pair")).toBe(true);
  });
});

describe("Pair a Mac from a drift-pair: link", () => {
  test("the pair intent opens the sheet with the link as the code", async () => {
    const started = await startConnections(new FakeConnectionsApi());
    await started.app.onIntent({ kind: "pair", uri: "drift-pair:1?id=link" });
    await flush();
    const sheet = started.root.querySelector<HTMLElement>("[role=dialog][aria-label='Pair a Mac']")!;
    expect(field(sheet, "Code shown on the Mac").value).toBe("drift-pair:1?id=link");
    expect(text(sheet)).toContain("Pairs with Studio Mac");
  });

  test("a link that launched the app is picked up at start", async () => {
    const fake = new FakeConnectionsApi();
    fake.pendingLink = "drift-pair:1?id=cold";
    const started = await startConnections(fake);
    const sheet = started.root.querySelector<HTMLElement>("[role=dialog][aria-label='Pair a Mac']")!;
    expect(field(sheet, "Code shown on the Mac").value).toBe("drift-pair:1?id=cold");
  });
});
