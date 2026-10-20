// M1-6 / M7-3 / M9-3 / M9-4 Red: certificate prompt, connecting, reconnect overlay, greeter hint
// and error screens render from state and emit intents.
import { describe, expect, test } from "bun:test";
import type { ErrorAction, ErrorExplanation } from "../src/bindings";
import { renderCertificatePrompt } from "../src/views/certificate";
import { currentStep, renderConnecting, stepsFor } from "../src/views/connecting";
import { renderError } from "../src/views/error";
import { renderGreeterHint } from "../src/views/greeter";
import { reconnectMessage, renderReconnectOverlay, secondsLeft } from "../src/views/reconnect";
import { renderStatsHud, statsLine } from "../src/views/stats";
import { button, certPrompt, check, field, GRDCTL_FINGERPRINT, hasButton, LNP_EXPLANATION, sessionView, stats, text } from "./helpers";

function root(): HTMLElement {
  const r = document.createElement("main");
  document.body.replaceChildren(r);
  return r;
}

describe("certificate prompt", () => {
  test("shows the fingerprint exactly as grdctl status prints it", () => {
    const r = root();
    renderCertificatePrompt(r, certPrompt(), "Homelab", { trust: () => {}, cancel: () => {} });
    const fp = r.querySelector(".fingerprint");
    expect(text(fp)).toBe(GRDCTL_FINGERPRINT);
    expect(text(r)).toContain("sudo grdctl --system status");
    expect(text(r)).toContain("TLS fingerprint");
    expect(text(r)).toContain("10.1.2.40:3389");
  });

  test("is an accessible alert dialog", () => {
    const r = root();
    renderCertificatePrompt(r, certPrompt(), "Homelab", { trust: () => {}, cancel: () => {} });
    const dialog = r.querySelector("[role=alertdialog]");
    expect(dialog).not.toBeNull();
    const title = r.querySelector(`[id="${dialog?.getAttribute("aria-labelledby")}"]`);
    expect(text(title)).toContain("10.1.2.40");
    expect(dialog?.getAttribute("aria-describedby")).toBeTruthy();
  });

  test("trust sends the remember choice; cancel rejects", () => {
    const r = root();
    const calls: string[] = [];
    renderCertificatePrompt(r, certPrompt(), "Homelab", {
      trust: (pin) => calls.push(`trust:${pin}`),
      cancel: () => calls.push("cancel"),
    });
    const remember = field(r, "Remember this certificate") as HTMLInputElement;
    expect(remember.checked).toBe(true);
    button(r, "Connect").click();
    check(remember, false);
    button(r, "Connect").click();
    button(r, "Cancel").click();
    expect(calls).toEqual(["trust:true", "trust:false", "cancel"]);
  });

  test("redirect targets explain the login hand-off", () => {
    const r = root();
    renderCertificatePrompt(r, certPrompt({ subject: "redirect-target" }), "Homelab", { trust: () => {}, cancel: () => {} });
    expect(text(r)).toContain("login screen");
  });
});

describe("connecting", () => {
  test("names the profile and the stage, with a cancel", () => {
    const r = root();
    let cancelled = false;
    renderConnecting(r, sessionView("connecting", { state: "connecting", leg: 1, stage: "nla" }), {
      cancel: () => {
        cancelled = true;
      },
    });
    expect(text(r)).toContain("Connecting to Homelab");
    expect(text(r)).toContain("Signing in");
    expect(r.querySelector("[role=progressbar]")).not.toBeNull();
    button(r, "Cancel").click();
    expect(cancelled).toBe(true);
  });

  test("Remote Login legs 2+ say the login screen is being opened", () => {
    const r = root();
    renderConnecting(r, sessionView("connecting", { state: "connecting", leg: 2, stage: "rdstls" }), { cancel: () => {} });
    expect(text(r)).toContain("login screen");
  });
});

describe("reconnect overlay", () => {
  test("counts down in whole seconds", () => {
    expect(secondsLeft(3000, 0)).toBe(3);
    expect(secondsLeft(3000, 1)).toBe(3);
    expect(secondsLeft(3000, 2001)).toBe(1);
    expect(secondsLeft(3000, 3000)).toBe(0);
    expect(secondsLeft(3000, 9000)).toBe(0);
    expect(reconnectMessage(3)).toBe("Reconnecting in 3 s…");
    expect(reconnectMessage(0)).toBe("Reconnecting…");
  });

  test("renders 'Reconnecting in N s… [Now] [Cancel]' as a live status", () => {
    const r = root();
    const calls: string[] = [];
    const view = sessionView("reconnecting", {
      state: "reconnecting",
      attempt: 2,
      next_in: 4200,
      reason: { kind: "network" },
    }, { mode: "headless" });
    renderReconnectOverlay(r, view, 0, { now: () => calls.push("now"), cancel: () => calls.push("cancel") });
    const status = r.querySelector("[role=status]");
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(text(status)).toContain("Reconnecting in 5 s…");
    expect(text(r)).toContain("Attempt 2 of 20");
    button(r, "Now").click();
    button(r, "Cancel").click();
    expect(calls).toEqual(["now", "cancel"]);
  });

  test("updates as time passes and handles an unlimited budget", () => {
    const r = root();
    const view = sessionView("reconnecting", {
      state: "reconnecting",
      attempt: 7,
      next_in: 4200,
      reason: { kind: "timeout" },
    }, { max_attempts: null });
    renderReconnectOverlay(r, view, 3500, { now: () => {}, cancel: () => {} });
    expect(text(r)).toContain("Reconnecting in 1 s…");
    expect(text(r)).toContain("Attempt 7");
    expect(text(r)).not.toContain(" of ");
  });
});

describe("greeter hint", () => {
  test("after a disconnect: the session is still running", () => {
    const r = root();
    renderGreeterHint(r, sessionView("greeter-hint", { state: "awaiting-greeter-login" }, { resuming: true }));
    expect(text(r)).toContain("Session is still running — log in to resume");
    expect(text(r)).toContain("drifttest");
    expect(r.querySelector("[role=status]")).not.toBeNull();
  });

  test("first login", () => {
    const r = root();
    renderGreeterHint(r, sessionView("greeter-hint", { state: "awaiting-greeter-login" }, { linux_username: null }));
    expect(text(r)).toContain("Log in to start your session");
    expect(text(r)).not.toContain("still running");
  });
});

describe("error screens", () => {
  function mountError(e: ErrorExplanation) {
    const r = root();
    const calls: ErrorAction[] = [];
    renderError(r, e, "Homelab", (a) => calls.push(a));
    return { r, calls };
  }

  test("Local Network Privacy screen links to System Settings", () => {
    const { r, calls } = mountError(LNP_EXPLANATION);
    expect(text(r.querySelector("h1"))).toBe("Drift can’t access your local network");
    expect(text(r)).toContain("System Settings › Privacy & Security › Local Network");
    button(r, "Open Local Network Settings").click();
    button(r, "Reconnect").click();
    expect(calls).toEqual(["open-local-network-settings", "reconnect"]);
    expect(r.querySelector("[role=alert]")).not.toBeNull();
  });

  test("next steps are an ordered list and the first action is the default", () => {
    const { r } = mountError({
      title: "Wrong user name or password",
      message: "The host rejected the RDP credentials in this profile.",
      next_steps: ["Desktop Sharing credentials must be set in GNOME Settings on the host.", "Update the user name and password in the profile."],
      actions: ["edit-profile", "reconnect"],
    });
    const steps = Array.from(r.querySelectorAll("ol li")).map(text);
    expect(steps[0]).toBe("Desktop Sharing credentials must be set in GNOME Settings on the host.");
    expect(steps.length).toBe(2);
    expect(button(r, "Edit Connection…").classList.contains("primary")).toBe(true);
    expect(button(r, "Reconnect").classList.contains("primary")).toBe(false);
  });

  test("close action is offered when present", () => {
    const { r, calls } = mountError({ title: "Disconnected", message: "You closed the connection.", next_steps: [], actions: ["reconnect", "close"] });
    expect(r.querySelector("ol")).toBeNull();
    expect(hasButton(r, "Open Local Network Settings")).toBe(false);
    button(r, "Close").click();
    expect(calls).toEqual(["close"]);
  });
});

describe("statistics HUD", () => {
  // Plan M1 "Done (manual M1)": `anim.py` shows >= 55 fps in the stats overlay.
  test("leads with the frame rate to one decimal", () => {
    expect(statsLine(stats())).toBe("58.9 fps · 1.2 Mbit/s · 4.3 ms · 2 unacked");
    expect(
      statsLine(stats({ fps_tenths: 600, mbit_tenths: 120, latency_p95_tenths_ms: 70, unacked_frames: 0 })),
    ).toBe("60.0 fps · 12.0 Mbit/s · 7.0 ms · 0 unacked");
  });

  test("is a polite live region so it never steals focus from the picture", () => {
    const r = root();
    renderStatsHud(r, stats());
    const hud = r.querySelector(".hud.stats");
    expect(hud).not.toBeNull();
    expect(hud?.getAttribute("role")).toBe("status");
    expect(hud?.getAttribute("aria-live")).toBe("polite");
    expect(text(hud)).toContain("58.9 fps");
    expect(r.querySelectorAll("button").length).toBe(0);
  });
});

describe("connecting checklist", () => {
  test("Remote Login maps both legs onto five steps", () => {
    expect(stepsFor("remote-login").length).toBe(5);
    expect(currentStep("remote-login", "tcp", 1)).toBe(0);
    expect(currentStep("remote-login", "nla", 1)).toBe(2);
    expect(currentStep("remote-login", "tls", 2)).toBe(3);
    expect(currentStep("remote-login", "rdstls", 2)).toBe(3);
    expect(currentStep("remote-login", "activation", 2)).toBe(4);
  });

  test("Headless and Desktop Sharing skip the login screen step", () => {
    expect(stepsFor("headless")).toEqual(["tcp", "tls", "nla", "activation"]);
    expect(currentStep("desktop-sharing", "activation", 1)).toBe(3);
  });

  test("steps before the current one are ticked; the list is hidden from VoiceOver", () => {
    const r = root();
    renderConnecting(r, sessionView("connecting", { state: "connecting", stage: "nla", leg: 1 }), { cancel: () => {} });
    const list = r.querySelector(".stages");
    expect(list?.getAttribute("aria-hidden")).toBe("true");
    expect(r.querySelectorAll(".stage.done").length).toBe(2);
    expect(text(r.querySelector(".stage.now"))).toBe("Signing in…");
  });
});
