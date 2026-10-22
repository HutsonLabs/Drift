// IOS-8 Red: the iPhone key row under the picture — Esc, Tab, latching modifiers, arrows, the
// iOS keyboard for text, and the touch-mode switch — and when the session page shows it.
import { describe, expect, test } from "bun:test";
import { type SessionApi, SessionApp } from "../src/sessionApp";
import { renderKeyRow } from "../src/views/keyRow";
import { button, flush, hasButton, identity, sessionView } from "./helpers";

function row(touch = "trackpad") {
  const root = document.createElement("main");
  document.body.replaceChildren(root);
  const calls: unknown[][] = [];
  renderKeyRow(root, touch, {
    sendKeys: (keys) => calls.push(["sendKeys", keys]),
    typeText: (t) => calls.push(["typeText", t]),
    setTouchMode: (m) => calls.push(["setTouchMode", m]),
  });
  return { root, calls };
}

describe("iPhone key row", () => {
  test("plain keys send one key", () => {
    const { root, calls } = row();
    button(root, "Esc").click();
    button(root, "Tab").click();
    button(root, "Up").click();
    button(root, "Left").click();
    expect(calls).toEqual([
      ["sendKeys", ["escape"]],
      ["sendKeys", ["tab"]],
      ["sendKeys", ["up"]],
      ["sendKeys", ["left"]],
    ]);
  });

  test("modifiers latch until the next key, then release", () => {
    const { root, calls } = row();
    const ctrl = button(root, "Control");
    ctrl.click();
    expect(ctrl.getAttribute("aria-pressed")).toBe("true");
    button(root, "Alt").click();
    button(root, "Esc").click();
    expect(calls).toEqual([["sendKeys", ["control", "alt", "escape"]]]);
    expect(button(root, "Control").getAttribute("aria-pressed")).toBe("false");
    // Tapping a latched modifier again unlatches it without sending anything.
    button(root, "Super").click();
    button(root, "Super").click();
    button(root, "Tab").click();
    expect(calls.at(-1)).toEqual(["sendKeys", ["tab"]]);
  });

  test("the keyboard button brings up the iOS keyboard; typing goes out as text", () => {
    const { root, calls } = row();
    button(root, "Keyboard").click();
    const field = root.querySelector<HTMLInputElement>("input.type-field");
    expect(field).not.toBeNull();
    expect(document.activeElement).toBe(field);
    field!.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: "hi", cancelable: true }));
    field!.dispatchEvent(new InputEvent("beforeinput", { inputType: "deleteContentBackward", cancelable: true }));
    field!.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertLineBreak", cancelable: true }));
    expect(calls).toEqual([
      ["typeText", "hi"],
      ["sendKeys", ["backspace"]],
      ["sendKeys", ["enter"]],
    ]);
  });

  test("text typed with a latched modifier becomes a chord", () => {
    const { root, calls } = row();
    button(root, "Keyboard").click();
    button(root, "Control").click();
    const field = root.querySelector<HTMLInputElement>("input.type-field")!;
    field.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: "c", cancelable: true }));
    expect(calls).toEqual([["sendKeys", ["control", "c"]]]);
  });

  test("the touch switch flips between trackpad and direct", () => {
    let { root, calls } = row("trackpad");
    button(root, "Touch: trackpad").click();
    expect(calls).toEqual([["setTouchMode", "direct"]]);
    ({ root, calls } = row("direct"));
    button(root, "Touch: tap where you touch").click();
    expect(calls).toEqual([["setTouchMode", "trackpad"]]);
  });
});

describe("session page on iPhone", () => {
  const LIVE = { state: "connected", desktop: { width: 1280, height: 800 }, scale: 100 } as const;
  const ok = <T>(data: T) => Promise.resolve({ status: "ok" as const, data });

  test("shows the key row over a live picture while the controls are visible", async () => {
    const calls: unknown[][] = [];
    const api = {
      windowIdentity: () => ok(identity({})),
      sendKeys: (keys: string[]) => (calls.push(["sendKeys", keys]), ok(null)),
      typeText: (t: string) => (calls.push(["typeText", t]), ok(null)),
      setTouchMode: (m: string) => (calls.push(["setTouchMode", m]), ok(null)),
      phoneControls: () => Promise.resolve(null),
    } as unknown as SessionApi;
    const root = document.createElement("main");
    document.body.replaceChildren(root);
    const app = new SessionApp(root, api);
    await app.start();
    app.onSessionView(sessionView("live", LIVE));
    expect(hasButton(root, "Esc")).toBe(false);
    app.onPhoneControls({ visible: true, touch: "trackpad" });
    expect(document.body.dataset.controls).toBe("keys");
    button(root, "Esc").click();
    await flush();
    expect(calls).toEqual([["sendKeys", ["escape"]]]);
    app.onPhoneControls({ visible: false, touch: "trackpad" });
    expect(hasButton(root, "Esc")).toBe(false);
    expect(document.body.dataset.controls).toBe("");
  });
});

describe("session page start on iPhone", () => {
  test("asks for the controls it may have missed", async () => {
    const api = {
      windowIdentity: () => Promise.resolve({ status: "ok", data: identity({}) }),
      phoneControls: () => Promise.resolve({ visible: true, touch: "direct" }),
    } as unknown as SessionApi;
    const root = document.createElement("main");
    document.body.replaceChildren(root);
    const app = new SessionApp(root, api);
    await app.start();
    app.onSessionView(sessionView("live", { state: "connected", desktop: { width: 1280, height: 800 }, scale: 100 }));
    expect(hasButton(root, "Touch: tap where you touch")).toBe(true);
  });
});

describe("key row icons (RESP-14)", () => {
  test("the keyboard and touch-mode keys are icons with their names, not text glyphs", () => {
    for (const [touch, icon] of [
      ["trackpad", "icon-trackpad"],
      ["direct", "icon-hand"],
    ] as const) {
      const { root } = row(touch);
      const keyboard = button(root, "Keyboard");
      expect(keyboard.querySelector("svg.icon-keyboard")).not.toBeNull();
      expect((keyboard.textContent ?? "").trim()).toBe("");
      const mode = root.querySelector<HTMLButtonElement>(".key.touch-mode")!;
      expect(mode.querySelector(`svg.${icon}`)).not.toBeNull();
      expect((mode.textContent ?? "").trim()).toBe("");
    }
  });
});
