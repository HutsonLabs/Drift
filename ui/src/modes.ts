// Connection modes in the order every list, tile row and menu shows them (UI-windows decision
// 14): Headless → Desktop Sharing → Remote Login. New connections start Headless. Also the
// spoken names of a connection's status (cards and the identity capsule).
import type { ConnectionStatus, ConnectMode } from "./bindings";

/** Display order of the modes. */
export const MODE_ORDER: readonly ConnectMode[] = ["headless", "desktop-sharing", "remote-login"];

/** Mode of a new connection. Existing profiles keep theirs. */
export const DEFAULT_MODE: ConnectMode = "headless";

/** The mode's name on tiles, cards and in VoiceOver labels. */
export const MODE_NAMES: Record<ConnectMode, string> = {
  headless: "Headless session",
  "desktop-sharing": "Desktop Sharing",
  "remote-login": "Remote Login",
};

/** A mode as the title bar names it. */
export const SHORT_MODE_NAMES: Record<ConnectMode, string> = {
  headless: "Headless",
  "desktop-sharing": "Desktop Sharing",
  "remote-login": "Remote Login",
};

/** A connection status as VoiceOver reads it. */
export const STATUS_NAMES: Record<ConnectionStatus, string> = {
  idle: "Not connected",
  connecting: "Connecting",
  live: "Live",
  reconnecting: "Reconnecting",
  failed: "Failed",
};
