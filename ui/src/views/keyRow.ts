// IOS-8: the iPhone key row under (portrait) or over (landscape) the live picture. Keys a phone
// keyboard lacks — Esc, Tab, arrows — plus latching modifiers, the iOS keyboard for text, and the
// touch-mode switch. Modifiers latch: tap Control, then a key, and the chord goes out as one
// press-release sequence; the latch then clears. The page renders it; Rust lays the webview out
// as the key row's strip (`phone::phone_layout`). The modifiers and one-tap shortcuts follow the
// kind (UI-kinds I): Ctrl Alt ⇧ Super and Activities for GNOME, ⌃ ⌥ ⇧ ⌘ with ⌘Space and ⌘Tab
// for a Mac.
import type { HostKind } from "../bindings";
import { h, mount } from "../dom";

/** What the key row asks Rust to do (see `commands::send_keys` / `type_text` / `set_touch_mode`). */
export interface KeyRowActions {
  sendKeys(keys: string[]): void;
  typeText(text: string): void;
  setTouchMode(mode: string): void;
}

type Key = [key: string, label: string, name: string];

/** The latching modifiers by kind (UI-kinds I), in the order a chord presses them. */
const MODIFIERS: Record<HostKind, Key[]> = {
  gnome: [
    ["control", "Ctrl", "Control"],
    ["alt", "Alt", "Alt"],
    ["shift", "⇧", "Shift"],
    ["super", "Super", "Super"],
  ],
  // Drift Host maps the Super key to Command.
  mac: [
    ["control", "⌃", "Control"],
    ["alt", "⌥", "Option"],
    ["shift", "⇧", "Shift"],
    ["super", "⌘", "Command"],
  ],
};

/** One-tap shortcuts by kind: GNOME's Activities, the Mac's Spotlight and app switcher. */
const SHORTCUTS: Record<HostKind, [keys: string[], label: string, name: string][]> = {
  gnome: [[["super"], "Activities", "Activities"]],
  mac: [
    [["super", "space"], "⌘Space", "Command-Space"],
    [["super", "tab"], "⌘Tab", "Command-Tab"],
  ],
};

const KEYS: [key: string, label: string, name: string][] = [
  ["escape", "esc", "Esc"],
  ["tab", "⇥", "Tab"],
  ["left", "←", "Left"],
  ["up", "↑", "Up"],
  ["down", "↓", "Down"],
  ["right", "→", "Right"],
];

const TOUCH_NAMES: Record<string, string> = { trackpad: "trackpad", direct: "tap where you touch" };

/** Renders the key row for a `kind` of computer into `root`. */
export function renderKeyRow(root: HTMLElement, touch: string, actions: KeyRowActions, kind: HostKind = "gnome"): void {
  const latched = new Set<string>();
  const modifierButtons = new Map<string, HTMLButtonElement>();
  const modifiers = MODIFIERS[kind];

  const release = () => {
    latched.clear();
    for (const b of modifierButtons.values()) b.setAttribute("aria-pressed", "false");
  };
  const send = (key: string) => {
    actions.sendKeys([...modifiers.map(([k]) => k).filter((k) => latched.has(k)), key]);
    release();
  };

  const shortcutRow = SHORTCUTS[kind].map(([keys, label, name]) =>
    h("button", { type: "button", class: "key shortcut", "aria-label": name, onclick: () => (actions.sendKeys(keys), release()) }, label),
  );
  const modifierRow = modifiers.map(([key, label, name]) => {
    const b = h("button", { type: "button", class: "key modifier", "aria-label": name, "aria-pressed": "false" }, label);
    b.addEventListener("click", () => {
      if (latched.has(key)) latched.delete(key);
      else latched.add(key);
      b.setAttribute("aria-pressed", String(latched.has(key)));
    });
    modifierButtons.set(key, b);
    return b;
  });
  const keyRow = KEYS.map(([key, label, name]) =>
    h("button", { type: "button", class: "key", "aria-label": name, onclick: () => send(key) }, label),
  );

  // The iOS keyboard needs a focused text field; it stays empty, every edit is sent at once.
  const field = h("input", {
    class: "type-field",
    type: "text",
    autocapitalize: "off",
    autocomplete: "off",
    autocorrect: "off",
    spellcheck: "false",
    "aria-label": "Text to type on the remote computer",
  });
  field.addEventListener("beforeinput", (event) => {
    const e = event as InputEvent;
    e.preventDefault();
    if (e.inputType === "insertText" && e.data) {
      if (latched.size > 0 && e.data.length === 1) send(e.data.toLowerCase());
      else actions.typeText(e.data);
    } else if (e.inputType === "deleteContentBackward") {
      send("backspace");
    } else if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") {
      send("enter");
    }
  });
  const keyboard = h(
    "button",
    { type: "button", class: "key", "aria-label": "Keyboard", onclick: () => field.focus() },
    "⌨",
  );
  const next = touch === "trackpad" ? "direct" : "trackpad";
  const touchSwitch = h(
    "button",
    {
      type: "button",
      class: "key touch-mode",
      "aria-label": `Touch: ${TOUCH_NAMES[touch] ?? touch}`,
      onclick: () => actions.setTouchMode(next),
    },
    touch === "trackpad" ? "▭" : "☝",
  );

  mount(
    root,
    h("div", { class: "key-row", role: "toolbar", "aria-label": "Keys" }, ...modifierRow, ...shortcutRow, ...keyRow, keyboard, touchSwitch, field),
  );
}
