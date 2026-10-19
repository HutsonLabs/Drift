// The two kinds of target (UI-kinds, docs/design/mockup-targets.html): a Mac running Drift Host
// and a GNOME computer. A profile paired with a Drift Host is a Mac (IOS-9); every other profile
// is a GNOME computer, whose mode (Headless / Desktop Sharing / Remote Login) only matters there.
import type { ConnectionProfile_Serialize, ConnectMode, HostKind, HostQuality } from "./bindings";

/** The profile's kind, as Rust's `ConnectionProfile::kind` decides it. */
export function kindOf(p: Pick<ConnectionProfile_Serialize, "paired_host">): HostKind {
  return p.paired_host ? "mac" : "gnome";
}

/** The kind's short name (badges, the title bar's VoiceOver label). */
export const KIND_NAMES: Record<HostKind, string> = { mac: "Mac", gnome: "GNOME" };

/** The kind as VoiceOver reads it on a card. */
export const KIND_DESCRIPTIONS: Record<HostKind, string> = { mac: "Mac running Drift Host", gnome: "GNOME computer" };

/** A GNOME mode as the title bar names it. */
export const SHORT_MODE_NAMES: Record<ConnectMode, string> = {
  headless: "Headless",
  "desktop-sharing": "Desktop Sharing",
  "remote-login": "Remote Login",
};

/** A Drift Host's quality step (EXT-4) as the title bar names it. */
export const QUALITY_NAMES: Record<HostQuality, string> = {
  best: "Sharp",
  high: "Balanced",
  medium: "Smooth",
  low: "Light",
};
