// Connecting progress (shown until the first picture arrives): the current stage as a live
// status line, plus every stage of the connection as a checklist so a stall is obvious.
import type { ConnectMode, ConnectStage, SessionView_Serialize } from "../bindings";
import { h, mount } from "../dom";
import { icon } from "../icons";

const STAGES: Record<ConnectStage, string> = {
  tcp: "Contacting the host…",
  tls: "Securing the connection…",
  nla: "Signing in…",
  rdstls: "Opening the GNOME login screen…",
  activation: "Starting the remote desktop…",
};

/** The checklist steps a mode goes through, in order. */
export function stepsFor(mode: ConnectMode): ConnectStage[] {
  return mode === "remote-login" ? ["tcp", "tls", "nla", "rdstls", "activation"] : ["tcp", "tls", "nla", "activation"];
}

/**
 * Index of the current step in [`stepsFor`]. Remote Login reconnects to the login screen on legs
 * ≥ 2; everything before RDSTLS on those legs counts as "Opening the GNOME login screen", and
 * leg 1 never gets past "Signing in".
 */
export function currentStep(mode: ConnectMode, stage: ConnectStage, leg: number): number {
  if (mode !== "remote-login") return Math.max(0, stepsFor(mode).indexOf(stage));
  if (leg >= 2) return stage === "activation" ? 4 : 3;
  return stage === "activation" || stage === "rdstls" ? 2 : ["tcp", "tls", "nla"].indexOf(stage);
}

/** Renders the progress screen for a `connecting` state. */
export function renderConnecting(root: HTMLElement, view: SessionView_Serialize, on: { cancel(): void }): void {
  const state = view.state;
  const stage = state.state === "connecting" ? state.stage : "tcp";
  const leg = state.state === "connecting" ? state.leg : 1;
  const detail =
    view.mode === "remote-login" && leg >= 2 && stage !== "rdstls"
      ? `${STAGES[stage]} (opening the login screen)`
      : STAGES[stage];
  const now = currentStep(view.mode, stage, leg);
  // The checklist repeats the status line visually, so it is hidden from VoiceOver.
  const steps = h(
    "ol",
    { class: "stages", "aria-hidden": "true" },
    stepsFor(view.mode).map((s, i) =>
      h(
        "li",
        { class: i < now ? "stage done" : i === now ? "stage now" : "stage" },
        h("span", { class: "tick" }, i < now ? icon("check") : ""),
        i === now ? STAGES[s] : STAGES[s].replace("…", ""),
      ),
    ),
  );
  mount(
    root,
    h(
      "div",
      { class: "scrim" },
      h(
        "section",
        { class: "sheet connecting", "aria-labelledby": "connecting-title" },
        h("div", { class: "spinner", role: "progressbar", "aria-label": "Connecting", "aria-busy": "true" }),
        h("h1", { id: "connecting-title" }, `Connecting to ${view.profile_name}…`),
        h("p", { class: "status-line", role: "status", "aria-live": "polite" }, detail),
        steps,
        h("div", { class: "actions center" }, h("button", { type: "button", onclick: () => on.cancel() }, "Cancel")),
      ),
    ),
  );
}
