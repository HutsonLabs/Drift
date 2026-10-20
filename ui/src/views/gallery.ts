// The Connections gallery (UI-windows board 1, decision 16): an **Open** section (profiles with a
// session window, in window order) and a **Saved** section (the rest, then the dashed add card),
// filtered by All / Open and by a search on name and host. The cards
// are one keyboard grid: a roving tabindex, arrow keys move, Return does the primary action,
// Space opens the ⋯ menu, ⌘E / ⌘D / ⌘⌫ edit, duplicate and delete.
import type { OpenConnection, ProfileEntry_Serialize } from "../bindings";
import { h, mount } from "../dom";
import { icon } from "../icons";
import { type CardIntents, renderCard } from "./card";

/** The toolbar's filter: everything, or the connections with a window. */
export type Filter = "all" | "open";

/** Everything the gallery draws. */
export interface GalleryModel {
  entries: ProfileEntry_Serialize[];
  /** Profiles with a session window, in window opening order. */
  open: OpenConnection[];
  /** When `open` arrived (ms). */
  since: number;
  thumbnails: ReadonlyMap<string, string>;
  filter: Filter;
  query: string;
  now: number;
  /** The card that owns the grid's tab stop (a profile id, or `NEW_CARD`). */
  focusId: string | null;
}

/** What the gallery can ask for. */
export interface GalleryIntents extends CardIntents {
  create(): void;
  edit(id: string): void;
  duplicate(id: string): void;
  remove(id: string): void;
  /** The grid's tab stop moved to `id`. */
  focus(id: string): void;
}

/** The `data-profile-id` of the dashed New Connection card. */
export const NEW_CARD = "new";

/** The dashed card's and the welcome's words. */
const ADD_LABEL = "Add a GNOME computer";

/** Whether a connection matches the search text (name or host, case-insensitive). */
export function matches(entry: ProfileEntry_Serialize, query: string): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || [entry.profile.name, entry.profile.host].some((s) => s.toLowerCase().includes(q));
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function section(id: string, title: string, count: string, cards: HTMLElement[]): HTMLElement {
  return h(
    "section",
    { class: "gallery-section", "aria-labelledby": `${id}-title` },
    h("div", { class: "gallery-head" }, h("h2", { id: `${id}-title` }, title), h("span", { class: "section-count" }, count)),
    h("div", { class: "gallery" }, cards),
  );
}

/** Renders the gallery into `region`, keeping the keyboard on the card that had it. */
export function renderGallery(region: HTMLElement, model: GalleryModel, on: GalleryIntents): void {
  const active = document.activeElement;
  const hadFocus = active instanceof HTMLElement && region.contains(active) ? active.closest<HTMLElement>("[data-profile-id]") : null;
  const refocus = hadFocus?.dataset.profileId ?? null;

  const byId = new Map(model.entries.map((e) => [e.profile.id, e]));
  const openIds = new Set(model.open.map((o) => o.profile_id));
  const card = (entry: ProfileEntry_Serialize, open: OpenConnection | null) =>
    renderCard({ entry, open, since: model.since, thumbnail: model.thumbnails.get(entry.profile.id) ?? null }, model.now, on);

  const shown = (e: ProfileEntry_Serialize) => matches(e, model.query);
  const openCards = model.open.flatMap((o) => {
    const e = byId.get(o.profile_id);
    return e && shown(e) ? [card(e, o)] : [];
  });
  const savedCards =
    model.filter === "open" ? [] : model.entries.filter((e) => !openIds.has(e.profile.id) && shown(e)).map((e) => card(e, null));

  const content = h("div", { class: "gallery-content" });
  if (model.entries.length === 0) {
    content.append(
      h(
        "header",
        { class: "welcome" },
        h("h2", {}, ADD_LABEL),
        h("p", {}, "Drift connects to GNOME Remote Desktop 50 or later. Enter the host and the RDP credentials set on it."),
      ),
    );
  }
  if (openCards.length > 0) {
    content.append(section("open", "Open", plural(openCards.length, "window", "windows"), openCards));
  }
  const searching = model.query.trim() !== "";
  if (model.filter !== "open" && (savedCards.length > 0 || !searching)) {
    const add = h(
      "button",
      { type: "button", class: "card add", tabindex: -1, "data-profile-id": NEW_CARD, "aria-label": ADD_LABEL, onclick: () => on.create() },
      h("span", { class: "plus", "aria-hidden": "true" }, icon("plus")),
      h("span", { "aria-hidden": "true" }, ADD_LABEL),
      h("kbd", { "aria-hidden": "true" }, "⌘N"),
    );
    content.append(section("saved", "Saved", plural(savedCards.length, "connection", "connections"), [...savedCards, add]));
  }
  if (openCards.length === 0 && savedCards.length === 0 && (searching || model.filter === "open") && model.entries.length > 0) {
    content.append(
      h(
        "p",
        { class: "no-results", role: "status" },
        searching ? `No connections match “${model.query.trim()}”.` : "No connection has a window open.",
      ),
    );
  }

  const cells = Array.from(content.querySelectorAll<HTMLElement>(".gallery > .card"));
  const stop = cells.find((c) => c.dataset.profileId === (refocus ?? model.focusId)) ?? cells[0];
  stop?.setAttribute("tabindex", "0");

  const moveTo = (target: HTMLElement | undefined) => {
    if (!target) return;
    for (const c of cells) c.setAttribute("tabindex", c === target ? "0" : "-1");
    target.focus();
    on.focus(target.dataset.profileId ?? "");
  };
  content.addEventListener("focusin", (e) => {
    const c = e.target instanceof HTMLElement ? e.target.closest<HTMLElement>(".gallery > .card") : null;
    if (c && c.getAttribute("tabindex") !== "0") {
      for (const x of cells) x.setAttribute("tabindex", x === c ? "0" : "-1");
      on.focus(c.dataset.profileId ?? "");
    }
  });
  content.addEventListener("keydown", (e) => {
    const cell = e.target instanceof HTMLElement && cells.includes(e.target) ? e.target : null;
    if (!cell) return;
    const next = neighbour(cells, cell, e.key);
    if (next !== null) {
      e.preventDefault();
      moveTo(next);
      return;
    }
    const id = cell.dataset.profileId ?? "";
    if (id === NEW_CARD) return;
    const shortcut = e.metaKey && !e.shiftKey && !e.altKey && !e.ctrlKey;
    const act = (f: () => void) => {
      e.preventDefault();
      f();
    };
    if (e.key === "Enter" && !e.metaKey) act(() => on.primary(id));
    else if (e.key === " ") {
      act(() => {
        const r = cell.querySelector(".more")?.getBoundingClientRect();
        on.menu(id, { x: r?.left ?? 0, y: (r?.bottom ?? 0) + 4 });
      });
    } else if (shortcut && e.key.toLowerCase() === "e") act(() => on.edit(id));
    else if (shortcut && e.key.toLowerCase() === "d") act(() => on.duplicate(id));
    else if (shortcut && (e.key === "Backspace" || e.key === "Delete")) act(() => on.remove(id));
  });

  mount(region, content);
  if (refocus !== null) {
    const again = cells.find((c) => c.dataset.profileId === refocus);
    if (again) again.focus();
  }
}

/**
 * The cell an arrow / Home / End key moves to from `from`, `undefined` at an edge, or `null` if
 * the key does not move. Up and Down pick the nearest cell in the next row by position, so they
 * follow the grid across the two sections whatever the column count.
 */
function neighbour(cells: HTMLElement[], from: HTMLElement, key: string): HTMLElement | undefined | null {
  const i = cells.indexOf(from);
  switch (key) {
    case "ArrowLeft":
      return cells[i - 1];
    case "ArrowRight":
      return cells[i + 1];
    case "Home":
      return cells[0];
    case "End":
      return cells[cells.length - 1];
    case "ArrowUp":
    case "ArrowDown": {
      const down = key === "ArrowDown";
      const r = from.getBoundingClientRect();
      const rows = cells.map((c) => ({ c, r: c.getBoundingClientRect() })).filter((x) => (down ? x.r.top > r.top + 1 : x.r.top < r.top - 1));
      if (rows.length === 0) return undefined;
      const rowTop = down ? Math.min(...rows.map((x) => x.r.top)) : Math.max(...rows.map((x) => x.r.top));
      const centre = r.left + r.width / 2;
      const row = rows.filter((x) => Math.abs(x.r.top - rowTop) <= 1);
      row.sort((a, b) => Math.abs(a.r.left + a.r.width / 2 - centre) - Math.abs(b.r.left + b.r.width / 2 - centre));
      return row[0]?.c;
    }
    default:
      return null;
  }
}
