// Session title-bar webview entry point (humble): wires the generated bindings to the
// controller. Everything testable lives in titlebarApp.ts and views/identity.ts.
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { commands, events } from "./bindings";
import { applyPlatform } from "./platform";
import { TitlebarApp } from "./titlebarApp";

applyPlatform();
const root = document.getElementById("titlebar");
if (root) {
  const app = new TitlebarApp(root, commands);
  // Identities are emitted to this webview only (`<label>-titlebar`).
  void events.windowIdentityChanged(getCurrentWebview()).listen((e) => app.onIdentity(e.payload));
  void app.start();
}
