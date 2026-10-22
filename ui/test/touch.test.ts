// RESP-7 Red: on a touch screen a card connects on a single tap and a long press opens its menu
// (findings F9). With a mouse or trackpad nothing changes: double-click and the context menu.
import { afterEach, describe, expect, test } from "bun:test";
import { FakeConnectionsApi, startConnections } from "./fakes";
import { entry, flush, openConnection, profile } from "./helpers";

const realMatchMedia = window.matchMedia.bind(window);

/** Makes `(pointer: coarse)` match, or not. */
function pointer(coarse: boolean): void {
  window.matchMedia = ((q: string) => {
    const matches = q.includes("pointer: coarse") ? coarse : realMatchMedia(q).matches;
    return { matches, media: q, addEventListener() {}, removeEventListener() {} } as unknown as MediaQueryList;
  }) as typeof window.matchMedia;
}

afterEach(() => {
  window.matchMedia = realMatchMedia;
});

const BUILD = profile("headless", { name: "Build box" });
const LAB = profile("remote-login", { name: "Lab" });

async function start() {
  const fake = new FakeConnectionsApi();
  fake.entries = [entry(BUILD), entry(LAB)];
  fake.open = [openConnection(LAB.id, "live")];
  const started = await startConnections(fake);
  const card = (id: string) => started.root.querySelector<HTMLElement>(`.card[data-profile-id="${id}"]`)!;
  return { ...started, card };
}

function press(el: HTMLElement, type: string): void {
  el.dispatchEvent(new PointerEvent(type, { pointerType: "touch", bubbles: true, clientX: 40, clientY: 40 }));
}

describe("touch", () => {
  test("a tap on a saved card connects", async () => {
    pointer(true);
    const { card, fake } = await start();
    card(BUILD.id).querySelector<HTMLElement>(".item-name")!.click();
    await flush();
    expect(fake.calls).toContainEqual(["connect", BUILD.id]);
  });

  test("a tap on an open card shows its window", async () => {
    pointer(true);
    const { card, fake } = await start();
    card(LAB.id).querySelector<HTMLElement>(".thumb")!.click();
    await flush();
    expect(fake.calls).toContainEqual(["showWindow", LAB.id]);
  });

  test("a tap on ⋯ opens the menu, not the connection", async () => {
    pointer(true);
    const { root, card, fake } = await start();
    card(BUILD.id).querySelector<HTMLElement>(".more")!.click();
    await flush();
    expect(root.querySelector("[role=menu]")).not.toBeNull();
    expect(fake.names()).not.toContain("connect");
  });

  test("a long press opens the menu and the tap that ends it does not connect", async () => {
    pointer(true);
    const { root, card, fake } = await start();
    const c = card(BUILD.id);
    press(c, "pointerdown");
    await new Promise((r) => setTimeout(r, 600));
    expect(root.querySelector("[role=menu]")).not.toBeNull();
    press(c, "pointerup");
    c.click();
    await flush();
    expect(fake.names()).not.toContain("connect");
  });

  test("a short press is a tap, not a long press", async () => {
    pointer(true);
    const { root, card } = await start();
    const c = card(BUILD.id);
    press(c, "pointerdown");
    await new Promise((r) => setTimeout(r, 100));
    press(c, "pointerup");
    await new Promise((r) => setTimeout(r, 600));
    expect(root.querySelector("[role=menu]")).toBeNull();
  });

  test("with a mouse a single click still does nothing (double-click connects)", async () => {
    pointer(false);
    const { card, fake } = await start();
    card(BUILD.id).querySelector<HTMLElement>(".item-name")!.click();
    await flush();
    expect(fake.names()).not.toContain("connect");
  });
});
