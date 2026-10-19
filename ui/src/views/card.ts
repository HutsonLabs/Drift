// One gallery card (UI-windows board 1, decision 16; UI-kinds C, D): a 16:9 preview (a Mac tile
// or the GNOME mode's gradient and glyph, or the session's live thumbnail) with the kind badge,
// the state pills, the one primary action shown on hover and focus, then the kind glyph, name,
// `Drift Host · host` or `host · mode`, and the ⋯ button.
import type { ConnectionStatus, OpenConnection, ProfileEntry_Serialize } from "../bindings";
import { h } from "../dom";
import { icon, KIND_ICONS, kindGlyph, MODE_ICONS } from "../icons";
import { KIND_DESCRIPTIONS, KIND_NAMES, kindOf } from "../kinds";
import { MODE_NAMES, STATUS_NAMES } from "../modes";

/** Everything a card draws. */
export interface CardModel {
  entry: ProfileEntry_Serialize;
  /** The profile's session window, if it has one. */
  open: OpenConnection | null;
  /** When `open` arrived (ms): uptime and countdown count from here. */
  since: number;
  /** The live preview (`data:` URL), kept in memory only. */
  thumbnail: string | null;
}

/** What a card can ask for. */
export interface CardIntents {
  /** Connect, or Show Window when the profile is open (button, double-click, Return). */
  primary(id: string): void;
  /** Open the ⋯ menu, at a pointer position or under the ⋯ button. */
  menu(id: string, at: { x: number; y: number }): void;
}

/** "2 h 14 m", "3 m", "< 1 m". */
export function formatUptime(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "< 1 m";
  if (minutes < 60) return `${minutes} m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} m`;
}

/** The Reconnecting pill's text with `ms` to wait. */
export function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return s > 0 ? `Reconnecting in ${s} s` : "Reconnecting…";
}

/** The label of a card's primary action. */
export function primaryLabel(open: boolean): string {
  return open ? "Show Window" : "Connect";
}

/** The VoiceOver label: "<name>, <mode>, <host>, <state>" ("Mac running Drift Host" for a Mac). */
export function cardLabel(entry: ProfileEntry_Serialize, status: ConnectionStatus): string {
  const p = entry.profile;
  const what = kindOf(p) === "mac" ? KIND_DESCRIPTIONS.mac : MODE_NAMES[p.mode];
  return `${p.name}, ${what}, ${p.host}, ${STATUS_NAMES[status]}`;
}

/** The line under the name: "Drift Host · <host>" for a Mac, "<host> · <mode>" for GNOME. */
export function cardDetail(p: ProfileEntry_Serialize["profile"]): string {
  return kindOf(p) === "mac" ? `Drift Host · ${p.host}` : `${p.host} · ${MODE_NAMES[p.mode]}`;
}

function pills(open: OpenConnection | null, since: number, now: number): (HTMLElement | null)[] {
  if (!open || open.status === "idle") return [];
  const pill = (cls: string, lead: Node | null, label: string, data: Record<string, string> = {}) =>
    h("span", { class: `pill ${cls}`, "aria-hidden": "true", ...data }, lead, h("span", { class: "pill-text" }, label));
  const dot = (status: ConnectionStatus) => h("span", { class: `status ${status}` });
  switch (open.status) {
    case "live": {
      const liveSince = since - (open.live_secs ?? 0) * 1000;
      return [
        pill("state", dot("live"), "Live"),
        pill("uptime", null, formatUptime(now - liveSince), { "data-since": String(liveSince) }),
      ];
    }
    case "connecting":
      return [pill("state", h("span", { class: "spin" }), "Connecting")];
    case "reconnecting": {
      const due = since + (open.reconnect_in_secs ?? 0) * 1000;
      return [pill("state", dot("reconnecting"), countdown(due - now), { "data-due": String(due) })];
    }
    case "failed":
      return [pill("state", dot("failed"), "Failed")];
  }
}

/** Re-reads the clock into every uptime and countdown pill under `root`. */
export function tickPills(root: ParentNode, now: number): void {
  for (const el of Array.from(root.querySelectorAll<HTMLElement>(".pill[data-since] .pill-text"))) {
    el.textContent = formatUptime(now - Number(el.parentElement?.dataset.since));
  }
  for (const el of Array.from(root.querySelectorAll<HTMLElement>(".pill[data-due] .pill-text"))) {
    el.textContent = countdown(Number(el.parentElement?.dataset.due) - now);
  }
}

/** The card element (`article.card`, `data-profile-id`); the gallery sets its tabindex. */
export function renderCard(model: CardModel, now: number, on: CardIntents): HTMLElement {
  const p = model.entry.profile;
  const status = model.open?.status ?? "idle";
  const isOpen = model.open !== null;
  const thumbnail = isOpen ? model.thumbnail : null;
  const label = primaryLabel(isOpen);

  const primary = h(
    "button",
    {
      type: "button",
      class: "primary",
      tabindex: -1,
      "aria-label": isOpen ? `Show ${p.name} Window` : `Connect to ${p.name}`,
      onclick: () => on.primary(p.id),
    },
    icon(isOpen ? "window" : "play"),
    label,
  );
  const more = h(
    "button",
    { type: "button", class: "icon-btn more", tabindex: -1, "aria-label": `Actions for ${p.name}`, "aria-haspopup": "menu" },
    icon("more"),
  );
  more.addEventListener("click", (e) => {
    e.stopPropagation();
    const r = more.getBoundingClientRect();
    on.menu(p.id, { x: r.left, y: r.bottom + 4 });
  });

  const kind = kindOf(p);
  // Idle art follows the kind: a muted Mac tile, or the GNOME mode's gradient and glyph.
  const thumb = h(
    "div",
    {
      class: ["thumb", kind === "mac" ? "thumb-mac" : `thumb-${p.mode}`, thumbnail ? "live" : null, status === "reconnecting" ? "dim" : null]
        .filter(Boolean)
        .join(" "),
    },
    thumbnail ? h("img", { class: "shot", src: thumbnail, alt: "", draggable: "false" }) : icon(kind === "mac" ? KIND_ICONS.mac : MODE_ICONS[p.mode]),
    pills(model.open, model.since, now),
    h("span", { class: `kind kind-${kind}`, "aria-hidden": "true" }, icon(KIND_ICONS[kind]), KIND_NAMES[kind]),
    h("div", { class: "over" }, primary),
  );

  const card = h(
    "article",
    { class: "card", tabindex: -1, "data-profile-id": p.id, "aria-label": cardLabel(model.entry, status) },
    thumb,
    h(
      "div",
      { class: "card-info" },
      kindGlyph(kind),
      h("span", { class: "item-text" }, h("span", { class: "item-name" }, p.name), h("span", { class: "item-detail" }, cardDetail(p))),
      more,
    ),
  );
  card.addEventListener("dblclick", (e) => {
    if (!(e.target instanceof Element && e.target.closest("button"))) on.primary(p.id);
  });
  card.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    card.focus();
    on.menu(p.id, { x: e.clientX, y: e.clientY });
  });
  return card;
}
