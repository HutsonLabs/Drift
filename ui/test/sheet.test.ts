// UI-windows Red (boards 2–3): the configuration sheet on the Connections window. Default
// Headless, mode order, credentials swap, inline validation, Add vs Add & Connect, Esc, the edit
// footer, the live note, the header and the "Keyboard, display and clipboard" disclosure.
import { describe, expect, test } from "bun:test";
import type { ConnectionProfile_Deserialize } from "../src/bindings";
import { newConnection, FakeConnectionsApi, startConnections } from "./fakes";
import { button, check, entry, field, flush, hasButton, key, labels, openConnection, profile, text, type } from "./helpers";

const STUDIO = profile("headless", { name: "Studio Workstation", host: "192.168.1.20" });
const KITCHEN = profile("desktop-sharing", { name: "Kitchen Laptop", host: "kitchen.lan", rdp_username: "kitchen", cert_pin: "aa:bb:cc" });

function fixture(): FakeConnectionsApi {
  const fake = new FakeConnectionsApi();
  fake.entries = [entry(KITCHEN), entry(STUDIO)];
  fake.open = [openConnection(STUDIO.id, "live", { live_secs: 60 })];
  return fake;
}

function sheet(root: ParentNode): HTMLElement {
  const s = root.querySelector<HTMLElement>("[role=dialog]");
  if (!s) throw new Error("no sheet");
  return s;
}

async function newSheet(fake = fixture()) {
  const started = await startConnections(fake);
  await newConnection(started.root);
  return started;
}

async function editSheet(name: string, fake = fixture()) {
  const started = await startConnections(fake);
  button(started.root, `Actions for ${name}`).click();
  const edit = Array.from(started.root.querySelectorAll<HTMLElement>("[role=menuitem]")).find((i) => text(i.querySelector(".label")) === "Edit…")!;
  edit.click();
  await flush();
  return started;
}

function footer(root: ParentNode): string[] {
  return Array.from(sheet(root).querySelectorAll(".cfg-foot button")).map(text);
}

async function fillValid(root: ParentNode) {
  type(field(root, "Name"), "Office Desktop");
  type(field(root, "Host"), "office.lan");
  type(field(root, "RDP user"), "hutson");
  type(field(root, "RDP password"), "pw-Fake1");
  await flush();
}

describe("new connection", () => {
  test("is a modal sheet over the gallery that defaults to Headless", async () => {
    const { root, fake } = await newSheet();
    const s = sheet(root);
    expect(s.getAttribute("aria-modal")).toBe("true");
    expect(text(s.querySelector(".cfg-head h2"))).toBe("New Connection");
    expect(text(s.querySelector(".cfg-head .meta"))).toBe("Headless session");
    expect(fake.calls).toContainEqual(["newProfile", "headless"]);
    const radios = Array.from(s.querySelectorAll<HTMLInputElement>("input[name=mode]"));
    expect(radios.map((r) => r.value)).toEqual(["headless", "desktop-sharing", "remote-login"]);
    expect(radios.filter((r) => r.checked).map((r) => r.value)).toEqual(["headless"]);
    // The gallery stays visible but out of reach while the sheet is up.
    expect(root.querySelector(".connections")?.hasAttribute("inert")).toBe(true);
    // Header and footer stay put; only the body scrolls.
    expect(s.querySelector(".cfg-body")).not.toBeNull();
    expect(footer(root)).toEqual(["Cancel", "Add", "Add & Connect"]);
  });

  test("switching the mode swaps only the credentials and keeps name, host and port", async () => {
    const { root, fake } = await newSheet();
    type(field(root, "Name"), "Lab");
    type(field(root, "Host"), "gnome.local");
    type(field(root, "Port"), "3390");
    check(field(root, "Remote Login") as HTMLInputElement);
    await flush();
    expect(labels(root)).toContain("System RDP user");
    expect(labels(root)).not.toContain("RDP user");
    expect((field(root, "Name") as HTMLInputElement).value).toBe("Lab");
    expect((field(root, "Host") as HTMLInputElement).value).toBe("gnome.local");
    expect((field(root, "Port") as HTMLInputElement).value).toBe("3390");
    expect(text(sheet(root).querySelector(".cfg-head .meta"))).toBe("Remote Login");
    expect(fake.names().filter((n) => n === "newProfile").length).toBe(1);
  });

  test("Add & Connect is the default button, disabled until validate_profile has no issues", async () => {
    const { root, fake } = await newSheet();
    const addConnect = button(sheet(root), "Add & Connect");
    expect(addConnect.type).toBe("submit");
    expect(addConnect.classList.contains("primary")).toBe(true);
    expect(fake.names()).toContain("validateProfile");
    expect(addConnect.disabled).toBe(true);
    expect(button(sheet(root), "Add").disabled).toBe(true);
    // A pristine field is not shouted at.
    expect(field(root, "Host").getAttribute("aria-invalid")).toBeNull();
    await fillValid(root);
    expect(button(sheet(root), "Add & Connect").disabled).toBe(false);
    expect(button(sheet(root), "Add").disabled).toBe(false);
  });

  test("errors show inline on a field once it was edited", async () => {
    const { root } = await newSheet();
    await fillValid(root);
    type(field(root, "Host"), "");
    await flush();
    const host = field(root, "Host");
    expect(host.getAttribute("aria-invalid")).toBe("true");
    const described = (host.getAttribute("aria-describedby") ?? "").split(" ").map((id) => text(root.querySelector(`[id="${id}"]`)));
    expect(described).toContain("Host is required.");
    expect(button(sheet(root), "Add & Connect").disabled).toBe(true);
    // Name was never cleared: no error there.
    expect(field(root, "Name").getAttribute("aria-invalid")).toBeNull();
    type(field(root, "Host"), "office.lan");
    await flush();
    expect(field(root, "Host").getAttribute("aria-invalid")).toBeNull();
    expect(root.querySelector(".field-error")).toBeNull();
  });

  test("Add saves the card; Add & Connect saves and connects", async () => {
    const { root, fake } = await newSheet();
    await fillValid(root);
    button(sheet(root), "Add").click();
    await flush();
    const save = fake.calls.find((c) => c[0] === "saveProfile")!;
    const saved = save[1] as ConnectionProfile_Deserialize;
    expect(saved.name).toBe("Office Desktop");
    expect(saved.mode).toBe("headless");
    expect(save[2]).toEqual({ rdp_password: "pw-Fake1", linux_password: { action: "forget" } });
    expect(root.querySelector("[role=dialog]")).toBeNull();
    expect(fake.names()).not.toContain("connect");
    expect(text(root)).toContain("Office Desktop");
    expect(root.querySelector(".connections")?.hasAttribute("inert")).toBe(false);

    await newConnection(root);
    await flush();
    await fillValid(root);
    (sheet(root).querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await flush();
    const second = fake.calls.filter((c) => c[0] === "saveProfile")[1]![1] as ConnectionProfile_Deserialize;
    expect(fake.calls).toContainEqual(["connect", second.id]);
    expect(root.querySelector("[role=dialog]")).toBeNull();
  });

  test("issues from save_profile are shown on their fields and the sheet stays open", async () => {
    const fake = fixture();
    fake.saveIssues = [{ field: "host", problem: "port-in-host", message: "Put the port number in the Port field, not in the host." }];
    const { root } = await newSheet(fake);
    await fillValid(root);
    button(sheet(root), "Add").click();
    await flush();
    expect(field(root, "Host").getAttribute("aria-invalid")).toBe("true");
    expect(text(sheet(root))).toContain("Put the port number in the Port field");
  });

  test("Esc cancels", async () => {
    const { root, fake } = await newSheet();
    type(field(root, "Name"), "Lab");
    key(field(root, "Name"), "Escape");
    expect(root.querySelector("[role=dialog]")).toBeNull();
    expect(fake.names()).not.toContain("saveProfile");
    await newConnection(root);
    await flush();
    button(sheet(root), "Cancel").click();
    expect(root.querySelector("[role=dialog]")).toBeNull();
  });
});

describe("editing a connection", () => {
  test("footer: Delete… on the left, then Cancel and Save, disabled until something changes", async () => {
    const { root } = await editSheet("Kitchen Laptop");
    expect(footer(root)).toEqual(["Delete…", "Cancel", "Save"]);
    expect(button(sheet(root), "Delete…").classList.contains("destructive")).toBe(true);
    expect(button(sheet(root), "Save").disabled).toBe(true);
    type(field(root, "Name"), "Kitchen");
    await flush();
    expect(button(sheet(root), "Save").disabled).toBe(false);
    type(field(root, "Name"), "Kitchen Laptop");
    await flush();
    expect(button(sheet(root), "Save").disabled).toBe(true);
  });

  test("Save stores the change and closes the sheet", async () => {
    const { root, fake } = await editSheet("Kitchen Laptop");
    type(field(root, "Name"), "Kitchen");
    await flush();
    button(sheet(root), "Save").click();
    await flush();
    const save = fake.calls.find((c) => c[0] === "saveProfile")!;
    expect((save[1] as ConnectionProfile_Deserialize).id).toBe(KITCHEN.id);
    expect((save[1] as ConnectionProfile_Deserialize).name).toBe("Kitchen");
    expect(save[2]).toEqual({ rdp_password: null, linux_password: { action: "forget" } });
    expect(root.querySelector("[role=dialog]")).toBeNull();
    expect(fake.names()).not.toContain("connect");
  });

  test("Delete… asks, then deletes and closes the sheet", async () => {
    const { root, fake } = await editSheet("Kitchen Laptop");
    button(sheet(root), "Delete…").click();
    const confirm = root.querySelector("[role=alertdialog]") as HTMLElement;
    expect(text(confirm.querySelector("h2"))).toBe("Delete “Kitchen Laptop”?");
    button(confirm, "Delete").click();
    await flush();
    expect(fake.calls).toContainEqual(["deleteProfile", KITCHEN.id]);
    expect(root.querySelector("[role=dialog]")).toBeNull();
  });

  test("header: host:port, trust status and Connect", async () => {
    const { root, fake } = await editSheet("Kitchen Laptop");
    const head = sheet(root).querySelector(".cfg-head") as HTMLElement;
    expect(text(head.querySelector("h2"))).toBe("Kitchen Laptop");
    expect(text(head.querySelector(".meta"))).toContain("kitchen.lan:3389");
    expect(text(head.querySelector(".meta .trusted"))).toBe("Trusted");
    expect(sheet(root).getAttribute("aria-labelledby")).toBeTruthy();
    button(head, "Connect").click();
    await flush();
    expect(fake.calls).toContainEqual(["connect", KITCHEN.id]);
    expect(root.querySelector("[role=dialog]")).toBeNull();
  });

  test("a live connection: Show Window in the header and “Applies on next connect” in the footer", async () => {
    const { root, fake } = await editSheet("Studio Workstation");
    const s = sheet(root);
    expect(text(s.querySelector(".cfg-head .meta"))).not.toContain("Trusted");
    expect(hasButton(s.querySelector(".cfg-head")!, "Connect")).toBe(false);
    expect(text(s.querySelector(".cfg-foot .note"))).toBe("Applies on next connect");
    button(s.querySelector(".cfg-head")!, "Show Window").click();
    await flush();
    expect(fake.calls).toContainEqual(["showWindow", STUDIO.id]);
  });

  test("an idle connection has no live note", async () => {
    const { root } = await editSheet("Kitchen Laptop");
    expect(text(sheet(root).querySelector(".cfg-foot"))).not.toContain("Applies on next connect");
  });
});

describe("Keyboard, display and clipboard", () => {
  test("a closed disclosure with a one-line summary that follows the values", async () => {
    const { root } = await newSheet();
    const details = sheet(root).querySelector("details.advanced") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    const summary = details.querySelector("summary") as HTMLElement;
    expect(text(summary)).toContain("Keyboard, display and clipboard");
    expect(text(summary.querySelector(".sum"))).toBe("⌘ → Super · Resize to fit · Retina · Text and images");
    type(field(root, "Clipboard"), "text");
    type(field(root, "Command key sends"), "ctrl");
    expect(text(summary.querySelector(".sum"))).toBe("⌘ → Control · Resize to fit · Retina · Text only");
    check(field(root, "Desktop Sharing") as HTMLInputElement);
    await flush();
    expect(text(sheet(root).querySelector("details.advanced .sum"))).toContain("Scaled to fit");
  });
});
