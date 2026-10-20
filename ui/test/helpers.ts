// Test helpers: accessible queries (by label / role / text) and fixture builders.
import type {
  CertificatePrompt,
  ConnectMode,
  ConnectionProfile_Serialize,
  ConnectionStatus,
  ErrorExplanation,
  OpenConnection,
  ProfileEntry_Serialize,
  SessionState_Serialize,
  SessionView_Serialize,
  StatsView,
  WindowIdentity,
} from "../src/bindings";

/** The form control labelled `text` (exact label text, ignoring a trailing hint). */
export function field(root: ParentNode, text: string): HTMLInputElement | HTMLSelectElement {
  const labels = Array.from(root.querySelectorAll("label"));
  const label = labels.find((l) => labelText(l) === text);
  if (!label) {
    throw new Error(`no label "${text}"; have: ${labels.map((l) => JSON.stringify(labelText(l))).join(", ")}`);
  }
  const id = label.htmlFor;
  const control = id ? root.querySelector(`[id="${id}"]`) : label.querySelector("input,select");
  if (!control) throw new Error(`label "${text}" has no control`);
  return control as HTMLInputElement | HTMLSelectElement;
}

/** Visible label text of a <label>, excluding nested controls. */
export function labelText(label: HTMLLabelElement): string {
  const clone = label.cloneNode(true) as HTMLLabelElement;
  for (const c of Array.from(clone.querySelectorAll("input,select,.hint"))) c.remove();
  return (clone.textContent ?? "").trim();
}

/** Every label text in `root`, in document order. */
export function labels(root: ParentNode): string[] {
  return Array.from(root.querySelectorAll("label")).map(labelText);
}

/** The button whose accessible name (aria-label or text) is `name`. */
export function button(root: ParentNode, name: string): HTMLButtonElement {
  const all = Array.from(root.querySelectorAll("button"));
  const b = all.find((x) => (x.getAttribute("aria-label") ?? x.textContent ?? "").trim() === name);
  if (!b) throw new Error(`no button "${name}"; have: ${all.map((x) => JSON.stringify(x.textContent?.trim())).join(", ")}`);
  return b;
}

/** Whether a button named `name` exists. */
export function hasButton(root: ParentNode, name: string): boolean {
  return Array.from(root.querySelectorAll("button")).some(
    (x) => (x.getAttribute("aria-label") ?? x.textContent ?? "").trim() === name,
  );
}

/** Normalised text content. */
export function text(node: Node | null | undefined): string {
  return (node?.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** Sets an input's value and fires `input` + `change`. */
export function type(el: HTMLInputElement | HTMLSelectElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

/** Dispatches a keydown on `el` (bubbles, cancelable). */
export function key(el: Element, k: string, mods: { meta?: boolean; shift?: boolean } = {}): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, metaKey: mods.meta ?? false, shiftKey: mods.shift ?? false });
  el.dispatchEvent(e);
  return e;
}

/** Toggles a checkbox / radio and fires `change`. */
export function check(el: HTMLInputElement, checked = true): void {
  el.checked = checked;
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

/** Waits for pending promise callbacks. */
export async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
}

let counter = 0;

/** A saved profile. */
export function profile(mode: ConnectMode, over: Partial<ConnectionProfile_Serialize> = {}): ConnectionProfile_Serialize {
  counter += 1;
  return {
    id: `00000000-0000-0000-0000-${String(counter).padStart(12, "0")}`,
    name: "Homelab",
    host: "10.1.2.40",
    port: 3389,
    mode,
    rdp_username: "rdp-user",
    linux_username: mode === "remote-login" ? "drifttest" : null,
    cert_pin: null,
    keyboard: { cmd_as: "super", type_with_mac_layout: false },
    display: { adaptive: true, retina: true },
    clipboard: "text-and-images",
    ...over,
  };
}

/** A profile list entry. */
export function entry(p: ConnectionProfile_Serialize, hasRdp = true, hasLinux = false): ProfileEntry_Serialize {
  return { profile: p, has_rdp_password: hasRdp, has_linux_password: hasLinux };
}

/** A session view snapshot. */
export function sessionView(
  screen: SessionView_Serialize["screen"],
  state: SessionState_Serialize,
  over: Partial<SessionView_Serialize> = {},
): SessionView_Serialize {
  return {
    profile_name: "Homelab",
    mode: "remote-login",
    linux_username: "drifttest",
    state,
    screen,
    certificate: null,
    explanation: null,
    resuming: false,
    max_attempts: 20,
    show_stats: false,
    stats: null,
    ...over,
  };
}

/** A statistics sample, as `StatsView` serialises it. */
export function stats(over: Partial<StatsView> = {}): StatsView {
  return { fps_tenths: 589, mbit_tenths: 12, latency_p95_tenths_ms: 43, unacked_frames: 2, ...over };
}

/** The homelab system daemon's real fingerprint, as `grdctl --system status` prints it. */
export const GRDCTL_FINGERPRINT =
  "f3:e7:a2:68:ee:a5:64:38:f8:14:03:12:50:4d:44:97:6c:dc:c3:ae:f2:4a:98:92:da:97:72:fe:b6:e6:fe:81";

/** A certificate prompt. */
export function certPrompt(over: Partial<CertificatePrompt> = {}): CertificatePrompt {
  return {
    host: "10.1.2.40",
    port: 3389,
    fingerprint: GRDCTL_FINGERPRINT,
    subject: "server",
    grdctl_command: "sudo grdctl --system status",
    ...over,
  };
}

/** The Local Network Privacy explanation, as `explain_disconnect` produces it. */
export const LNP_EXPLANATION: ErrorExplanation = {
  title: "Drift can’t access your local network",
  message: "macOS blocked Drift from connecting to computers on your local network.",
  next_steps: [
    "Open System Settings › Privacy & Security › Local Network and turn on Drift.",
    "Then reconnect.",
  ],
  actions: ["open-local-network-settings", "reconnect"],
};

/** A profile with a session window (the gallery's "Open" section). */
export function openConnection(
  profileId: string,
  status: ConnectionStatus,
  over: Partial<OpenConnection> = {},
): OpenConnection {
  return {
    profile_id: profileId,
    window: `session-${profileId.slice(-2)}`,
    status,
    live_secs: status === "live" ? 0 : null,
    reconnect_in_secs: status === "reconnecting" ? 3 : null,
    attempt: status === "reconnecting" ? 1 : null,
    ...over,
  };
}

/** A session window's title-bar identity. */
export function identity(over: Partial<WindowIdentity> = {}): WindowIdentity {
  return {
    profile_id: "00000000-0000-0000-0000-00000000abcd",
    name: "Homelab",
    host: "gnome.local",
    mode: "remote-login",
    status: "live",
    hint: null,
    show_stats: false,
    ...over,
  };
}
