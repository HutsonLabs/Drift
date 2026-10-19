// "Add a Connection" (UI-kinds B): "+", ⌘N, the dashed card and first launch ask which kind of
// computer comes next — a Mac running Drift Host (the pairing sheet) or a GNOME computer (the
// New Connection form). Esc and Cancel close it.
import { h, mount } from "../dom";
import { kindGlyph } from "../icons";

/** What the chooser can ask for. */
export interface AddIntents {
  pairMac(): void;
  addGnome(): void;
  cancel(): void;
}

/** Renders the chooser into `root` and focuses its first choice. */
export function renderAddSheet(root: HTMLElement, on: AddIntents): void {
  const choice = (kind: "mac" | "gnome", title: string, about: string, action: string, run: () => void) =>
    h(
      "div",
      { class: `add-choice add-${kind}` },
      kindGlyph(kind, "large"),
      h("div", { class: "add-text" }, h("h3", {}, title), h("p", {}, about)),
      h("button", { type: "button", class: kind === "mac" ? "primary" : null, onclick: () => run() }, action),
    );
  const dialog = h(
    "section",
    { class: "cfg add-sheet", role: "dialog", "aria-modal": "true", "aria-label": "Add a Connection" },
    h("header", { class: "cfg-head" }, h("div", { class: "cfg-title" }, h("h2", {}, "Add a Connection"), h("p", { class: "meta" }, "What are you connecting to?"))),
    h(
      "div",
      { class: "cfg-body" },
      choice("mac", "A Mac running Drift Host", "Pair once with the code shown on the Mac. Drift pins its key; no RDP passwords.", "Pair a Mac…", on.pairMac),
      choice(
        "gnome",
        "A GNOME computer",
        "GNOME Remote Desktop 50 or later: Headless, Desktop Sharing or Remote Login, with the RDP credentials set on the host.",
        "Add GNOME Computer…",
        on.addGnome,
      ),
    ),
    h("footer", { class: "cfg-foot" }, h("span", { class: "spacer" }), h("button", { type: "button", onclick: () => on.cancel() }, "Cancel")),
  );
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      on.cancel();
    }
  });
  mount(root, h("div", { class: "scrim sheet-scrim" }, dialog));
  dialog.querySelector<HTMLButtonElement>(".add-choice button")?.focus();
}
