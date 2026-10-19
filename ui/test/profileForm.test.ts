// M1-6 / M3-2 / UI-windows Red: the profile fields render from state inside the configuration
// sheet; the mode decides the credential fields.
import { describe, expect, test } from "bun:test";
import type { ConnectMode } from "../src/bindings";
import { type FormDraft, type FormModel, formModel, toSecretsUpdate } from "../src/views/profileForm";
import { renderSheet } from "../src/views/sheet";
import { button, check, field, flush, GRDCTL_FINGERPRINT, hasButton, labels, profile, text, type } from "./helpers";

interface Harness {
  root: HTMLElement;
  model: FormModel;
  saved: { draft: FormDraft; connect: boolean }[];
  events: string[];
}

/** Mounts the sheet with intents that behave like the controller: `change` re-renders. */
function mount(model: FormModel): Harness {
  const root = document.createElement("main");
  document.body.replaceChildren(root);
  const h: Harness = { root, model, saved: [], events: [] };
  const render = () =>
    renderSheet(root, h.model, {
      change: (d) => {
        h.model = { ...h.model, profile: d.profile, rdpPassword: d.rdpPassword, linuxPassword: d.linuxPassword, typeLinuxPassword: d.typeLinuxPassword };
        render();
      },
      validate: () => Promise.resolve([]),
      save: (draft, connect) => h.saved.push({ draft, connect }),
      cancel: () => h.events.push("cancel"),
      remove: () => h.events.push("remove"),
      forgetCertificate: () => h.events.push("forget-certificate"),
      connect: () => h.events.push("connect"),
      showWindow: () => h.events.push("show-window"),
    });
  render();
  return h;
}

function newModel(mode: ConnectMode): FormModel {
  return formModel(profile(mode, { name: "", host: "", rdp_username: "", linux_username: null }), {
    isNew: true,
    hasRdpPassword: false,
    hasLinuxPassword: false,
  });
}

describe("mode-dependent fields", () => {
  test("Remote Login asks for the system RDP credentials and an optional Linux user", () => {
    const { root } = mount(newModel("remote-login"));
    const l = labels(root);
    for (const want of ["Name", "Host", "Port", "System RDP user", "System RDP password", "Linux user (optional)", "Type my Linux password at the login screen"]) {
      expect(l).toContain(want);
    }
    expect(l).not.toContain("Linux password");
    expect(l).not.toContain("RDP user");
    expect(text(root)).toContain("sudo grdctl --system rdp set-credentials");
  });

  test("the stored Linux password field appears only after the explicit opt-in", () => {
    const h = mount(newModel("remote-login"));
    const optIn = field(h.root, "Type my Linux password at the login screen") as HTMLInputElement;
    expect(optIn.type).toBe("checkbox");
    expect(optIn.checked).toBe(false);
    check(optIn);
    expect(labels(h.root)).toContain("Linux password");
    expect((field(h.root, "Linux password") as HTMLInputElement).type).toBe("password");
    expect(text(h.root)).toContain("Keychain");
    check(field(h.root, "Type my Linux password at the login screen") as HTMLInputElement, false);
    expect(labels(h.root)).not.toContain("Linux password");
  });

  test("Headless asks for one RDP credential set and nothing about Linux", () => {
    const { root } = mount(newModel("headless"));
    const l = labels(root);
    expect(l).toContain("RDP user");
    expect(l).toContain("RDP password");
    expect(l.some((x) => x.startsWith("Linux"))).toBe(false);
    expect(l).not.toContain("Type my Linux password at the login screen");
    expect(text(root)).toContain("grdctl --headless rdp set-credentials");
  });

  test("Desktop Sharing points at GNOME Settings and cannot resize", () => {
    const { root } = mount(newModel("desktop-sharing"));
    const l = labels(root);
    expect(l).toContain("RDP user");
    expect(l.some((x) => x.startsWith("Linux"))).toBe(false);
    expect(text(root)).toContain("GNOME Settings");
    const adaptive = field(root, "Resize the remote desktop to fit the window") as HTMLInputElement;
    expect(adaptive.disabled).toBe(true);
    expect(text(root)).toContain("scales it to fit");
  });

  test("switching the mode swaps the fields and keeps what was typed", () => {
    const h = mount(newModel("remote-login"));
    type(field(h.root, "Name"), "Lab");
    type(field(h.root, "Host"), "gnome.local");
    const headless = field(h.root, "Headless session") as HTMLInputElement;
    expect(headless.type).toBe("radio");
    check(headless);
    expect(h.model.profile.mode).toBe("headless");
    expect(labels(h.root)).toContain("RDP user");
    expect(labels(h.root)).not.toContain("Linux user (optional)");
    expect((field(h.root, "Name") as HTMLInputElement).value).toBe("Lab");
    expect((field(h.root, "Host") as HTMLInputElement).value).toBe("gnome.local");
  });

  test("the three modes are a labelled radio group, Headless first", () => {
    const { root } = mount(newModel("remote-login"));
    const group = root.querySelector("fieldset.mode");
    expect(text(group?.querySelector("legend"))).toBe("GNOME session");
    const radios = Array.from(group?.querySelectorAll("input[type=radio]") ?? []) as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(["headless", "desktop-sharing", "remote-login"]);
    expect(radios.filter((r) => r.checked).map((r) => r.value)).toEqual(["remote-login"]);
    const tiles = Array.from(group?.querySelectorAll("label") ?? []).map((l) => text(l));
    expect(tiles).toEqual(["Headless session", "Desktop Sharing", "Remote Login"]);
  });
});

describe("rendering from state", () => {
  test("values come from the profile", () => {
    const p = profile("remote-login", { name: "Office", host: "gnome.lan", port: 3390, rdp_username: "sys" });
    const { root } = mount(formModel(p, { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    expect((field(root, "Name") as HTMLInputElement).value).toBe("Office");
    expect((field(root, "Host") as HTMLInputElement).value).toBe("gnome.lan");
    expect((field(root, "Port") as HTMLInputElement).value).toBe("3390");
    expect((field(root, "System RDP user") as HTMLInputElement).value).toBe("sys");
    expect((field(root, "Linux user (optional)") as HTMLInputElement).value).toBe("drifttest");
  });

  test("a stored password is not required again; a new profile requires one", () => {
    const stored = mount(formModel(profile("headless"), { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    const pw = field(stored.root, "RDP password") as HTMLInputElement;
    expect(pw.required).toBe(false);
    expect(pw.placeholder).toContain("leave blank to keep");
    const fresh = mount(newModel("headless"));
    expect((field(fresh.root, "RDP password") as HTMLInputElement).required).toBe(true);
  });

  test("an opted-in profile shows the checkbox ticked with the password kept", () => {
    const p = profile("remote-login");
    const { root } = mount(formModel(p, { isNew: false, hasRdpPassword: true, hasLinuxPassword: true }));
    expect((field(root, "Type my Linux password at the login screen") as HTMLInputElement).checked).toBe(true);
    expect((field(root, "Linux password") as HTMLInputElement).placeholder).toContain("leave blank to keep");
  });

  test("validation issues mark fields invalid and describe them", async () => {
    const model = newModel("headless");
    model.issues = [
      { field: "host", problem: "port-in-host", message: "Put the port number in the Port field, not in the host." },
      { field: "rdp-username", problem: "empty", message: "RDP user name is required." },
    ];
    const { root } = mount(model);
    await flush();
    const host = field(root, "Host");
    expect(host.getAttribute("aria-invalid")).toBe("true");
    const described = (host.getAttribute("aria-describedby") ?? "").split(" ");
    const msg = described.map((id) => root.querySelector(`[id="${id}"]`)).find((e) => e?.classList.contains("field-error"));
    expect(text(msg)).toBe("Put the port number in the Port field, not in the host.");
    expect(field(root, "RDP user").getAttribute("aria-invalid")).toBe("true");
    expect(field(root, "Name").getAttribute("aria-invalid")).toBeNull();
  });

  test("a form-level error is announced", () => {
    const model = newModel("headless");
    model.error = "password storage failed: denied";
    const { root } = mount(model);
    expect(text(root.querySelector("[role=alert]"))).toContain("password storage failed");
  });

  test("a pinned certificate is shown in grdctl format and can be forgotten", () => {
    const p = profile("headless", { cert_pin: GRDCTL_FINGERPRINT });
    const h = mount(formModel(p, { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    expect(text(h.root.querySelector(".fingerprint"))).toBe(GRDCTL_FINGERPRINT);
    button(h.root, "Forget Certificate").click();
    expect(h.events).toEqual(["forget-certificate"]);
  });

  test("delete is offered only for saved profiles", () => {
    expect(hasButton(mount(newModel("headless")).root, "Delete…")).toBe(false);
    const h = mount(formModel(profile("headless"), { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    button(h.root, "Delete…").click();
    expect(h.events).toEqual(["remove"]);
  });

  test("every control has an accessible label", () => {
    for (const mode of ["remote-login", "headless", "desktop-sharing"] as const) {
      const m = formModel(profile(mode), { isNew: false, hasRdpPassword: true, hasLinuxPassword: true });
      const { root } = mount(m);
      for (const el of Array.from(root.querySelectorAll("input,select")) as HTMLElement[]) {
        const id = el.id;
        const labelled = (id && root.querySelector(`label[for="${id}"]`)) || el.closest("label") || el.getAttribute("aria-label");
        expect({ mode, id, labelled: Boolean(labelled) }).toEqual({ mode, id, labelled: true });
      }
    }
  });
});

describe("saving", () => {
  test("submit sends the edited profile and passwords, and connects a new one", async () => {
    const h = mount(newModel("remote-login"));
    type(field(h.root, "Name"), "Lab");
    type(field(h.root, "Host"), "gnome.local");
    type(field(h.root, "Port"), "3390");
    type(field(h.root, "System RDP user"), "sys");
    type(field(h.root, "System RDP password"), "pw-Fake1");
    type(field(h.root, "Linux user (optional)"), "");
    check(field(h.root, "Type using Mac layout") as HTMLInputElement);
    type(field(h.root, "Command key sends"), "ctrl");
    type(field(h.root, "Clipboard"), "text");
    type(field(h.root, "Touch on iPhone and iPad"), "direct");
    await flush();
    (h.root.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    expect(h.saved.length).toBe(1);
    expect(h.saved[0]!.connect).toBe(true);
    const d = h.saved[0]!.draft;
    expect(d.profile.name).toBe("Lab");
    expect(d.profile.host).toBe("gnome.local");
    expect(d.profile.port).toBe(3390);
    expect(d.profile.rdp_username).toBe("sys");
    expect(d.profile.linux_username).toBeNull();
    expect(d.profile.keyboard).toEqual({ cmd_as: "ctrl", type_with_mac_layout: true });
    expect(d.profile.clipboard).toBe("text");
    expect(d.profile.touch).toBe("direct");
    expect(d.rdpPassword).toBe("pw-Fake1");
  });

  test("toSecretsUpdate maps blanks to keep and the opt-in to store/forget", () => {
    const base = newModel("remote-login");
    const d = (over: Partial<FormDraft>): FormDraft => ({
      profile: base.profile,
      rdpPassword: "",
      linuxPassword: "",
      typeLinuxPassword: false,
      ...over,
    });
    expect(toSecretsUpdate(d({}), false)).toEqual({ rdp_password: null, linux_password: { action: "keep" } });
    expect(toSecretsUpdate(d({ rdpPassword: "x" }), false).rdp_password).toBe("x");
    expect(toSecretsUpdate(d({ typeLinuxPassword: true, linuxPassword: "l" }), false).linux_password).toEqual({
      action: "store",
      password: "l",
    });
    expect(toSecretsUpdate(d({ typeLinuxPassword: true }), true).linux_password).toEqual({ action: "keep" });
    expect(toSecretsUpdate(d({ typeLinuxPassword: false }), true).linux_password).toEqual({ action: "forget" });
    const headless = { ...base.profile, mode: "headless" as const };
    expect(toSecretsUpdate(d({ profile: headless, typeLinuxPassword: true, linuxPassword: "l" }), true).linux_password).toEqual({
      action: "forget",
    });
  });

  test("cancel is an intent", () => {
    const h = mount(formModel(profile("headless"), { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    button(h.root, "Cancel").click();
    expect(h.events).toEqual(["cancel"]);
  });
});

describe("header", () => {
  test("a new profile names its mode; a saved one shows host:port and trust status", () => {
    const fresh = mount(newModel("desktop-sharing"));
    expect(text(fresh.root.querySelector(".cfg-head .meta"))).toBe("Desktop Sharing");
    const p = profile("headless", { name: "Office", host: "gnome.lan", port: 3390, cert_pin: GRDCTL_FINGERPRINT });
    const saved = mount(formModel(p, { isNew: false, hasRdpPassword: true, hasLinuxPassword: false }));
    expect(text(saved.root.querySelector(".cfg-head h2"))).toBe("Office");
    expect(text(saved.root.querySelector(".cfg-head .meta"))).toBe("gnome.lan:3390 · Trusted");
    button(saved.root.querySelector(".cfg-head")!, "Connect").click();
    expect(saved.events).toEqual(["connect"]);
  });

  test("an open profile offers Show Window instead of Connect", () => {
    const h = mount(formModel(profile("headless"), { isNew: false, hasRdpPassword: true, hasLinuxPassword: false, open: true }));
    button(h.root.querySelector(".cfg-head")!, "Show Window").click();
    expect(h.events).toEqual(["show-window"]);
  });
});
