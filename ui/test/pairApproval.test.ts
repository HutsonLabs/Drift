// HOST-15 Red: set up a Mac from the device (ADR HOST-15-guided-setup). A Drift Host that can
// approve devices needs no code: choose it, sign in with the Mac's account (filled in from its
// advert), and Drift connects at once while the Mac's user clicks Allow. Older hosts still need
// their code. Test values only.
import { describe, expect, test } from "bun:test";
import type { HostAdvert } from "../src/bindings";
import { renderConnecting, stepsFor } from "../src/views/connecting";
import { FakeConnectionsApi, pairMac, startConnections } from "./fakes";
import { button, field, flush, sessionView, text, type } from "./helpers";

const STUDIO: HostAdvert = {
  service_name: "Studio Mac",
  host_id: "00000000-0000-0000-0000-000000000077",
  name: "Studio Mac",
  host: "studio.local",
  port: 3389,
  fingerprint: "ab:ab",
  account: "andrew",
  approval: true,
};

const OLD: HostAdvert = { ...STUDIO, host_id: "00000000-0000-0000-0000-000000000078", name: "Old Mac", host: "old.local", account: null, approval: false };

async function pairSheet() {
  const started = await startConnections(new FakeConnectionsApi());
  await pairMac(started.root);
  const sheet = started.root.querySelector<HTMLElement>("[role=dialog][aria-label='Pair a Mac']");
  if (!sheet) throw new Error("no pairing sheet");
  return { ...started, sheet };
}

describe("Set up a Mac from the device", () => {
  test("choosing a Mac that approves needs no code and fills in its account", async () => {
    const { sheet, app, fake } = await pairSheet();
    app.onHostsDiscovered([STUDIO, OLD]);
    await flush();
    button(sheet, "Studio Mac studio.local").click();
    await flush();
    expect(field(sheet, "Account on the Mac").value).toBe("andrew");
    expect(text(sheet)).toContain("Click Allow on Studio Mac");
    type(field(sheet, "Password (optional)"), "fake-pw");
    await flush();
    const pair = button(sheet, "Pair");
    expect(pair.disabled).toBe(false);
    pair.click();
    await flush();
    const call = fake.calls.find((c) => c[0] === "pairHost")!;
    expect(call[1]).toEqual({ kind: "approve", advert: STUDIO, username: "andrew", password: "fake-pw" });
    // Drift connects straight away; the session window says to click Allow on the Mac.
    const paired = fake.entries.at(-1)!.profile.id;
    expect(fake.calls).toContainEqual(["connect", paired]);
    expect(document.querySelector("[role=dialog][aria-label='Pair a Mac']")).toBeNull();
  });

  test("a typed account is not overwritten by the advert", async () => {
    const { sheet, app } = await pairSheet();
    type(field(sheet, "Account on the Mac"), "someone-else");
    app.onHostsDiscovered([STUDIO]);
    await flush();
    button(sheet, "Studio Mac studio.local").click();
    await flush();
    expect(field(sheet, "Account on the Mac").value).toBe("someone-else");
  });

  test("an older Mac still needs the code from its screen", async () => {
    const { sheet, app } = await pairSheet();
    app.onHostsDiscovered([STUDIO, OLD]);
    await flush();
    button(sheet, "Old Mac old.local").click();
    type(field(sheet, "Account on the Mac"), "mac-user");
    await flush();
    expect(button(sheet, "Pair").disabled).toBe(true);
    expect(text(sheet)).not.toContain("Click Allow on");
  });

  test("a code typed for a Mac that approves still pairs with the code", async () => {
    const { sheet, app, fake } = await pairSheet();
    app.onHostsDiscovered([STUDIO]);
    await flush();
    button(sheet, "Studio Mac studio.local").click();
    type(field(sheet, "Code shown on the Mac"), "7654 3215");
    await flush();
    button(sheet, "Pair").click();
    await flush();
    const call = fake.calls.find((c) => c[0] === "pairHost")!;
    expect((call[1] as { kind: string }).kind).toBe("advert");
  });
});

describe("waiting for the Mac", () => {
  test("the connecting screen asks the user to click Allow on the Mac", () => {
    const r = document.createElement("main");
    document.body.replaceChildren(r);
    renderConnecting(r, sessionView("connecting", { state: "connecting", leg: 1, stage: "approval" }), { cancel: () => {} });
    expect(text(r.querySelector("[role=status]"))).toBe("Click Allow on the Mac to finish pairing…");
    // Every earlier step is done.
    expect(r.querySelectorAll(".stage.done").length).toBe(stepsFor("headless").length);
  });
});
