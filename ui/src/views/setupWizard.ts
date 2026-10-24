// "Set Up a Linux Machine" (ADR GNOME-WIZARD-headless-setup): a sheet on the Connections window
// that walks through Machine → Sign in → Options → Set up → Done. It signs in over SSH with
// Drift's own key (the host key is trusted only on an explicit click; a changed one only after
// "Forget Old Key"), adds that key with the account password once, runs
// host/drift-host-setup.sh as --check and then --apply with its lines live, and ends in the
// New Connection form prefilled with the pinned certificate and the RDP password. Passwords are
// read from the fields when used and never kept here.
import type { Result } from "../api";
import { describeError } from "../api";
import type {
  commands,
  ConnectionProfile_Serialize,
  Diagnosis,
  RunMode,
  SetupError,
  SetupIssue,
  SetupLine,
  SetupOptions,
  SetupResult_Serialize,
  SignIn,
  Sudo,
  Target,
} from "../bindings";
import { h, mount } from "../dom";
import { icon } from "../icons";

/** The IPC commands the wizard uses. */
export type SetupApi = Pick<
  typeof commands,
  | "setupDefaults"
  | "setupValidate"
  | "setupPublicKey"
  | "setupForgetHost"
  | "setupConnect"
  | "setupInstallKey"
  | "setupRun"
  | "setupDisconnect"
>;

/** What the wizard hands back to the page. */
export interface SetupIntents {
  /** "Add to Drift": the unsaved connection and its RDP password. */
  finish(profile: ConnectionProfile_Serialize, rdpPassword: string): void;
  /** The wizard closed (Cancel, Esc, Done, or finish). */
  closed(): void;
  /** Puts text on the clipboard. */
  copy(text: string): Promise<void>;
}

type Step =
  | { kind: "machine"; error: string | null }
  | { kind: "trust"; fingerprint: string }
  | { kind: "changed"; fingerprint: string }
  | { kind: "key"; offersPassword: boolean; error: string | null; copied: boolean }
  | { kind: "options"; error: string | null }
  | { kind: "run"; mode: RunMode; result: SetupResult_Serialize | null; error: string | null }
  | { kind: "done"; profiles: ConnectionProfile_Serialize[] };

const TITLE = "Set Up a Linux Machine";

/** Each option issue's field and message. */
const ISSUES: Record<SetupIssue, [string, string]> = {
  name: ["setup-name", "Give the connection a name."],
  account: ["setup-session-account", "Not a Linux account name."],
  port: ["setup-rdp-port", "Enter a port from 1 to 65535."],
  "port-taken-by-remote-login": ["setup-rdp-port", "Port 3389 belongs to Remote Login; pick another."],
  "rdp-username": ["setup-rdp-user", "Enter an RDP user name."],
  "rdp-password": ["setup-rdp-password", "Use at least 8 characters, without line breaks."],
  "allow-from": ["setup-allow-from", "Enter a network such as 10.1.0.0/21."],
};
const SUDO_FIELD = "setup-sudo";

/** The wizard controller; one per open sheet. */
export class SetupWizard {
  private step: Step = { kind: "machine", error: null };
  private target: Target = { host: "", port: 22, user: "" };
  private options: SetupOptions | null = null;
  private sudo: Sudo = "passwordless";
  private lines: SetupLine[] = [];
  private busy = false;
  private closedAlready = false;

  constructor(
    private readonly root: HTMLElement,
    private readonly api: SetupApi,
    private readonly on: SetupIntents,
  ) {
    this.render();
  }

  /** A line of the running setup (the `setupLineEmitted` event). */
  line(line: SetupLine): void {
    if (this.step.kind !== "run" || this.step.result) return;
    this.lines.push(line);
    const list = this.root.querySelector(".setup-lines");
    const details = this.root.querySelector(".setup-output pre");
    const shown = describeLine(line, this.target.user);
    if (shown && list) list.append(shown);
    if (line.kind === "output" && details) details.textContent += `${line.text}\n`;
  }

  /** Closes the sheet and ends the SSH session. */
  close(): void {
    if (this.closedAlready) return;
    this.closedAlready = true;
    void this.api.setupDisconnect();
    this.root.replaceChildren();
    this.on.closed();
  }

  // ---- actions -----------------------------------------------------------------------------

  private async run<T>(work: () => Promise<T>): Promise<T | undefined> {
    if (this.busy) return undefined;
    this.busy = true;
    this.root.querySelector(".cfg")?.setAttribute("aria-busy", "true");
    try {
      return await work();
    } finally {
      this.busy = false;
    }
  }

  private async signIn(answer: Promise<Result<SignIn>>): Promise<void> {
    const r = await answer;
    if (this.closedAlready) return;
    if (r.status === "error") return this.go({ kind: "machine", error: describeError(r.error) });
    const s = r.data;
    if (s.kind === "ready") {
      this.sudo = s.sudo;
      this.options ??= await this.api.setupDefaults(this.target.host, this.target.user);
      return this.go({ kind: "options", error: null });
    }
    this.go(stepFor(s.diagnosis, this.target));
  }

  private connect(trust: "known-only" | "trust-new"): void {
    void this.run(() => this.signIn(this.api.setupConnect(this.target, trust) as Promise<Result<SignIn>>));
  }

  private continueFromMachine(): void {
    const host = this.value("setup-host").trim();
    const user = this.value("setup-account").trim();
    const port = Number(this.value("setup-port"));
    if (host === "" || user === "") return this.go({ kind: "machine", error: "Enter the machine and the account to sign in with." });
    if (!Number.isInteger(port) || port < 1 || port > 65535) return this.go({ kind: "machine", error: "Enter an SSH port from 1 to 65535." });
    if (host !== this.target.host || user !== this.target.user) this.options = null;
    this.target = { host, port, user };
    this.connect("known-only");
  }

  private installKey(): void {
    const password = this.value("setup-account-password");
    if (password === "") return this.alert("Enter the account's password.");
    void this.run(() => this.signIn(this.api.setupInstallKey(this.target, password) as Promise<Result<SignIn>>));
  }

  private forgetOldKey(): void {
    void this.run(async () => {
      const r = (await this.api.setupForgetHost(this.target.host, this.target.port)) as Result<null>;
      if (r.status === "error") return this.go({ kind: "machine", error: describeError(r.error) });
      await this.signIn(this.api.setupConnect(this.target, "known-only") as Promise<Result<SignIn>>);
    });
  }

  private copyKey(): void {
    void this.run(async () => {
      const r = (await this.api.setupPublicKey()) as Result<string>;
      if (r.status === "error") return this.alert(describeError(r.error));
      await this.on.copy(r.data);
      if (this.step.kind === "key") this.go({ ...this.step, copied: true });
    });
  }

  /** Check: validate in place (the fields keep focus), then run `--check`. */
  private check(): void {
    const options = this.readOptions();
    const sudoPassword = this.sudo === "password" ? this.value(SUDO_FIELD) : null;
    void this.run(async () => {
      const issues = await this.api.setupValidate(options);
      const sudoMissing = sudoPassword === "";
      showIssues(this.root, issues, sudoMissing);
      this.options = options;
      if (issues.length > 0 || sudoMissing) return;
      await this.setup("check", sudoPassword);
    });
  }

  private apply(): void {
    // The sudo password is asked again only if the check needed it: it is not kept.
    void this.run(() => this.setup("apply", this.sudo === "password" ? this.value(SUDO_FIELD) || null : null));
  }

  private async setup(mode: RunMode, sudoPassword: string | null): Promise<void> {
    const options = this.options;
    if (!options) return;
    this.lines = [];
    this.go({ kind: "run", mode, result: null, error: null }, sudoPassword);
    const r = (await this.api.setupRun(options, mode, sudoPassword)) as
      | { status: "ok"; data: SetupResult_Serialize }
      | { status: "error"; error: SetupError };
    if (this.closedAlready) return;
    if (r.status === "error") return this.setupFailed(r.error);
    const result = r.data;
    if (mode === "apply" && result.outcome === "applied") return this.go({ kind: "done", profiles: result.profiles });
    const error =
      result.outcome === "failed"
        ? result.errors.join("\n") || "The setup did not finish. The details are below."
        : null;
    this.go({ kind: "run", mode, result, error }, sudoPassword);
  }

  private setupFailed(e: SetupError): void {
    switch (e.kind) {
      case "not-signed-in":
        return this.go({ kind: "machine", error: "Sign in to the machine again." });
      case "invalid":
        this.go({ kind: "options", error: null });
        return showIssues(this.root, e.issues, false);
      case "sudo-password-needed":
        this.go({ kind: "options", error: null });
        return showIssues(this.root, [], true);
      case "sudo-password-rejected":
        return this.go({ kind: "options", error: "sudo did not accept that password." });
      case "ssh":
        return this.go({ kind: "run", mode: "check", result: null, error: diagnosisText(e.diagnosis, this.target) });
    }
  }

  // ---- rendering ---------------------------------------------------------------------------

  private go(step: Step, sudoPassword: string | null = null): void {
    this.step = step;
    this.render(sudoPassword);
  }

  private alert(message: string): void {
    const slot = this.root.querySelector(".setup-alert");
    if (slot) slot.replaceChildren(h("p", { class: "form-error", role: "alert" }, message));
  }

  private value(id: string): string {
    return this.root.querySelector<HTMLInputElement>(`[id="${id}"]`)?.value ?? "";
  }

  private checked(id: string): boolean {
    return this.root.querySelector<HTMLInputElement>(`[id="${id}"]`)?.checked ?? false;
  }

  private readOptions(): SetupOptions {
    const allow = this.value("setup-allow-from").trim();
    const port = Number(this.value("setup-rdp-port"));
    return {
      name: this.value("setup-name"),
      account: this.value("setup-session-account").trim(),
      port: Number.isInteger(port) && port > 0 && port < 65536 ? port : 0,
      rdp_username: this.value("setup-rdp-user"),
      rdp_password: this.value("setup-rdp-password"),
      remote_login: this.checked("setup-remote-login"),
      install_desktop: this.checked("setup-desktop"),
      open_firewall: this.checked("setup-firewall"),
      allow_from: allow === "" ? null : allow,
    };
  }

  private render(sudoPassword: string | null = null): void {
    const { body, foot, focus } = this.page(sudoPassword);
    const form = h(
      "form",
      { class: "profile-form setup-form", novalidate: true },
      h(
        "header",
        { class: "cfg-head" },
        icon("server"),
        h("div", { class: "cfg-title" }, h("h2", { id: "setup-title" }, TITLE), h("p", { class: "meta" }, stepLabel(this.step))),
      ),
      h("div", { class: "cfg-body" }, body),
      h("footer", { class: "cfg-foot" }, foot),
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      form.querySelector<HTMLButtonElement>("button.primary:not([disabled])")?.click();
    });
    const dialog = h("section", { class: "cfg setup", role: "dialog", "aria-modal": "true", "aria-labelledby": "setup-title" }, form);
    dialog.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close();
      }
    });
    mount(this.root, h("div", { class: "scrim sheet-scrim" }, dialog));
    (form.querySelector<HTMLElement>(focus ?? "input, button.primary") ?? dialog).focus();
  }

  private page(sudoPassword: string | null): { body: (Node | string)[]; foot: (Node | string)[]; focus?: string } {
    const s = this.step;
    const t = this.target;
    const cancel = h("button", { type: "button", onclick: () => this.close() }, "Cancel");
    const spacer = h("span", { class: "spacer" });
    const alertSlot = (message: string | null) =>
      h("div", { class: "setup-alert" }, message ? h("p", { class: "form-error", role: "alert" }, message) : "");
    const primary = (label: string, onclick: () => void) => h("button", { type: "submit", class: "primary", onclick: (e: Event) => (e.preventDefault(), onclick()) }, label);
    const back = (to: () => void) => h("button", { type: "button", onclick: to }, "Back");

    switch (s.kind) {
      case "machine":
        return {
          body: [
            h("p", { class: "lead" }, "Drift signs in over SSH, sets up a headless GNOME session that keeps running, and adds the connection. Nothing is sent anywhere else."),
            alertSlot(s.error),
            group(
              textField("setup-host", "Host", t.host, { placeholder: "Name or IP address", spellcheck: "false", autocomplete: "off" }),
              h(
                "div",
                { class: "row" },
                textField("setup-account", "Account", t.user, { spellcheck: "false", autocomplete: "username", class: "grow" }),
                textField("setup-port", "SSH Port", String(t.port), { type: "number", min: "1", max: "65535", class: "port" }),
              ),
            ),
          ],
          foot: [spacer, cancel, primary("Continue", () => this.continueFromMachine())],
          focus: t.host === "" ? "#setup-host" : undefined,
        };
      case "trust":
        return {
          body: [
            h("p", {}, `This is the first time Drift signs in to ${t.host}. Check that this is its SSH host key:`),
            h("p", {}, h("code", { class: "fingerprint" }, s.fingerprint)),
            h("p", { class: "hint" }, "On the machine: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub"),
          ],
          foot: [back(() => this.go({ kind: "machine", error: null })), spacer, cancel, primary("Trust and Continue", () => this.connect("trust-new"))],
        };
      case "changed":
        return {
          body: [
            h("p", { class: "warning", role: "alert" }, icon("warn"), `${t.host}'s SSH host key has changed since Drift last signed in. The machine may have been reinstalled — or someone may be intercepting the connection.`),
            h("p", {}, "The new key:"),
            h("p", {}, h("code", { class: "fingerprint" }, s.fingerprint)),
            h("p", { class: "hint" }, "Only forget the old key if you know why it changed."),
          ],
          foot: [back(() => this.go({ kind: "machine", error: null })), spacer, cancel, h("button", { type: "button", class: "destructive", onclick: () => this.forgetOldKey() }, "Forget Old Key")],
        };
      case "key":
        return {
          body: [
            h("p", {}, `${t.user} on ${t.host} does not accept Drift's key yet.`),
            alertSlot(s.error),
            s.offersPassword
              ? group(textField("setup-account-password", "Account Password", "", { type: "password", autocomplete: "current-password", hint: "Used once to add Drift's key, then forgotten." }))
              : "",
            h("p", {}, s.offersPassword ? "Or add Drift's key to ~/.ssh/authorized_keys on the machine yourself:" : "Add Drift's key to ~/.ssh/authorized_keys on the machine, then try again:"),
            h(
              "p",
              {},
              h("button", { type: "button", onclick: () => this.copyKey() }, "Copy Public Key"),
              s.copied ? h("span", { class: "note" }, " Copied") : "",
            ),
          ],
          foot: [
            back(() => this.go({ kind: "machine", error: null })),
            spacer,
            cancel,
            h("button", { type: "button", onclick: () => this.connect("known-only") }, "Try Again"),
            s.offersPassword ? primary("Add Drift's Key", () => this.installKey()) : "",
          ],
          focus: s.offersPassword ? "#setup-account-password" : undefined,
        };
      case "options":
        return { body: this.optionsBody(s.error, alertSlot), foot: [back(() => this.go({ kind: "machine", error: null })), spacer, cancel, primary("Check", () => this.check())], focus: "#setup-name" };
      case "run":
        return this.runPage(s, sudoPassword, { cancel, spacer, alertSlot, primary, back });
      case "done": {
        const first = s.profiles[0];
        const password = this.options?.rdp_password ?? "";
        return {
          body: [
            h("p", { class: "lead" }, icon("check"), `${first?.name ?? t.host} is ready.`),
            h("p", {}, "Its headless GNOME session keeps running after restarts. Add the connection to Drift; the RDP password and the certificate are filled in."),
            ...s.profiles.map((p) =>
              h(
                "div",
                { class: "setup-result" },
                h("span", { class: "grow" }, h("strong", {}, p.name), ` ${p.host}:${p.port}`),
                h("button", { type: "button", class: p === first ? "primary" : "", onclick: () => this.finish(p, password) }, p === first ? "Add to Drift" : "Add Remote Login to Drift"),
              ),
            ),
          ],
          foot: [spacer, h("button", { type: "button", onclick: () => this.close() }, "Done")],
        };
      }
    }
  }

  private optionsBody(error: string | null, alertSlot: (m: string | null) => HTMLElement): (Node | string)[] {
    const o = this.options;
    if (!o) return [];
    return [
      alertSlot(error),
      group(
        textField("setup-name", "Name", o.name, { autocomplete: "off" }),
        h(
          "div",
          { class: "row" },
          textField("setup-session-account", "Session Account", o.account, { spellcheck: "false", class: "grow", hint: "Gets the headless session. GNOME runs one session per account." }),
          textField("setup-rdp-port", "RDP Port", String(o.port), { type: "number", min: "1", max: "65535", class: "port" }),
        ),
      ),
      group(
        textField("setup-rdp-user", "RDP User Name", o.rdp_username, { spellcheck: "false", autocomplete: "off" }),
        textField("setup-rdp-password", "RDP Password", o.rdp_password ?? "", { spellcheck: "false", autocomplete: "off", hint: "Generated for this machine. Drift fills it in when you add the connection." }),
      ),
      group(
        checkField("setup-desktop", "Install GNOME if missing", o.install_desktop, "The smallest GNOME that runs a headless session, without NetworkManager."),
        checkField("setup-firewall", "Open the port in the firewall", o.open_firewall, null),
        textField("setup-allow-from", "Only From", o.allow_from ?? "", { spellcheck: "false", placeholder: "Any network, or e.g. 10.1.0.0/21" }),
        checkField("setup-remote-login", "Also set up Remote Login", o.remote_login, "GNOME's login screen on port 3389, for any account."),
      ),
      this.sudo === "password"
        ? group(textField(SUDO_FIELD, "Sudo Password", "", { type: "password", autocomplete: "current-password", hint: `${this.target.user}'s password, for sudo on the machine. Not kept.` }))
        : "",
    ];
  }

  private runPage(
    s: Extract<Step, { kind: "run" }>,
    sudoPassword: string | null,
    ui: {
      cancel: HTMLElement;
      spacer: HTMLElement;
      alertSlot: (m: string | null) => HTMLElement;
      primary: (label: string, onclick: () => void) => HTMLElement;
      back: (to: () => void) => HTMLElement;
    },
  ): { body: (Node | string)[]; foot: (Node | string)[] } {
    const host = this.target.host;
    const r = s.result;
    const running = r === null && s.error === null;
    const status = running
      ? s.mode === "check"
        ? `Checking ${host}…`
        : `Setting up ${host}… Installing GNOME can take a few minutes.`
      : r?.outcome === "changes-pending"
        ? `${r.changes ?? 0} change${r.changes === 1 ? "" : "s"} to make on ${host}.`
        : r?.outcome === "converged"
          ? `${host} is already set up; nothing to change.`
          : null;
    const output = this.lines.filter((l) => l.kind === "output").map((l) => (l.kind === "output" ? `${l.text}\n` : "")).join("");
    const shown = this.lines.map((l) => describeLine(l, this.target.user)).filter((n): n is HTMLElement => n !== null);
    const ready = r !== null && (r.outcome === "changes-pending" || r.outcome === "converged");
    // A sudo password the check needed rides along to Set Up in a hidden field, never stored.
    const sudo = sudoPassword !== null ? h("input", { type: "hidden", id: SUDO_FIELD, value: sudoPassword }) : "";
    return {
      body: [
        ui.alertSlot(s.error),
        status ? h("p", { class: "lead", "aria-live": "polite" }, running ? h("span", { class: "spinner", "aria-hidden": "true" }) : "", status) : "",
        h("ul", { class: "setup-lines", "aria-live": "polite" }, shown),
        h("details", { class: "setup-output" }, h("summary", {}, "Details"), h("pre", {}, output)),
        sudo,
      ],
      foot: [
        running ? "" : ui.back(() => this.go({ kind: "options", error: null })),
        ui.spacer,
        ui.cancel,
        ready ? ui.primary("Set Up", () => this.apply()) : "",
      ],
    };
  }

  private finish(profile: ConnectionProfile_Serialize, password: string): void {
    this.close();
    this.on.finish(profile, password);
  }
}

/** The step a sign-in problem leads to. */
function stepFor(d: Diagnosis, t: Target): Step {
  switch (d.kind) {
    case "host-key-unknown":
      return { kind: "trust", fingerprint: d.fingerprint };
    case "host-key-changed":
      return { kind: "changed", fingerprint: d.fingerprint };
    case "key-not-authorized":
      return { kind: "key", offersPassword: d.offers_password, error: null, copied: false };
    case "password-rejected":
      return { kind: "key", offersPassword: true, error: "That password was not accepted.", copied: false };
    case "password-not-offered":
      return { kind: "key", offersPassword: false, error: "This machine does not accept password logins.", copied: false };
    default:
      return { kind: "machine", error: diagnosisText(d, t) };
  }
}

/** Human text for a connection problem. */
function diagnosisText(d: Diagnosis, t: Target): string {
  switch (d.kind) {
    case "unreachable":
      return `Drift could not reach ${t.host}: ${d.detail}`;
    case "timed-out":
      return `${t.host} did not answer in time.`;
    case "connection-lost":
      return "The connection to the machine was lost. Try again.";
    case "host-key-unknown":
    case "host-key-changed":
      return `${t.host}'s SSH host key is not trusted.`;
    case "key-not-authorized":
      return `${t.user} on ${t.host} does not accept Drift's key.`;
    case "password-rejected":
      return "That password was not accepted.";
    case "password-not-offered":
      return "This machine does not accept password logins.";
    case "failed":
      return d.detail;
  }
}

function stepLabel(s: Step): string {
  switch (s.kind) {
    case "machine":
      return "Machine";
    case "trust":
    case "changed":
    case "key":
      return "Sign In";
    case "options":
      return "Options";
    case "run":
      return s.mode === "check" ? "Check" : "Set Up";
    case "done":
      return "Done";
  }
}

/** A setup line as a list item; `null` for lines that are not shown in the list. */
function describeLine(l: SetupLine, account: string): HTMLElement | null {
  const tidy = (text: string) => {
    const t = text.startsWith(`${account}: `) ? text.slice(account.length + 2) : text;
    return t.charAt(0).toUpperCase() + t.slice(1);
  };
  switch (l.kind) {
    case "ok":
      return h("li", { class: "ok" }, icon("check"), tidy(l.text));
    case "would":
      return h("li", { class: "would" }, `Would ${tidy(l.text).charAt(0).toLowerCase()}${tidy(l.text).slice(1)}`);
    case "change":
      return h("li", { class: "change" }, tidy(l.text));
    case "skip":
    case "wait":
      return h("li", { class: "note" }, tidy(l.text));
    case "error":
      return h("li", { class: "error" }, icon("warn"), tidy(l.text));
    default:
      return null;
  }
}

/** Shows option issues inline, in place (focus stays where it is). */
function showIssues(root: HTMLElement, issues: SetupIssue[], sudoMissing: boolean): void {
  const wanted = new Map<string, string>();
  for (const i of issues) {
    const [id, message] = ISSUES[i];
    if (!wanted.has(id)) wanted.set(id, message);
  }
  if (sudoMissing) wanted.set(SUDO_FIELD, "Enter the password sudo asks for.");
  const ids = new Set([...Object.values(ISSUES).map(([id]) => id), SUDO_FIELD]);
  for (const id of ids) {
    const el = root.querySelector<HTMLInputElement>(`[id="${id}"]`);
    if (!el || el.type === "hidden") continue;
    const errId = `${id}-error`;
    root.querySelector(`[id="${errId}"]`)?.remove();
    const message = wanted.get(id);
    const describedBy = (el.getAttribute("aria-describedby") ?? "").split(" ").filter((x) => x && x !== errId);
    if (message) {
      el.setAttribute("aria-invalid", "true");
      el.setAttribute("aria-describedby", [...describedBy, errId].join(" "));
      el.parentElement?.append(h("p", { class: "field-error", id: errId }, message));
    } else {
      el.removeAttribute("aria-invalid");
      if (describedBy.length > 0) el.setAttribute("aria-describedby", describedBy.join(" "));
      else el.removeAttribute("aria-describedby");
    }
  }
}

function group(...rows: (Node | string)[]): HTMLElement {
  return h("div", { class: "group" }, rows);
}

interface FieldOpts {
  type?: string;
  autocomplete?: string;
  placeholder?: string;
  hint?: string;
  min?: string;
  max?: string;
  spellcheck?: string;
  class?: string;
}

function textField(id: string, label: string, value: string, o: FieldOpts = {}): HTMLElement {
  return h(
    "div",
    { class: ["field", o.class].filter(Boolean).join(" ") },
    h("label", { for: id }, label),
    h("input", {
      id,
      name: id,
      type: o.type ?? "text",
      value,
      autocomplete: o.autocomplete,
      placeholder: o.placeholder,
      min: o.min,
      max: o.max,
      spellcheck: o.spellcheck,
      "aria-describedby": o.hint ? `${id}-hint` : null,
    }),
    o.hint ? h("p", { class: "hint", id: `${id}-hint` }, o.hint) : "",
  );
}

function checkField(id: string, label: string, checked: boolean, hint: string | null): HTMLElement {
  return h(
    "div",
    { class: "field check-field" },
    h("label", { class: "check" }, h("input", { type: "checkbox", role: "switch", id, checked, "aria-describedby": hint ? `${id}-hint` : null }), label),
    hint ? h("p", { class: "hint", id: `${id}-hint` }, hint) : "",
  );
}
