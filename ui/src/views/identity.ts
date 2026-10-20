// A session window's title bar (UI-windows boards 4, 5, 7; decisions 5 and 6): the centred
// identity capsule (mode glyph, name, mode, then a status dot, or a spinner while connecting;
// the greeter hint, else the host, as its tooltip) and, on the right, Show Connections and
// Statistics. The whole bar drags the window; nothing in it takes the keyboard (the title bar
// hands focus back).
import type { WindowIdentity } from "../bindings";
import { h, mount } from "../dom";
import { icon, modeGlyph } from "../icons";
import { SHORT_MODE_NAMES, STATUS_NAMES } from "../modes";

/** What the title bar can ask for. */
export interface IdentityIntents {
  showConnections(): void;
  toggleStats(): void;
}

/** Renders the title bar for `id` into `root`, replacing its contents. */
export function renderIdentity(root: HTMLElement, id: WindowIdentity, on: IdentityIntents): void {
  const indicator =
    id.status === "connecting"
      ? h("span", { class: "spin", "aria-hidden": "true" })
      : id.status === "idle"
        ? null
        : h("span", { class: `status ${id.status}`, "aria-hidden": "true" });
  const detail = SHORT_MODE_NAMES[id.mode];
  const capsule = h(
    "div",
    {
      class: "ident",
      role: "group",
      "aria-label": `${id.name}, ${detail}, ${STATUS_NAMES[id.status]}`,
      title: id.hint ?? id.host,
      "data-tauri-drag-region": true,
    },
    modeGlyph(id.mode, "small"),
    h("span", { class: "name", "data-tauri-drag-region": true }, id.name),
    h("span", { class: "detail", "data-tauri-drag-region": true }, detail),
    indicator,
  );
  // A click on a button must not move the keyboard off the page or the remote desktop.
  const keep = (e: Event) => e.preventDefault();
  const picture = id.status === "live" || id.status === "reconnecting";
  mount(
    root,
    h(
      "div",
      { class: "titlebar", "data-tauri-drag-region": true },
      capsule,
      h("span", { class: "spacer", "data-tauri-drag-region": true }),
      h(
        "button",
        { type: "button", class: "tb-btn", tabindex: -1, "aria-label": "Show Connections", title: "Show Connections (⌘0)", onmousedown: keep, onclick: () => on.showConnections() },
        icon("grid"),
      ),
      picture
        ? h(
            "button",
            {
              type: "button",
              class: "tb-btn",
              tabindex: -1,
              "aria-label": "Statistics",
              "aria-pressed": String(id.show_stats),
              title: id.show_stats ? "Hide Statistics" : "Show Statistics",
              onmousedown: keep,
              onclick: () => on.toggleStats(),
            },
            icon("gauge"),
          )
        : null,
    ),
  );
}
