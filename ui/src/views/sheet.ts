// The configuration sheet (UI-windows boards 2–3, decision 15): a document-modal sheet on the
// Connections window. Fixed header (kind glyph, name, host:port or "Drift Host · host" and trust
// status, Connect or Show Window) and footer (Cancel / Add / Add & Connect, or Delete… (Unpair…
// for a Mac) / Cancel / Save); the profile
// rows scroll between them. Esc cancels. Validation is Rust's (`validate_profile`), run on every
// edit; issues show inline on fields the user has edited, and the default button stays disabled
// until there are none.
import type { ConnectionProfile_Serialize, ProfileField, ProfileIssue } from "../bindings";
import { h, mount } from "../dom";
import { icon, kindGlyph } from "../icons";
import { kindOf } from "../kinds";
import {
  advancedSummary,
  type FieldIntents,
  fieldOf,
  type FormDraft,
  type FormModel,
  formFields,
  isDirty,
  MODES,
  readDraft,
  showIssues,
} from "./profileForm";

/** What the sheet can ask for. */
export interface SheetIntents extends FieldIntents {
  /** Rust's validation of the current values (an empty list means valid). */
  validate(profile: ConnectionProfile_Serialize): Promise<ProfileIssue[]>;
  /** Add / Add & Connect / Save; `connect` = connect once saved. */
  save(draft: FormDraft, connect: boolean): void;
  cancel(): void;
  /** Delete… (the controller asks for confirmation). */
  remove(): void;
  /** The header's Connect (a saved profile without a window). */
  connect(draft: FormDraft): void;
  /** The header's Show Window (a profile with a window). */
  showWindow(): void;
}

/** A rendered sheet. */
export interface SheetHandle {
  /** The current values, with the fields edited so far. */
  draft(): FormDraft & { touched: ProfileField[] };
}

/** Renders the sheet for `model` into `root` (replacing its contents) and focuses it. */
export function renderSheet(root: HTMLElement, model: FormModel, on: SheetIntents): SheetHandle {
  const p = model.profile;
  const touched = new Set<ProfileField>(model.touched);
  // Issues from a failed save stay on their field until it is edited.
  let serverIssues = model.issues;
  let checked: ProfileIssue[] = [];
  let valid = !model.isNew && model.issues.length === 0;
  let sequence = 0;

  const form = h("form", { class: "profile-form", novalidate: true });
  const draft = () => readDraft(form, p);
  const handle: SheetHandle = { draft: () => ({ ...draft(), touched: [...touched] }) };
  const intents: FieldIntents = { change: () => on.change(handle.draft()), forgetCertificate: () => on.forgetCertificate() };

  const titleId = "cfg-title";
  const mac = kindOf(p) === "mac";
  const meta = model.isNew
    ? h("p", { class: "meta" }, MODES[p.mode].label)
    : h(
        "p",
        { class: "meta" },
        h("span", {}, mac ? `Drift Host · ${p.host}` : `${p.host}:${p.port}`),
        p.cert_pin ? [" ", h("span", { class: "sep", "aria-hidden": "true" }, "·"), " ", h("span", { class: "trusted" }, icon("shield"), "Trusted")] : "",
      );
  const headAction = model.isNew
    ? ""
    : model.open
      ? h("button", { type: "button", onclick: () => on.showWindow() }, icon("window"), "Show Window")
      : h("button", { type: "button", class: "primary", onclick: () => on.connect(draft()) }, icon("play"), "Connect");
  const head = h(
    "header",
    { class: "cfg-head" },
    kindGlyph(kindOf(p), "large"),
    h("div", { class: "cfg-title" }, h("h2", { id: titleId }, model.isNew ? "New Connection" : p.name || "Connection"), meta),
    h("span", { class: "spacer" }),
    headAction,
  );

  const busy = model.busy ? "true" : null;
  const add = h("button", { type: "button", onclick: () => submit(false) }, "Add");
  const primary = h("button", { type: "submit", class: "primary", "aria-busy": busy }, model.isNew ? "Add & Connect" : "Save");
  const foot = h(
    "footer",
    { class: "cfg-foot" },
    model.isNew
      ? h("span", { class: "note" }, "Passwords are kept in your Keychain")
      : h("button", { type: "button", class: "destructive", onclick: () => on.remove() }, mac ? "Unpair…" : "Delete…"),
    !model.isNew && model.open ? h("span", { class: "note live-note" }, h("span", { class: "status live", "aria-hidden": "true" }), "Applies on next connect") : "",
    h("span", { class: "spacer" }),
    h("button", { type: "button", onclick: () => on.cancel() }, "Cancel"),
    model.isNew ? add : "",
    primary,
  );

  const body = h("div", { class: "cfg-body" }, formFields(model, intents, draft));
  form.append(head, body, foot);

  const refreshButtons = () => {
    const d = draft();
    const ready = valid && !model.busy;
    add.disabled = !ready;
    primary.disabled = !ready || !isDirty(model, d);
  };
  const shown = () => {
    const edited = checked.filter((i) => touched.has(i.field));
    return [...edited, ...serverIssues.filter((i) => !edited.some((e) => e.field === i.field))];
  };
  const validate = () => {
    const d = draft();
    const summary = form.querySelector(".advanced .sum");
    if (summary) summary.textContent = advancedSummary(d.profile);
    const mine = ++sequence;
    refreshButtons();
    void on.validate(d.profile).then((issues) => {
      if (mine !== sequence || !form.isConnected) return;
      checked = issues;
      valid = issues.length === 0 && serverIssues.length === 0;
      showIssues(form, shown());
      refreshButtons();
    });
  };
  form.addEventListener("input", (e) => {
    const field = e.target instanceof HTMLElement ? fieldOf(e.target.id) : undefined;
    if (field) {
      touched.add(field);
      serverIssues = serverIssues.filter((i) => i.field !== field);
    }
    validate();
  });
  form.addEventListener("change", () => validate());
  function submit(connect: boolean) {
    if (model.busy) return;
    if (!valid) {
      // Return on an invalid form: show every problem, not only those of edited fields.
      for (const i of checked) touched.add(i.field);
      showIssues(form, shown());
      return;
    }
    const d = draft();
    if (!isDirty(model, d)) return;
    on.save({ ...d }, connect);
  }
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    submit(model.isNew);
  });

  const dialog = h(
    "section",
    { class: "cfg", role: "dialog", "aria-modal": "true", "aria-labelledby": titleId },
    form,
  );
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      on.cancel();
    }
  });
  // A re-render (mode switch, opt-in) keeps the keyboard on the same control.
  const active = document.activeElement instanceof HTMLInputElement && root.contains(document.activeElement) ? document.activeElement : null;
  const again = active?.id ? `[id="${active.id}"]` : active?.name ? `input[name="${active.name}"][value="${active.value}"]` : null;
  mount(root, h("div", { class: "scrim sheet-scrim" }, dialog));
  showIssues(form, shown());
  validate();
  (form.querySelector<HTMLElement>(again ?? "#profile-name") ?? dialog).focus();
  return handle;
}
