// "Pair a Mac" (IOS-9, ADR IOS-9-pairing): a sheet on the Connections window. While it is open
// the controller browses for Drift Hosts and pushes them in (`update`). The user picks a host
// and types the code its screen shows, or scans (or pastes) its QR code, which names the host by
// itself; then gives the account on the Mac. A Mac whose Drift Host approves devices (HOST-15)
// needs no code: the account comes from its advert and its user clicks Allow when Drift first
// connects. Rust recognises the code (`read_pairing_code`) and pairs (`pair_host`); this view
// only collects input and shows state.
import type { HostAdvert, PairHostRequest, PairingInput } from "../bindings";
import { h, mount } from "../dom";
import { icon } from "../icons";

/** What the pairing sheet can ask for. */
export interface PairIntents {
  /** Recognise the code field's text; resolves to the input or an error message. */
  read(text: string): Promise<PairingInput | string>;
  /** Scan a QR code; resolves to its text or an error message. */
  scan(): Promise<{ text: string } | { error: string }>;
  /** Pair; resolves to an error message, or `null` once paired (the controller closes the sheet). */
  pair(request: PairHostRequest): Promise<string | null>;
  cancel(): void;
}

/** A rendered pairing sheet. */
export interface PairSheetHandle {
  /** The hosts found so far. */
  update(hosts: HostAdvert[]): void;
  /** Puts `text` (a `drift-pair:` link) in the code field and reads it. */
  setCode(text: string): void;
}

/** Renders the pairing sheet into `root` and focuses its code field. */
export function renderPairSheet(root: HTMLElement, on: PairIntents): PairSheetHandle {
  let hosts: HostAdvert[] = [];
  let chosen: string | null = null;
  let input: PairingInput | null = null;
  let codeError: string | null = null;
  let error: string | null = null;
  let busy = false;
  let sequence = 0;
  // The user typed an account: the chosen Mac's advert no longer fills it in.
  let accountTyped = false;

  const list = h("div", { class: "pair-hosts", role: "group", "aria-label": "Macs on this network" });
  const code = h("input", { id: "pair-code", type: "text", autocomplete: "off", spellcheck: "false", inputmode: "text", placeholder: "1234 5678" });
  const account = h("input", { id: "pair-account", type: "text", autocomplete: "username", spellcheck: "false" });
  const password = h("input", { id: "pair-password", type: "password", autocomplete: "current-password" });
  const codeNote = h("p", { class: "field-note", "aria-live": "polite" });
  const status = h("p", { class: "pair-error", role: "alert" });
  const pairButton = h("button", { type: "submit", class: "primary" }, "Pair");

  const chosenHost = () => hosts.find((x) => x.host_id === chosen);
  // HOST-15: a chosen Mac that approves devices, with no code typed.
  const approves = () => input === null && codeError === null && code.value.trim() === "" && chosenHost()?.approval === true;
  const ready = () =>
    !busy &&
    account.value.trim() !== "" &&
    (input?.kind === "qr" || (input?.kind === "code" && chosenHost() !== undefined) || approves());

  const choose = (x: HostAdvert) => {
    chosen = x.host_id;
    if (!accountTyped && x.account) account.value = x.account;
  };

  const render = () => {
    mount(
      list,
      ...(hosts.length === 0
        ? [h("p", { class: "searching" }, "Looking for Macs running Drift Host…")]
        : hosts.map((x) =>
            h(
              "button",
              {
                type: "button",
                class: "pair-host",
                "aria-label": `${x.name} ${x.host}`,
                "aria-pressed": String(x.host_id === chosen),
                onclick: () => {
                  choose(x);
                  render();
                },
              },
              icon("display"),
              h("span", { class: "name" }, x.name),
              h("span", { class: "host" }, x.host),
            ),
          )),
    );
    if (codeError) codeNote.textContent = codeError;
    else if (input?.kind === "qr") codeNote.textContent = `Pairs with ${input.name} (${input.host}:${input.port}).`;
    else if (input?.kind === "code") codeNote.textContent = chosen ? "" : "Choose the Mac above.";
    else if (approves()) codeNote.textContent = `No code needed. Click Allow on ${chosenHost()?.name} when Drift Host asks.`;
    else codeNote.textContent = "Shown on the Mac by Drift Host. You can also paste its pairing link.";
    status.textContent = error ?? "";
    pairButton.disabled = !ready();
    pairButton.setAttribute("aria-busy", String(busy));
  };

  const read = async () => {
    const mine = ++sequence;
    const text = code.value.trim();
    if (text === "") {
      input = null;
      codeError = null;
      render();
      return;
    }
    const r = await on.read(text);
    if (mine !== sequence) return;
    if (typeof r === "string") {
      input = null;
      codeError = r;
    } else {
      input = r;
      codeError = null;
    }
    render();
  };
  code.addEventListener("input", () => void read());
  account.addEventListener("input", () => {
    accountTyped = true;
    render();
  });

  const scan = async () => {
    const r = await on.scan();
    if ("error" in r) {
      error = r.error;
      render();
      return;
    }
    error = null;
    code.value = r.text;
    await read();
  };

  const submit = async () => {
    if (!ready() || (!input && !approves())) return;
    const user = account.value.trim();
    const pw = password.value === "" ? null : password.value;
    const advert = chosenHost();
    const request: PairHostRequest =
      input === null
        ? { kind: "approve", advert: advert as HostAdvert, username: user, password: pw }
        : input.kind === "qr"
          ? { kind: "qr", uri: input.uri, username: user, password: pw }
          : { kind: "advert", advert: advert as HostAdvert, code: input.code, username: user, password: pw };
    busy = true;
    error = null;
    render();
    const failed = await on.pair(request);
    busy = false;
    error = failed;
    if (form.isConnected) render();
  };

  const label = (id: string, text: string, control: HTMLElement) => h("div", { class: "row" }, h("label", { for: id }, text), control);
  const form = h(
    "form",
    { class: "pair-form", novalidate: true },
    h("header", { class: "cfg-head" }, icon("display"), h("div", { class: "cfg-title" }, h("h2", {}, "Pair a Mac"), h("p", { class: "meta" }, "Drift Host"))),
    h(
      "div",
      { class: "cfg-body" },
      list,
      label("pair-code", "Code shown on the Mac", h("div", { class: "code-row" }, code, h("button", { type: "button", onclick: () => void scan() }, "Scan Code"))),
      codeNote,
      label("pair-account", "Account on the Mac", account),
      label("pair-password", "Password (optional)", password),
      status,
    ),
    h(
      "footer",
      { class: "cfg-foot" },
      h("span", { class: "note" }, "The Mac’s certificate is pinned when you pair"),
      h("span", { class: "spacer" }),
      h("button", { type: "button", onclick: () => on.cancel() }, "Cancel"),
      pairButton,
    ),
  );
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    void submit();
  });
  const dialog = h("section", { class: "cfg pair", role: "dialog", "aria-modal": "true", "aria-label": "Pair a Mac" }, form);
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      on.cancel();
    }
  });
  mount(root, h("div", { class: "scrim sheet-scrim" }, dialog));
  render();
  code.focus();
  return {
    update(next) {
      hosts = next;
      if (chosen && !hosts.some((x) => x.host_id === chosen)) chosen = null;
      const only = hosts.length === 1 ? hosts[0] : undefined;
      if (!chosen && only) choose(only);
      render();
    },
    setCode(text) {
      code.value = text;
      void read();
    },
  };
}
