// Session window page entry point (humble): wires the generated bindings to the controller.
// Everything testable lives in sessionApp.ts and views/.
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { commands, events } from "./bindings";
import { applyPlatform } from "./platform";
import { SessionApp } from "./sessionApp";

applyPlatform();
const root = document.getElementById("app");
if (root) {
  const app = new SessionApp(root, commands);
  // Session views are emitted to this window only (SessionManager → emit_to(window)).
  void events.sessionViewChanged(getCurrentWebviewWindow()).listen((e) => app.onSessionView(e.payload));
  void events.phoneControlsChanged(getCurrentWebviewWindow()).listen((e) => app.onPhoneControls(e.payload));
  void app.start();
}
