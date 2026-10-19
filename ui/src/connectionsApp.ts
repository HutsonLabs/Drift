// Controller of the Connections window (UI-windows boards 1–3): the toolbar drawn in the
// transparent title bar, the gallery, the configuration sheet, the delete confirmation and
// command errors. It loads profiles and open connections, follows Rust's pushes
// (`connectionsChanged`, `thumbnailUpdated`, `connectionsIntentRequested`) and turns clicks into
// commands. The API is injected (the generated `commands` in production, a fake in tests).
import { describeError, type Result } from "./api";
import type {
  ConnectionProfile_Serialize,
  Connections,
  ConnectionsIntent,
  HostAdvert,
  PairingInput,
  OpenConnection,
  ProfileEntry_Serialize,
  Thumbnail,
  commands,
} from "./bindings";
import { h, mount } from "./dom";
import { icon } from "./icons";
import { DEFAULT_MODE } from "./modes";
import { kindOf } from "./kinds";
import { renderAddSheet } from "./views/addSheet";
import { primaryLabel, tickPills } from "./views/card";
import { renderConfirm } from "./views/confirm";
import { type Filter, renderGallery } from "./views/gallery";
import { openMenu } from "./views/menu";
import { type FormDraft, type FormModel, formModel, isDirty, toSecretsUpdate } from "./views/profileForm";
import { type PairSheetHandle, renderPairSheet } from "./views/pairSheet";
import { renderSheet, type SheetHandle } from "./views/sheet";

/** The IPC commands the Connections page uses. */
export type ConnectionsApi = Pick<
  typeof commands,
  | "listProfiles"
  | "newProfile"
  | "validateProfile"
  | "saveProfile"
  | "deleteProfile"
  | "duplicateProfile"
  | "forgetCertificate"
  | "connect"
  | "showWindow"
  | "disconnectProfile"
  | "connections"
  | "startHostDiscovery"
  | "stopHostDiscovery"
  | "readPairingCode"
  | "pairHost"
  | "scanPairingQr"
  | "takePairingLink"
>;

/** The Connections page controller. */
export class ConnectionsApp {
  private entries: ProfileEntry_Serialize[] = [];
  private open: OpenConnection[] = [];
  private openSince = 0;
  /** Live previews by profile id. Memory only: never stored, never logged. */
  private readonly thumbnails = new Map<string, string>();
  private filter: Filter = "all";
  private query = "";
  private focusId: string | null = null;
  private sheet: { model: FormModel; handle: SheetHandle } | null = null;
  /** The "Pair a Mac" sheet while it is open (IOS-9). */
  private pairing: PairSheetHandle | null = null;
  /** The "Add a Connection" chooser is open (UI-kinds B). */
  private adding = false;
  private timer: ReturnType<typeof setInterval> | null = null;

  private readonly page: HTMLElement;
  private readonly count: HTMLElement;
  private readonly filterButtons: Record<Filter, HTMLButtonElement>;
  private readonly noticeSlot: HTMLElement;
  private readonly region: HTMLElement;
  private readonly menuLayer: HTMLElement;
  private readonly sheetLayer: HTMLElement;
  private readonly confirmLayer: HTMLElement;

  constructor(
    root: HTMLElement,
    private readonly api: ConnectionsApi,
    private readonly now: () => number = () => Date.now(),
  ) {
    const seg = (f: Filter, label: string) =>
      h("button", { type: "button", "aria-pressed": String(f === this.filter), onclick: () => this.setFilter(f) }, label);
    this.filterButtons = { all: seg("all", "All"), mac: seg("mac", "Macs"), gnome: seg("gnome", "GNOME"), open: seg("open", "Open") };
    this.count = h("span", { class: "count" });
    const search = h("input", {
      type: "search",
      placeholder: "Search",
      "aria-label": "Search connections",
      spellcheck: "false",
      oninput: () => {
        this.query = search.value;
        this.renderGallery();
      },
    });
    // The toolbar is the window's transparent title bar: dragging it moves the window, and the
    // first 88 points belong to the traffic lights.
    const toolbar = h(
      "header",
      { class: "toolbar", "data-tauri-drag-region": true },
      h("h1", { "data-tauri-drag-region": true }, "Connections"),
      this.count,
      h("span", { class: "spacer", "data-tauri-drag-region": true }),
      h("div", { class: "seg", role: "group", "aria-label": "Show" }, Object.values(this.filterButtons)),
      h("div", { class: "search" }, icon("search"), search),
      // One add button (UI-kinds B): it asks for the kind first.
      h("button", { type: "button", class: "tb-btn", "aria-label": "New Connection", title: "Add a Mac or GNOME computer (⌘N)", onclick: () => this.openAdd() }, icon("plus")),
    );
    this.noticeSlot = h("div", { class: "notice-slot" });
    this.region = h("div", { class: "gallery-scroll" });
    this.page = h("div", { class: "connections" }, toolbar, this.noticeSlot, this.region);
    this.menuLayer = h("div", { class: "menu-layer" });
    this.sheetLayer = h("div", { class: "sheet-layer" });
    this.confirmLayer = h("div", { class: "confirm-layer" });
    mount(root, this.page, this.menuLayer, this.sheetLayer, this.confirmLayer);
  }

  /** Loads profiles and open connections; with no profiles yet, opens the New Connection sheet. */
  async start(): Promise<void> {
    await Promise.all([this.reload(), this.loadConnections()]);
    this.renderGallery();
    // A drift-pair: link that launched the app opens "Pair a Mac" instead (IOS-9).
    const link = (await this.api.takePairingLink()) as string | null;
    if (link) await this.openPairing(link);
    else if (this.entries.length === 0 && this.sheet === null) this.openAdd();
  }

  /** Open connections changed (the `connectionsChanged` event). */
  onConnections(c: Connections): void {
    this.open = c.open;
    this.openSince = this.now();
    const open = new Set(c.open.map((o) => o.profile_id));
    for (const id of [...this.thumbnails.keys()]) if (!open.has(id)) this.thumbnails.delete(id);
    this.renderGallery();
    const sheet = this.sheet;
    if (sheet && !sheet.model.isNew && sheet.model.open !== open.has(sheet.model.profile.id)) {
      this.showSheet({ ...sheet.model, ...draftFields(sheet.handle.draft()), open: !sheet.model.open });
    }
  }

  /** A live preview arrived or went away (the `thumbnailUpdated` event). */
  onThumbnail(t: Thumbnail): void {
    // Kept even if the push that opens the profile has not arrived yet; `onConnections` drops
    // the previews of profiles without a window.
    if (t.image === null) this.thumbnails.delete(t.profile_id);
    else this.thumbnails.set(t.profile_id, t.image);
    this.renderGallery();
  }

  /** Cmd+N / Cmd+E / Edit Connection… from anywhere (the `connectionsIntentRequested` event). */
  async onIntent(intent: ConnectionsIntent): Promise<void> {
    if (intent.kind === "new") {
      this.openAdd();
    } else if (intent.kind === "pair") {
      await this.openPairing(intent.uri);
    } else {
      await this.reload();
      this.renderGallery();
      this.openEdit(intent.profile_id);
    }
  }

  /** Drift Hosts found while the pairing sheet is open (the `hostsDiscovered` event). */
  onHostsDiscovered(hosts: HostAdvert[]): void {
    this.pairing?.update(hosts);
  }

  /** Re-reads the clock into the uptime and countdown pills. */
  tick(): void {
    tickPills(this.region, this.now());
  }

  /** Stops timers. */
  dispose(): void {
    this.setTimer(false);
  }

  // ---- data -----------------------------------------------------------------------------

  private async reload(): Promise<void> {
    const r = (await this.api.listProfiles()) as Result<ProfileEntry_Serialize[]>;
    if (r.status === "ok") this.entries = r.data;
    else this.notice(describeError(r.error));
  }

  private async loadConnections(): Promise<void> {
    const r = (await this.api.connections()) as Result<Connections>;
    if (r.status === "ok") {
      this.open = r.data.open;
      this.openSince = this.now();
    }
  }

  private isOpen(id: string): boolean {
    return this.open.some((o) => o.profile_id === id);
  }

  private entry(id: string): ProfileEntry_Serialize | undefined {
    return this.entries.find((e) => e.profile.id === id);
  }

  /** Runs a command; an error becomes the page's notice. */
  private async run<T>(p: Promise<Result<T>>): Promise<T | null> {
    const r = await p;
    if (r.status === "error") {
      this.notice(describeError(r.error));
      return null;
    }
    this.notice(null);
    return r.data;
  }

  private notice(text: string | null): void {
    if (text === null) mount(this.noticeSlot);
    else mount(this.noticeSlot, h("p", { class: "notice", role: "alert" }, text));
  }

  // ---- gallery actions ------------------------------------------------------------------

  private setFilter(f: Filter): void {
    this.filter = f;
    for (const [k, b] of Object.entries(this.filterButtons)) b.setAttribute("aria-pressed", String(k === f));
    this.renderGallery();
  }

  private primary(id: string): void {
    void this.run(this.isOpen(id) ? this.api.showWindow(id) : this.api.connect(id));
  }

  private async duplicate(id: string): Promise<void> {
    const copy = await this.run(this.api.duplicateProfile(id) as Promise<Result<ProfileEntry_Serialize>>);
    if (!copy) return;
    await this.reload();
    this.focusId = copy.profile.id;
    this.renderGallery();
  }

  private askRemove(id: string): void {
    const e = this.entry(id);
    if (!e) return;
    const done = () => {
      mount(this.confirmLayer);
      this.updateInert();
    };
    const mac = kindOf(e.profile) === "mac";
    renderConfirm(this.confirmLayer, {
      title: mac ? `Unpair “${e.profile.name}”?` : `Delete “${e.profile.name}”?`,
      message: mac
        ? "Drift forgets this Mac and its pinned key. To connect again, pair with a new code from the Mac."
        : "Its saved passwords are removed from your Keychain too. This can’t be undone.",
      action: mac ? "Unpair" : "Delete",
      cancel: () => {
        done();
        this.refocus();
      },
      confirm: () => {
        done();
        void this.remove(id);
      },
    });
    this.updateInert();
  }

  private async remove(id: string): Promise<void> {
    if (this.sheet?.model.profile.id === id) this.closeSheet();
    const r = (await this.api.deleteProfile(id)) as Result<null>;
    if (r.status === "error") {
      this.notice(describeError(r.error));
      return;
    }
    this.thumbnails.delete(id);
    await this.reload();
    this.renderGallery();
    this.refocus();
  }

  private menu(id: string, at: { x: number; y: number }): void {
    const e = this.entry(id);
    if (!e) return;
    const open = this.isOpen(id);
    openMenu(
      this.menuLayer,
      e.profile.name,
      [
        { label: primaryLabel(open), shortcut: "↩", keys: "Enter", run: () => this.primary(id) },
        { label: "Edit…", shortcut: "⌘E", keys: "Meta+E", run: () => this.openEdit(id) },
        { label: "Duplicate", shortcut: "⌘D", keys: "Meta+D", run: () => void this.duplicate(id) },
        ...(open ? [{ label: "Disconnect", run: () => void this.run(this.api.disconnectProfile(id)) }] : []),
        { label: "Delete…", shortcut: "⌘⌫", keys: "Meta+Backspace", destructive: true, run: () => this.askRemove(id) },
      ],
      at,
      () => this.card(id),
    );
  }

  private card(id: string): HTMLElement | null {
    return this.region.querySelector<HTMLElement>(`[data-profile-id="${id}"]`);
  }

  private refocus(): void {
    const target = (this.focusId && this.card(this.focusId)) || this.region.querySelector<HTMLElement>(".card[tabindex='0']");
    target?.focus();
  }

  // ---- the sheet ------------------------------------------------------------------------

  /** "Add a Connection": which kind comes next (UI-kinds B). */
  private openAdd(): void {
    if (this.pairing) this.closePairing();
    if (this.sheet) this.closeSheet();
    this.adding = true;
    const close = () => {
      this.adding = false;
      mount(this.sheetLayer);
      this.updateInert();
    };
    renderAddSheet(this.sheetLayer, {
      pairMac: () => {
        close();
        void this.openPairing();
      },
      addGnome: () => {
        close();
        void this.openNew();
      },
      cancel: () => {
        close();
        this.refocus();
      },
    });
    this.updateInert();
  }

  private async openNew(): Promise<void> {
    const p = (await this.api.newProfile(DEFAULT_MODE)) as ConnectionProfile_Serialize;
    this.showSheet(formModel(p, { isNew: true, hasRdpPassword: false, hasLinuxPassword: false }));
  }

  private openEdit(id: string): void {
    const e = this.entry(id);
    if (!e) {
      this.notice(describeError({ kind: "not-found" }));
      return;
    }
    this.focusId = id;
    this.showSheet(
      formModel(e.profile, { isNew: false, hasRdpPassword: e.has_rdp_password, hasLinuxPassword: e.has_linux_password, open: this.isOpen(id) }),
    );
  }

  private showSheet(model: FormModel): void {
    const handle = renderSheet(this.sheetLayer, model, {
      change: (d) => this.showSheet({ ...model, ...draftFields(d) }),
      validate: (p) => this.api.validateProfile(p),
      save: (d, connect) => void this.save(d, connect),
      cancel: () => this.closeSheet(),
      remove: () => this.askRemove(model.profile.id),
      forgetCertificate: () => void this.forgetCertificate(),
      connect: (d) => void this.connectFromSheet(d),
      showWindow: () => void this.run(this.api.showWindow(model.profile.id)),
    });
    this.sheet = { model, handle };
    this.updateInert();
  }

  private closeSheet(): void {
    this.sheet = null;
    mount(this.sheetLayer);
    this.updateInert();
    this.refocus();
  }

  private async save(draft: FormDraft, connect: boolean): Promise<void> {
    const sheet = this.sheet;
    if (!sheet) return;
    const model = { ...sheet.model, ...draftFields(draft) };
    this.showSheet({ ...model, busy: true, error: null });
    const r = (await this.api.saveProfile(draft.profile, toSecretsUpdate(draft, model.hasLinuxPassword))) as Result<ProfileEntry_Serialize>;
    if (r.status === "error") {
      const e = r.error;
      this.showSheet({ ...model, busy: false, issues: e.kind === "invalid" ? e.issues : [], error: e.kind === "invalid" ? null : describeError(e) });
      return;
    }
    const id = r.data.profile.id;
    this.focusId = id;
    this.closeSheet();
    await this.reload();
    this.renderGallery();
    this.refocus();
    if (connect) await this.run(this.api.connect(id));
  }

  /** The header's Connect: saves first if something changed. */
  private async connectFromSheet(draft: FormDraft): Promise<void> {
    const sheet = this.sheet;
    if (!sheet) return;
    if (isDirty(sheet.model, draft)) {
      await this.save(draft, true);
      return;
    }
    this.closeSheet();
    await this.run(this.api.connect(sheet.model.profile.id));
  }

  private async forgetCertificate(): Promise<void> {
    const sheet = this.sheet;
    if (!sheet) return;
    const id = sheet.model.profile.id;
    const r = await this.run(this.api.forgetCertificate(id) as Promise<Result<ProfileEntry_Serialize>>);
    if (!r) return;
    await this.reload();
    const d = sheet.handle.draft();
    this.showSheet({ ...sheet.model, ...draftFields(d), profile: { ...d.profile, cert_pin: null }, original: { ...sheet.model.original, cert_pin: null } });
  }

  // ---- pairing a Mac (IOS-9) -----------------------------------------------------------

  private async openPairing(link?: string): Promise<void> {
    if (this.pairing) {
      if (link) this.pairing.setCode(link);
      return;
    }
    if (this.sheet) this.closeSheet();
    this.pairing = renderPairSheet(this.sheetLayer, {
      read: async (text) => {
        const r = (await this.api.readPairingCode(text)) as Result<PairingInput>;
        return r.status === "ok" ? r.data : describeError(r.error);
      },
      scan: async () => {
        const r = (await this.api.scanPairingQr()) as Result<string>;
        return r.status === "ok" ? { text: r.data } : { error: describeError(r.error) };
      },
      pair: async (request) => {
        const r = (await this.api.pairHost(request)) as Result<ProfileEntry_Serialize>;
        if (r.status === "error") return describeError(r.error);
        this.focusId = r.data.profile.id;
        this.closePairing();
        await this.reload();
        this.renderGallery();
        this.refocus();
        // HOST-15: approval happens on the first connection, so make it now; its window says
        // to click Allow on the Mac.
        if (request.kind === "approve") await this.run(this.api.connect(r.data.profile.id));
        return null;
      },
      cancel: () => this.closePairing(),
    });
    this.updateInert();
    if (link) this.pairing.setCode(link);
    const r = (await this.api.startHostDiscovery()) as Result<null>;
    if (r.status === "error") this.notice(describeError(r.error));
  }

  private closePairing(): void {
    if (!this.pairing) return;
    this.pairing = null;
    void this.api.stopHostDiscovery();
    mount(this.sheetLayer);
    this.updateInert();
    this.refocus();
  }

  // ---- rendering ------------------------------------------------------------------------

  private updateInert(): void {
    const confirm = this.confirmLayer.childElementCount > 0;
    const sheet = this.sheet !== null || this.pairing !== null || this.adding;
    this.page.toggleAttribute("inert", sheet || confirm);
    this.sheetLayer.toggleAttribute("inert", confirm);
  }

  private renderGallery(): void {
    this.count.textContent = String(this.entries.length);
    renderGallery(
      this.region,
      {
        entries: this.entries,
        open: this.open,
        since: this.openSince,
        thumbnails: this.thumbnails,
        filter: this.filter,
        query: this.query,
        now: this.now(),
        focusId: this.focusId,
      },
      {
        primary: (id) => this.primary(id),
        menu: (id, at) => this.menu(id, at),
        create: () => this.openAdd(),
        edit: (id) => this.openEdit(id),
        duplicate: (id) => void this.duplicate(id),
        remove: (id) => this.askRemove(id),
        focus: (id) => {
          this.focusId = id;
        },
      },
    );
    this.setTimer(this.open.some((o) => o.status === "live" || o.status === "reconnecting"));
  }

  private setTimer(on: boolean): void {
    if (on && this.timer === null) this.timer = setInterval(() => this.tick(), 1000);
    if (!on && this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }
}

function draftFields(d: FormDraft & { touched?: FormModel["touched"] }): Partial<FormModel> {
  return {
    profile: d.profile,
    rdpPassword: d.rdpPassword,
    linuxPassword: d.linuxPassword,
    typeLinuxPassword: d.typeLinuxPassword,
    ...(d.touched ? { touched: d.touched } : {}),
  };
}
