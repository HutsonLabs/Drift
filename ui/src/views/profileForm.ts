// Profile fields (M1-6, M3-2), shown in the configuration sheet (views/sheet.ts): the rows of a
// FormModel, reading them back, the validation shown inline, and the password changes to save.
// The connection mode decides which credential fields exist:
// - Remote Login: the *system* RDP credentials, an optional Linux user (greeter hint) and an
//   explicit opt-in to store the Linux password so Drift types it at the GNOME login screen.
// - Headless / Desktop Sharing: one RDP credential set.
// Validation happens in Rust (`validate_profile` / `save_profile`); issues come back in the model.
import type {
  ClipboardPrefs,
  CmdAs,
  ConnectionProfile_Serialize,
  ConnectMode,
  ProfileField,
  ProfileIssue,
  SecretsUpdate,
  TouchMode,
} from "../bindings";
import { actionButton, h } from "../dom";
import { icon, modeGlyph } from "../icons";
import { MODE_ORDER } from "../modes";

/** Everything the form renders. */
export interface FormModel {
  /** The draft profile being edited. */
  profile: ConnectionProfile_Serialize;
  /** Not saved yet. */
  isNew: boolean;
  /** An RDP password is stored for this profile. */
  hasRdpPassword: boolean;
  /** A Linux greeter password is stored (the user opted in). */
  hasLinuxPassword: boolean;
  /** Opt-in checkbox state: store the Linux password and type it at the greeter. */
  typeLinuxPassword: boolean;
  /** Typed (unsaved) RDP password; empty = keep the stored one. */
  rdpPassword: string;
  /** Typed (unsaved) Linux password; empty = keep the stored one. */
  linuxPassword: string;
  /** Per-field problems from Rust. */
  issues: ProfileIssue[];
  /** A form-level error (storage, backend). */
  error: string | null;
  /** A save is in flight. */
  busy: boolean;
  /** The profile as saved (or as created): Save stays disabled while the draft equals it. */
  original: ConnectionProfile_Serialize;
  /** The profile has a session window (header Show Window, "Applies on next connect"). */
  open: boolean;
  /** Fields the user has edited: only these show validation issues as they type. */
  touched: ProfileField[];
}

/** The form's current values. */
export interface FormDraft {
  profile: ConnectionProfile_Serialize;
  rdpPassword: string;
  linuxPassword: string;
  typeLinuxPassword: boolean;
}

/** What the fields can ask for. */
export interface FieldIntents {
  /** A value that changes the form's shape changed (mode, opt-in): re-render with `draft`. */
  change(draft: FormDraft): void;
  forgetCertificate(): void;
}

/** A fresh model for `profile`. */
export function formModel(
  profile: ConnectionProfile_Serialize,
  opts: { isNew: boolean; hasRdpPassword: boolean; hasLinuxPassword: boolean; open?: boolean },
): FormModel {
  return {
    profile,
    isNew: opts.isNew,
    hasRdpPassword: opts.hasRdpPassword,
    hasLinuxPassword: opts.hasLinuxPassword,
    typeLinuxPassword: opts.hasLinuxPassword,
    rdpPassword: "",
    linuxPassword: "",
    issues: [],
    error: null,
    busy: false,
    original: profile,
    open: opts.open ?? false,
    touched: [],
  };
}

/** `profile` with every optional preference spelled out, as the form reads it back. */
function normalize(p: ConnectionProfile_Serialize): ConnectionProfile_Serialize {
  return {
    ...p,
    linux_username: p.mode === "remote-login" && p.linux_username ? p.linux_username : null,
    cert_pin: p.cert_pin ?? null,
    keyboard: { cmd_as: p.keyboard.cmd_as ?? "super", type_with_mac_layout: p.keyboard.type_with_mac_layout ?? false },
    display: { adaptive: p.display.adaptive ?? true, retina: p.display.retina ?? true },
    clipboard: p.clipboard ?? "text-and-images",
    touch: p.touch ?? "auto",
  };
}

/** Whether `draft` differs from what is saved (a new profile is always unsaved). */
export function isDirty(model: FormModel, draft: FormDraft): boolean {
  if (model.isNew) return true;
  return (
    JSON.stringify(normalize(draft.profile)) !== JSON.stringify(normalize(model.original)) ||
    draft.rdpPassword !== "" ||
    draft.linuxPassword !== "" ||
    draft.typeLinuxPassword !== model.hasLinuxPassword
  );
}

/** The one-line summary of the "Keyboard, display and clipboard" disclosure. */
export function advancedSummary(p: ConnectionProfile_Serialize): string {
  const q = normalize(p);
  const parts = [q.keyboard.cmd_as === "ctrl" ? "⌘ → Control" : "⌘ → Super"];
  if (q.keyboard.type_with_mac_layout) parts.push("Mac layout");
  if (p.mode === "desktop-sharing") parts.push("Scaled to fit");
  else parts.push(q.display.adaptive ? "Resize to fit" : "Fixed size");
  if (q.display.retina) parts.push("Retina");
  parts.push({ "text-and-images": "Text and images", text: "Text only", off: "Clipboard off" }[q.clipboard]);
  return parts.join(" · ");
}

/** Password changes to send with `save_profile`. */
export function toSecretsUpdate(draft: FormDraft, hasLinuxPassword: boolean): SecretsUpdate {
  const rdp_password = draft.rdpPassword === "" ? null : draft.rdpPassword;
  if (draft.profile.mode !== "remote-login") return { rdp_password, linux_password: { action: "forget" } };
  if (!draft.typeLinuxPassword) {
    return { rdp_password, linux_password: hasLinuxPassword ? { action: "forget" } : { action: "keep" } };
  }
  if (draft.linuxPassword !== "") return { rdp_password, linux_password: { action: "store", password: draft.linuxPassword } };
  return { rdp_password, linux_password: { action: "keep" } };
}

/** The mode tiles' labels and explanations. */
export const MODES: Record<ConnectMode, { label: string; about: string }> = {
  headless: {
    label: "Headless session",
    about: "A GNOME session that keeps running on the host without a screen. Reconnects go straight back into it.",
  },
  "desktop-sharing": {
    label: "Desktop Sharing",
    about: "Show and control a screen that someone is already logged in to.",
  },
  "remote-login": {
    label: "Remote Login",
    about: "Log in at the GNOME login screen, as if you were at the computer. After a reconnect you log in again and return to your running session.",
  },
};

const CREDENTIALS: Record<ConnectMode, { legend: string; user: string; password: string; hint: string }> = {
  "remote-login": {
    legend: "System RDP credentials",
    user: "System RDP user",
    password: "System RDP password",
    hint: "The Remote Login credentials set on the host with “sudo grdctl --system rdp set-credentials”. They open the GNOME login screen; you then log in with your own account.",
  },
  headless: {
    legend: "RDP credentials",
    user: "RDP user",
    password: "RDP password",
    hint: "The credentials set for that user on the host with “grdctl --headless rdp set-credentials”.",
  },
  "desktop-sharing": {
    legend: "Desktop Sharing credentials",
    user: "RDP user",
    password: "RDP password",
    hint: "Set on the host in GNOME Settings › System › Remote Desktop › Desktop Sharing.",
  },
};

/** The input of each validated field. */
const FIELD_IDS: Record<ProfileField, string> = {
  name: "profile-name",
  host: "host",
  port: "port",
  "rdp-username": "rdp-user",
  "linux-username": "linux-user",
};

const KEEP = "Saved — leave blank to keep";

/** The field (for validation) whose input has element id `id`. */
export function fieldOf(id: string): ProfileField | undefined {
  return (Object.keys(FIELD_IDS) as ProfileField[]).find((f) => FIELD_IDS[f] === id);
}

/**
 * The sheet body's rows for `model`: name, host and port, the mode tiles, the mode's credentials,
 * the GNOME login screen (Remote Login), the trusted certificate, and the "Keyboard, display and
 * clipboard" disclosure. `draft` reads the whole form back (for `change`).
 */
export function formFields(model: FormModel, on: FieldIntents, draft: () => FormDraft): Node[] {
  const p = model.profile;
  const cred = CREDENTIALS[p.mode];
  const isLogin = p.mode === "remote-login";
  const needsPassword = model.isNew || !model.hasRdpPassword;

  const modeGroup = h(
    "fieldset",
    // The mode explanation is part of the group's description, so VoiceOver reads it with the
    // radio buttons instead of leaving it as unattached text (M9-4).
    { class: "mode field", "aria-describedby": "mode-hint" },
    h("legend", {}, "Connection type"),
    h(
      "div",
      { class: "segmented" },
      MODE_ORDER.map((mode) =>
        h(
          "label",
          { class: "segment" },
          h("input", { type: "radio", name: "mode", value: mode, checked: mode === p.mode, onchange: () => on.change(draft()) }),
          modeGlyph(mode, "small"),
          MODES[mode].label,
        ),
      ),
    ),
    h("p", { class: "hint", id: "mode-hint" }, MODES[p.mode].about),
  );

  const credentials = h(
    "fieldset",
    { class: "credentials", "aria-describedby": "credentials-hint" },
    h("legend", {}, cred.legend),
    h("p", { class: "hint group-hint", id: "credentials-hint" }, cred.hint),
    group(
      textField("rdp-user", cred.user, p.rdp_username, { autocomplete: "username" }),
      textField("rdp-password", cred.password, model.rdpPassword, {
        type: "password",
        autocomplete: "current-password",
        required: needsPassword,
        placeholder: needsPassword ? "Required" : KEEP,
      }),
    ),
  );

  const greeter = isLogin
    ? h(
        "fieldset",
        { class: "greeter" },
        h("legend", {}, "GNOME login screen"),
        group(
          textField("linux-user", "Linux user (optional)", p.linux_username ?? "", {
            autocomplete: "off",
            hint: "Shown as a reminder while the login screen is open.",
          }),
          h(
            "div",
            { class: "field check-field" },
            h(
              "label",
              { class: "check" },
              h("input", {
                type: "checkbox",
                role: "switch",
                id: "type-linux-password",
                checked: model.typeLinuxPassword,
                "aria-describedby": "type-linux-password-hint",
                onchange: () => on.change(draft()),
              }),
              "Type my Linux password at the login screen",
            ),
            h(
              "p",
              { class: "hint", id: "type-linux-password-hint" },
              "Off by default. When on, the password is kept in your Keychain and Drift types it into the GNOME password field for you.",
            ),
          ),
          model.typeLinuxPassword &&
            textField("linux-password", "Linux password", model.linuxPassword, {
              type: "password",
              autocomplete: "off",
              required: !model.hasLinuxPassword,
              placeholder: model.hasLinuxPassword ? KEEP : "Required",
            }),
        ),
      )
    : null;

  const pin = p.cert_pin
    ? h(
        "section",
        { class: "security", "aria-labelledby": "security-title" },
        h("h3", { class: "group-title", id: "security-title" }, "Security"),
        group(
          h(
            "div",
            { class: "field pin" },
            h("span", { class: "pin-label", id: "pin-label" }, "Trusted certificate (SHA-256)"),
            h("code", { class: "fingerprint", "aria-labelledby": "pin-label" }, p.cert_pin),
            actionButton("Forget Certificate", () => on.forgetCertificate()),
          ),
        ),
      )
    : null;

  const sharing = p.mode === "desktop-sharing";
  const advanced = h(
    "details",
    { class: "advanced" },
    h(
      "summary",
      {},
      icon("chevron"),
      h("span", { class: "summary-title" }, "Keyboard, display and clipboard"),
      h("span", { class: "sum" }, advancedSummary(p)),
    ),
    group(
      selectField("cmd-as", "Command key sends", p.keyboard.cmd_as ?? "super", [
        ["super", "Super (Windows key)"],
        ["ctrl", "Control"],
      ]),
      checkField("mac-layout", "Type using Mac layout", p.keyboard.type_with_mac_layout ?? false,
        "Sends typed characters instead of key positions, so the Mac keyboard layout applies."),
      checkField("adaptive", "Resize the remote desktop to fit the window", sharing ? false : (p.display.adaptive ?? true),
        sharing ? "Desktop Sharing can’t resize the remote screen; Drift scales it to fit." : null, sharing),
      checkField("retina", "Use Retina resolution", p.display.retina ?? true,
        "Renders GNOME at 2× on Retina displays for sharp text."),
      selectField("clipboard", "Clipboard", p.clipboard ?? "text-and-images", [
        ["text-and-images", "Text and images"],
        ["text", "Text only"],
        ["off", "Off"],
      ]),
      // IOS-7: how a finger drives the remote pointer; the Mac ignores it.
      selectField("touch", "Touch on iPhone and iPad", p.touch ?? "auto", [
        ["auto", "Automatic (trackpad on iPhone, direct on iPad)"],
        ["trackpad", "Trackpad"],
        ["direct", "Tap where you touch"],
      ]),
    ),
  );

  return [
    model.error ? h("p", { class: "form-error", role: "alert" }, model.error) : "",
    group(
      textField("profile-name", "Name", p.name, { autocomplete: "off", placeholder: "e.g. Studio Workstation" }),
      h(
        "div",
        { class: "row" },
        textField("host", "Host", p.host, {
          autocomplete: "off",
          placeholder: "Name or IP address",
          spellcheck: "false",
          class: "grow",
        }),
        textField("port", "Port", String(p.port), { type: "number", min: "1", max: "65535", class: "port" }),
      ),
      modeGroup,
    ),
    credentials,
    greeter ?? "",
    pin ?? "",
    advanced,
  ].filter((n): n is HTMLElement => n !== "");
}

/**
 * Shows `issues` inline: each affected input gets `aria-invalid` and a described-by error line;
 * every other validated input is cleared. Updates in place, so typing keeps focus.
 */
export function showIssues(form: HTMLElement, issues: ProfileIssue[]): void {
  for (const [f, id] of Object.entries(FIELD_IDS) as [ProfileField, string][]) {
    const el = form.querySelector<HTMLInputElement>(`[id="${id}"]`);
    if (!el) continue;
    const issue = issues.find((i) => i.field === f);
    const errId = `${id}-error`;
    const describedBy = (el.getAttribute("aria-describedby") ?? "").split(" ").filter((x) => x && x !== errId);
    form.querySelector(`[id="${errId}"]`)?.remove();
    if (issue) {
      el.setAttribute("aria-invalid", "true");
      el.setAttribute("aria-describedby", [...describedBy, errId].join(" "));
      el.parentElement?.append(h("p", { class: "field-error", id: errId }, issue.message));
    } else {
      el.removeAttribute("aria-invalid");
      if (describedBy.length > 0) el.setAttribute("aria-describedby", describedBy.join(" "));
      else el.removeAttribute("aria-describedby");
    }
  }
}

/** A rounded glass group of form rows. */
function group(...rows: (Node | string | false | null)[]): HTMLElement {
  return h("div", { class: "group" }, rows);
}

interface FieldOpts {
  type?: string;
  autocomplete?: string;
  placeholder?: string;
  required?: boolean;
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
      required: o.required ?? false,
      min: o.min,
      max: o.max,
      spellcheck: o.spellcheck,
      "aria-describedby": o.hint ? `${id}-hint` : null,
    }),
    o.hint ? h("p", { class: "hint", id: `${id}-hint` }, o.hint) : "",
  );
}

function checkField(id: string, label: string, checked: boolean, hint: string | null, disabled = false): HTMLElement {
  return h(
    "div",
    { class: "field check-field" },
    h(
      "label",
      { class: "check" },
      h("input", { type: "checkbox", role: "switch", id, checked, disabled, "aria-describedby": hint ? `${id}-hint` : null }),
      label,
    ),
    hint ? h("p", { class: "hint", id: `${id}-hint` }, hint) : "",
  );
}

function selectField(id: string, label: string, value: string, options: [string, string][]): HTMLElement {
  const select = h("select", { id, name: id }, options.map(([v, text]) => h("option", { value: v }, text)));
  select.value = value;
  return h("div", { class: "field" }, h("label", { for: id }, label), select);
}

function input(form: HTMLFormElement, id: string): HTMLInputElement | null {
  return form.querySelector<HTMLInputElement>(`[id="${id}"]`);
}

/** Reads the form's current values over `base` (fields that are not rendered keep `base`). */
export function readDraft(form: HTMLFormElement, base: ConnectionProfile_Serialize): FormDraft {
  const val = (id: string, fallback: string) => input(form, id)?.value ?? fallback;
  const checked = (id: string, fallback: boolean) => {
    const el = input(form, id);
    return el ? el.checked : fallback;
  };
  const radios = Array.from(form.querySelectorAll<HTMLInputElement>("input[name=mode]"));
  const mode = (radios.find((r) => r.checked)?.value ?? base.mode) as ConnectMode;
  const port = Number.parseInt(val("port", String(base.port)), 10);
  const linuxUser = val("linux-user", base.linux_username ?? "").trim();
  const sharing = mode === "desktop-sharing";
  const adaptiveEl = input(form, "adaptive");
  const profile: ConnectionProfile_Serialize = {
    ...base,
    name: val("profile-name", base.name),
    host: val("host", base.host).trim(),
    port: Number.isFinite(port) && port >= 0 && port <= 65535 ? port : 0,
    mode,
    rdp_username: val("rdp-user", base.rdp_username),
    linux_username: mode === "remote-login" && linuxUser !== "" ? linuxUser : null,
    keyboard: {
      cmd_as: val("cmd-as", base.keyboard.cmd_as ?? "super") as CmdAs,
      type_with_mac_layout: checked("mac-layout", base.keyboard.type_with_mac_layout ?? false),
    },
    display: {
      // Desktop Sharing shows the box disabled; keep the stored preference untouched.
      adaptive: sharing || !adaptiveEl || adaptiveEl.disabled ? (base.display.adaptive ?? true) : adaptiveEl.checked,
      retina: checked("retina", base.display.retina ?? true),
    },
    clipboard: val("clipboard", base.clipboard ?? "text-and-images") as ClipboardPrefs,
    touch: val("touch", base.touch ?? "auto") as TouchMode,
  };
  return {
    profile,
    rdpPassword: val("rdp-password", ""),
    linuxPassword: val("linux-password", ""),
    typeLinuxPassword: mode === "remote-login" && checked("type-linux-password", false),
  };
}
