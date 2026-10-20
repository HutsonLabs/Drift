// Connections window entry point (humble): wires the generated tauri-specta bindings to the
// controller. Everything testable lives in connectionsApp.ts and views/.
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { commands, events } from "./bindings";
import { ConnectionsApp } from "./connectionsApp";

const root = document.getElementById("app");
if (root) {
  const app = new ConnectionsApp(root, commands);
  const here = getCurrentWebviewWindow();
  // All three are emitted to the `connections` window only.
  void events.connectionsChanged(here).listen((e) => app.onConnections(e.payload));
  void events.thumbnailUpdated(here).listen((e) => app.onThumbnail(e.payload));
  void events.connectionsIntentRequested(here).listen((e) => void app.onIntent(e.payload));
  void app.start();
}
