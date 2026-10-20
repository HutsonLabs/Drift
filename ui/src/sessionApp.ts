// Controller of a session window's page (UI-windows boards 4, 5, 7): renders the screen chosen
// by Rust (`SessionView.screen`: connecting stages, certificate sheet, greeter banner, statistics
// HUD, reconnect ring, error sheet) and turns clicks into IPC intents. A session window always
// has a session: Cancel while connecting and the error sheet's Close close the window, and Edit
// Connection… opens this profile's edit sheet on the Connections window.
import type { Result } from "./api";
import type { ErrorAction, PhoneControls, SessionView_Serialize, TouchMode, WindowIdentity, commands } from "./bindings";
import { mount } from "./dom";
import { renderCertificatePrompt } from "./views/certificate";
import { renderConnecting } from "./views/connecting";
import { renderError } from "./views/error";
import { renderGreeterHint } from "./views/greeter";
import { renderKeyRow } from "./views/keyRow";
import { renderReconnectOverlay } from "./views/reconnect";
import { renderStatsHud } from "./views/stats";

/** The IPC commands a session window's page uses. */
export type SessionApi = Pick<
  typeof commands,
  | "openLocalNetworkSettings"
  | "acceptCertificate"
  | "rejectCertificate"
  | "reconnectNow"
  | "cancelReconnect"
  | "closeSession"
  | "showConnections"
  | "windowIdentity"
  | "sendKeys"
  | "typeText"
  | "setTouchMode"
  | "phoneControls"
>;

/**
 * Which HUD floats over the live picture, mirroring `present::hud_for` in Rust: the greeter hint
 * banner, the statistics panel, or nothing.
 */
function hud(view: SessionView_Serialize): "" | "banner" | "stats" {
  if (view.screen === "greeter-hint") return "banner";
  if (view.screen === "live" && view.show_stats && view.stats) return "stats";
  return "";
}

/** The session page controller. */
export class SessionApp {
  private session: SessionView_Serialize | null = null;
  private sessionAt = 0;
  /** This window's profile (for Edit Connection…); it never changes for a window. */
  private profileId: string | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  /** iPhone: whether the key row shows, and the session's touch mode (IOS-8). */
  private controls: PhoneControls = { visible: false, touch: "auto" };

  constructor(
    private readonly root: HTMLElement,
    private readonly api: SessionApi,
    private readonly now: () => number = () => Date.now(),
  ) {}

  /** Learns which profile this window belongs to. */
  async start(): Promise<void> {
    const r = (await this.api.windowIdentity()) as Result<WindowIdentity>;
    if (r.status === "ok") this.profileId = r.data.profile_id;
    // iPhone: the controls may have been announced before this page listened (IOS-8).
    const controls = await this.api.phoneControls?.();
    if (controls) this.onPhoneControls(controls);
  }

  /** A new view snapshot for this window's session (the `sessionViewChanged` event). */
  onSessionView(view: SessionView_Serialize): void {
    this.session = view;
    this.sessionAt = this.now();
    this.render();
  }

  /** The iPhone controls changed (the `phoneControlsChanged` event; never sent on the Mac). */
  onPhoneControls(controls: PhoneControls): void {
    this.controls = controls;
    this.render();
  }

  /** Stops timers. */
  dispose(): void {
    this.stopTimer();
  }

  private act(action: ErrorAction): void {
    switch (action) {
      case "reconnect":
        void this.api.reconnectNow();
        break;
      case "close":
        void this.api.closeSession();
        break;
      case "open-local-network-settings":
        void this.api.openLocalNetworkSettings();
        break;
      case "edit-profile":
        void this.api.showConnections(this.profileId);
        break;
    }
  }

  private render(): void {
    const view = this.session;
    if (!view) return;
    document.body.dataset.screen = view.screen;
    // Which panel (if any) floats over the live picture; Rust shrinks the web view to match.
    document.body.dataset.hud = hud(view);
    const keys = this.controls.visible && view.screen === "live";
    document.body.dataset.controls = keys ? "keys" : "";
    if (view.screen === "reconnecting") this.startTimer();
    else this.stopTimer();

    switch (view.screen) {
      case "connecting":
        renderConnecting(this.root, view, { cancel: () => void this.api.closeSession() });
        break;
      case "certificate":
        if (view.certificate) {
          const fp = view.certificate.fingerprint;
          renderCertificatePrompt(this.root, view.certificate, view.profile_name, {
            trust: (pin) => void this.api.acceptCertificate(fp, pin),
            cancel: () => void this.api.rejectCertificate(),
          });
        }
        break;
      case "greeter-hint":
        renderGreeterHint(this.root, view);
        break;
      case "reconnecting":
        this.renderReconnect();
        break;
      case "error":
        if (view.explanation) renderError(this.root, view.explanation, view.profile_name, (a) => this.act(a));
        break;
      case "live":
        if (keys) this.renderKeys();
        else if (hud(view) === "stats" && view.stats) renderStatsHud(this.root, view.stats);
        else mount(this.root);
        break;
      default:
        mount(this.root);
    }
  }

  private renderKeys(): void {
    renderKeyRow(this.root, this.controls.touch, {
      sendKeys: (keys) => void this.api.sendKeys(keys),
      typeText: (text) => void this.api.typeText(text),
      setTouchMode: (mode) => void this.api.setTouchMode(mode as TouchMode),
    });
  }

  private renderReconnect(): void {
    const view = this.session;
    if (!view) return;
    renderReconnectOverlay(this.root, view, this.now() - this.sessionAt, {
      now: () => void this.api.reconnectNow(),
      cancel: () => void this.api.cancelReconnect(),
    });
  }

  private startTimer(): void {
    if (this.timer === null) this.timer = setInterval(() => this.renderReconnect(), 250);
  }

  private stopTimer(): void {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }
}
