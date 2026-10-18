// A fake of the Connections window's IPC commands (the generated bindings' shapes) that records
// every call, plus a helper that starts a ConnectionsApp against it.
import type {
  CommandError,
  ConnectionProfile_Deserialize,
  ConnectionProfile_Serialize,
  ConnectMode,
  OpenConnection,
  ProfileEntry_Serialize,
  ProfileIssue,
  SecretsUpdate,
} from "../src/bindings";
import { type ConnectionsApi, ConnectionsApp } from "../src/connectionsApp";
import { entry, flush, profile } from "./helpers";

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };
const ok = <T>(data: T): Promise<Result<T>> => Promise.resolve({ status: "ok", data });

/** What `validate_profile` says about the fields every test fills in (a subset of Rust's rules). */
export function fakeValidate(p: ConnectionProfile_Deserialize): ProfileIssue[] {
  const issues: ProfileIssue[] = [];
  if (p.name.trim() === "") issues.push({ field: "name", problem: "empty", message: "Name is required." });
  if (p.host.trim() === "") issues.push({ field: "host", problem: "empty", message: "Host is required." });
  if (p.rdp_username.trim() === "") issues.push({ field: "rdp-username", problem: "empty", message: "RDP user name is required." });
  return issues;
}

export class FakeConnectionsApi {
  calls: unknown[][] = [];
  /** What `scan_pairing_qr` returns (IOS-9); `null` = no camera. */
  scanned: string | null = null;
  /** What `pair_host` fails with, if anything. */
  pairError: CommandError | null = null;
  entries: ProfileEntry_Serialize[] = [];
  open: OpenConnection[] = [];
  saveIssues: ProfileIssue[] | null = null;
  commandError: CommandError | null = null;

  names(): string[] {
    return this.calls.map((c) => String(c[0]));
  }

  api(): ConnectionsApi {
    const log = (...c: unknown[]) => this.calls.push(c);
    const done = (): Promise<Result<null>> =>
      this.commandError ? Promise.resolve({ status: "error", error: this.commandError }) : ok(null);
    return {
      listProfiles: () => (log("listProfiles"), ok([...this.entries])),
      newProfile: (mode: ConnectMode) => (
        log("newProfile", mode),
        Promise.resolve(profile(mode, { name: "", host: "", rdp_username: "", linux_username: null }))
      ),
      validateProfile: (p: ConnectionProfile_Deserialize) => (log("validateProfile", p), Promise.resolve(fakeValidate(p))),
      saveProfile: (p: ConnectionProfile_Deserialize, s: SecretsUpdate) => {
        log("saveProfile", p, s);
        if (this.saveIssues) return Promise.resolve({ status: "error", error: { kind: "invalid", issues: this.saveIssues } });
        const e = entry(p as ConnectionProfile_Serialize, true, false);
        this.entries = [...this.entries.filter((x) => x.profile.id !== p.id), e];
        return ok(e);
      },
      deleteProfile: (id: string) => {
        log("deleteProfile", id);
        this.entries = this.entries.filter((x) => x.profile.id !== id);
        return ok(null);
      },
      duplicateProfile: (id: string) => {
        log("duplicateProfile", id);
        const src = this.entries.find((x) => x.profile.id === id)!;
        const copy = entry(profile(src.profile.mode, { ...src.profile, id: `${id.slice(0, -4)}copy`, name: `${src.profile.name} copy`, cert_pin: null }));
        this.entries = [...this.entries, copy];
        return ok(copy);
      },
      forgetCertificate: (id: string) => (log("forgetCertificate", id), ok(this.entries.find((e) => e.profile.id === id)!)),
      connect: (id: string) => (log("connect", id), done()),
      showWindow: (id: string) => (log("showWindow", id), done()),
      disconnectProfile: (id: string) => (log("disconnectProfile", id), done()),
      connections: () => (log("connections"), ok({ open: [...this.open] })),
      startHostDiscovery: () => (log("startHostDiscovery"), ok(null)),
      stopHostDiscovery: () => (log("stopHostDiscovery"), ok(null)),
      scanPairingQr: () => (
        log("scanPairingQr"),
        this.scanned === null
          ? Promise.resolve({ status: "error", error: { kind: "pairing", message: "No camera." } })
          : ok(this.scanned)
      ),
      readPairingCode: (text: string) => {
        log("readPairingCode", text);
        if (text.startsWith("drift-pair:")) {
          return ok({ kind: "qr", uri: text, name: "Studio Mac", host: "studio.local", port: 3389 });
        }
        const digits = text.replace(/[\s-]/g, "");
        if (/^\d{8}$/.test(digits)) return ok({ kind: "code", code: `${digits.slice(0, 4)} ${digits.slice(4)}` });
        return Promise.resolve({ status: "error", error: { kind: "pairing", message: "That code has a typo." } });
      },
      pairHost: (request: unknown) => {
        log("pairHost", request);
        if (this.pairError) return Promise.resolve({ status: "error", error: this.pairError });
        const e = entry(profile("headless", { name: "Studio Mac", host: "studio.local", cert_pin: "ab:ab", paired_host: "77" }));
        this.entries = [...this.entries, e];
        return ok(e);
      },
    } as unknown as ConnectionsApi;
  }
}

/** Starts a Connections page against `fake`; `clock.now` drives pills and countdowns. */
export async function startConnections(fake = new FakeConnectionsApi(), clock = { now: 1_000_000 }) {
  const root = document.createElement("main");
  root.id = "app";
  document.body.replaceChildren(root);
  const app = new ConnectionsApp(root, fake.api(), () => clock.now);
  await app.start();
  await flush();
  return { root, app, fake, clock };
}
