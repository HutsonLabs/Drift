// What every page shares about IPC results: the tauri-specta result shape and the text shown
// for a command error.
import type { CommandError } from "./bindings";

/** A command's result, as the generated bindings return it. */
export type Result<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

/** Human text for a command error. */
export function describeError(e: CommandError): string {
  switch (e.kind) {
    case "invalid":
      return "Please fix the highlighted fields.";
    case "not-found":
      return "This connection no longer exists.";
    case "storage":
    case "platform":
      return e.message;
    case "no-session":
      return "This window is not connected.";
    case "not-implemented":
      return `${e.what} is not available yet.`;
  }
}
