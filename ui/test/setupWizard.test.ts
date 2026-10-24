// WIZ-4 Red (docs/adr/GNOME-WIZARD-headless-setup.md): "Set Up a Linux Machine" on the
// Connections page. Machine → sign in (trust the host key, add Drift's key with the password
// once, or copy it) → options → check, then set up with live lines → the New Connection form
// prefilled with the pinned certificate and the RDP password. Nothing is saved until Add.
import { describe, expect, test } from "bun:test";
import type { SetupLine, SetupResult_Serialize } from "../src/bindings";
import { FakeConnectionsApi, startConnections } from "./fakes";
import { button, field, flush, hasButton, key, profile, text, type } from "./helpers";

const FP = "SHA256:Ho6XoGv0lXnxbUoU7lF5WBXz5mQe0k6xR6eK3JqvVj4";
const FP2 = "SHA256:9w3lYcO1q2s1jpJ0YH6VmA9KqvTz1Fhd2N4o7S8uXyw";
const TARGET = { host: "dev.lan", port: 22, user: "tester" };
const PIN = "48:a5:ff:8e:c8:3f:b3:06:d9:69:cc:22:9d:ea:86:76:88:54:57:11:7c:b3:dc:52:a5:22:b3:52:cf:e2:8e:2a";

const CHECK_LINES: SetupLine[] = [
  { kind: "mode", mode: "check" },
  { kind: "ok", text: "GNOME Shell and GDM are installed" },
  { kind: "would", text: "tester: enable and start pipewire.socket" },
  { kind: "command", text: "as_user tester systemctl --user enable --now pipewire.socket" },
  { kind: "output", text: "Reading package lists..." },
  { kind: "summary", changes: 1, errors: 0 },
];
const CHECKED: SetupResult_Serialize = { outcome: "changes-pending", errors: [], changes: 1, profiles: [] };
const APPLIED: SetupResult_Serialize = {
  outcome: "applied",
  errors: [],
  changes: 1,
  profiles: [profile("headless", { name: "dev", host: "dev.lan", port: 3390, rdp_username: "tester", cert_pin: PIN })],
};

function wizard(root: ParentNode): HTMLElement {
  const w = Array.from(root.querySelectorAll<HTMLElement>("[role=dialog]")).find((d) => text(d.querySelector("h2")) === "Set Up a Linux Machine");
  if (!w) throw new Error("no setup wizard");
  return w;
}

function isOpen(root: ParentNode): boolean {
  return Array.from(root.querySelectorAll("[role=dialog] h2")).some((h) => text(h) === "Set Up a Linux Machine");
}

function calls(fake: FakeConnectionsApi, name: string): unknown[][] {
  return fake.calls.filter((c) => c[0] === name);
}

async function open(fake = new FakeConnectionsApi(), copied: string[] = []) {
  const started = await startConnections(fake, undefined, copied);
  button(started.root.querySelector("header.toolbar")!, "Set Up a Linux Machine").click();
  await flush();
  return started;
}

/** Fills in the machine step and continues. */
async function machine(root: ParentNode, host = "dev.lan", account = "tester"): Promise<void> {
  const w = wizard(root);
  type(field(w, "Host"), host);
  type(field(w, "Account"), account);
  button(w, "Continue").click();
  await flush();
}

describe("Set Up a Linux Machine", () => {
  test("opens from the toolbar and asks for the machine first", async () => {
    const { root } = await open();
    const w = wizard(root);
    expect((field(w, "SSH Port") as HTMLInputElement).value).toBe("22");
    expect(hasButton(w, "Continue")).toBe(true);
    expect(hasButton(w, "Cancel")).toBe(true);
  });

  test("a new machine: trust its host key, add Drift's key with the password once, then options", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [
      { kind: "problem", diagnosis: { kind: "host-key-unknown", fingerprint: FP } },
      { kind: "problem", diagnosis: { kind: "key-not-authorized", offers_password: true } },
    ];
    const { root } = await open(fake);
    await machine(root);
    expect(calls(fake, "setupConnect")[0]).toEqual(["setupConnect", TARGET, "known-only"]);
    expect(text(wizard(root))).toContain(FP);

    button(wizard(root), "Trust and Continue").click();
    await flush();
    expect(calls(fake, "setupConnect")[1]).toEqual(["setupConnect", TARGET, "trust-new"]);

    const w = wizard(root);
    type(field(w, "Account Password"), "hunter22");
    button(w, "Add Drift's Key").click();
    await flush();
    expect(calls(fake, "setupInstallKey")).toEqual([["setupInstallKey", TARGET, "hunter22"]]);

    const options = wizard(root);
    expect((field(options, "Name") as HTMLInputElement).value).toBe("dev");
    expect((field(options, "Session Account") as HTMLInputElement).value).toBe("tester");
    expect((field(options, "RDP Port") as HTMLInputElement).value).toBe("3390");
    expect((field(options, "RDP Password") as HTMLInputElement).value).toBe("GeneratedPassword1234567");
    expect(hasButton(options, "Check")).toBe(true);
  });

  test("a wrong account password is named and asked for again", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [{ kind: "problem", diagnosis: { kind: "key-not-authorized", offers_password: true } }];
    fake.installAnswer = { kind: "problem", diagnosis: { kind: "password-rejected" } };
    const { root } = await open(fake);
    await machine(root);
    type(field(wizard(root), "Account Password"), "nope");
    button(wizard(root), "Add Drift's Key").click();
    await flush();
    expect(text(wizard(root).querySelector("[role=alert]"))).toBe("That password was not accepted.");
    expect((field(wizard(root), "Account Password") as HTMLInputElement).value).toBe("");
  });

  test("without password logins, Drift's public key can be copied", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [{ kind: "problem", diagnosis: { kind: "key-not-authorized", offers_password: false } }];
    const copied: string[] = [];
    const { root } = await open(fake, copied);
    await machine(root);
    const w = wizard(root);
    expect(() => field(w, "Account Password")).toThrow();
    button(w, "Copy Public Key").click();
    await flush();
    expect(copied).toEqual([fake.publicKey]);
    expect(text(w)).toContain("~/.ssh/authorized_keys");
    expect(hasButton(w, "Try Again")).toBe(true);
  });

  test("a changed host key is only forgotten on request", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [
      { kind: "problem", diagnosis: { kind: "host-key-changed", fingerprint: FP2 } },
      { kind: "problem", diagnosis: { kind: "host-key-unknown", fingerprint: FP2 } },
    ];
    const { root } = await open(fake);
    await machine(root);
    expect(text(wizard(root))).toContain("changed");
    expect(hasButton(wizard(root), "Trust and Continue")).toBe(false);
    button(wizard(root), "Forget Old Key").click();
    await flush();
    expect(calls(fake, "setupForgetHost")).toEqual([["setupForgetHost", "dev.lan", 22]]);
    expect(text(wizard(root))).toContain(FP2);
    expect(hasButton(wizard(root), "Trust and Continue")).toBe(true);
  });

  test("an unreachable machine stays on the first step with the reason", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [{ kind: "problem", diagnosis: { kind: "unreachable", detail: "No route to host (os error 65)" } }];
    const { root } = await open(fake);
    await machine(root);
    const w = wizard(root);
    expect(text(w.querySelector("[role=alert]"))).toContain("No route to host");
    expect((field(w, "Host") as HTMLInputElement).value).toBe("dev.lan");
  });

  test("check, then set up with live lines, then add the connection prefilled with its password", async () => {
    const fake = new FakeConnectionsApi();
    fake.runs = [
      { lines: CHECK_LINES, result: { status: "ok", data: CHECKED } },
      {
        lines: [
          { kind: "mode", mode: "apply" },
          { kind: "change", text: "tester: enable and start pipewire.socket" },
          { kind: "fingerprint", owner: "tester", fingerprint: PIN },
          { kind: "summary", changes: 1, errors: 0 },
        ],
        result: { status: "ok", data: APPLIED },
      },
    ];
    const { root } = await open(fake);
    await machine(root);
    button(wizard(root), "Check").click();
    await flush();
    const [, options, mode, sudo] = calls(fake, "setupRun")[0]!;
    expect([mode, sudo]).toEqual(["check", null]);
    expect((options as { rdp_password: string }).rdp_password).toBe("GeneratedPassword1234567");
    let w = wizard(root);
    expect(text(w)).toContain("Would enable and start pipewire.socket");
    expect(text(w)).toContain("1 change to make");
    // Package output is kept, but folded away.
    expect(w.querySelector("details")?.textContent).toContain("Reading package lists...");

    button(w, "Set Up").click();
    await flush();
    expect(calls(fake, "setupRun")[1]![2]).toBe("apply");
    w = wizard(root);
    expect(text(w)).toContain("dev is ready");
    button(w, "Add to Drift").click();
    await flush();

    expect(isOpen(root)).toBe(false);
    const form = root.querySelector<HTMLElement>("[role=dialog]")!;
    expect(text(form.querySelector("h2"))).toBe("New Connection");
    expect((field(form, "Host") as HTMLInputElement).value).toBe("dev.lan");
    expect((field(form, "Port") as HTMLInputElement).value).toBe("3390");
    expect(form.querySelector<HTMLInputElement>("#rdp-password")!.value).toBe("GeneratedPassword1234567");
    expect(calls(fake, "saveProfile")).toEqual([]);
  });

  test("a failed setup shows the script's errors and goes back to the options", async () => {
    const fake = new FakeConnectionsApi();
    fake.runs = [
      {
        lines: [{ kind: "error", text: "this gnome-remote-desktop has no headless mode; GNOME 46 or later is needed" }],
        result: { status: "ok", data: { outcome: "failed", errors: ["this gnome-remote-desktop has no headless mode; GNOME 46 or later is needed"], changes: null, profiles: [] } },
      },
    ];
    const { root } = await open(fake);
    await machine(root);
    button(wizard(root), "Check").click();
    await flush();
    const w = wizard(root);
    expect(text(w.querySelector("[role=alert]"))).toContain("GNOME 46 or later is needed");
    expect(hasButton(w, "Set Up")).toBe(false);
    button(w, "Back").click();
    await flush();
    expect(hasButton(wizard(root), "Check")).toBe(true);
  });

  test("sudo with a password asks for it, and a wrong one is named", async () => {
    const fake = new FakeConnectionsApi();
    fake.signIns = [{ kind: "ready", sudo: "password" }];
    fake.runs = [{ lines: [], result: { status: "error", error: { kind: "sudo-password-rejected" } } }];
    const { root } = await open(fake);
    await machine(root);
    let w = wizard(root);
    button(w, "Check").click();
    await flush();
    expect(calls(fake, "setupRun")).toEqual([]);
    expect(field(w, "Sudo Password").getAttribute("aria-invalid")).toBe("true");

    type(field(w, "Sudo Password"), "nope");
    button(w, "Check").click();
    await flush();
    expect(calls(fake, "setupRun")[0]![3]).toBe("nope");
    w = wizard(root);
    expect(text(w.querySelector("[role=alert]"))).toBe("sudo did not accept that password.");
    expect(hasButton(w, "Check")).toBe(true);
  });

  test("invalid options are shown on their field and nothing runs", async () => {
    const fake = new FakeConnectionsApi();
    const { root } = await open(fake);
    await machine(root);
    const w = wizard(root);
    type(field(w, "RDP Port"), "0");
    button(w, "Check").click();
    await flush();
    expect(field(w, "RDP Port").getAttribute("aria-invalid")).toBe("true");
    expect(calls(fake, "setupRun")).toEqual([]);
  });

  test("Only From appears with the firewall and is passed through", async () => {
    const fake = new FakeConnectionsApi();
    fake.runs = [{ lines: [], result: { status: "ok", data: CHECKED } }];
    const { root } = await open(fake);
    await machine(root);
    const w = wizard(root);
    type(field(w, "Only From"), "10.1.0.0/21");
    button(w, "Check").click();
    await flush();
    expect((calls(fake, "setupRun")[0]![1] as { allow_from: string | null }).allow_from).toBe("10.1.0.0/21");
  });

  test("Esc or Cancel closes the wizard and ends its SSH session", async () => {
    const fake = new FakeConnectionsApi();
    const { root } = await open(fake);
    key(wizard(root), "Escape");
    await flush();
    expect(isOpen(root)).toBe(false);
    expect(calls(fake, "setupDisconnect").length).toBe(1);
  });
});
