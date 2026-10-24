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
  RunMode,
  SecretsUpdate,
  SetupError,
  SetupIssue,
  SetupLine,
  SetupOptions,
  SetupResult_Serialize,
  SignIn,
  Target,
  Trust,
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

/** One scripted `setup_run`: the lines it streams, then its result. */
export interface FakeRun {
  lines: SetupLine[];
  result: Result<SetupResult_Serialize, SetupError>;
}

/** What the Rust side's `SetupOptions::defaults` plus a generated password give. */
export function fakeDefaults(host: string, account: string): SetupOptions {
  const name = /^[0-9.:]+$/.test(host) ? host : (host.split(".")[0] ?? host);
  return {
    name,
    account,
    port: 3390,
    rdp_username: account,
    rdp_password: "GeneratedPassword1234567",
    remote_login: false,
    install_desktop: true,
    open_firewall: true,
    allow_from: null,
  };
}

export class FakeConnectionsApi {
  calls: unknown[][] = [];
  /** The unsaved profile of a `drift-connect:` link that launched the app (`take_connect_link`). */
  pendingLink: ConnectionProfile_Serialize | null = null;
  entries: ProfileEntry_Serialize[] = [];
  open: OpenConnection[] = [];
  saveIssues: ProfileIssue[] | null = null;
  commandError: CommandError | null = null;
  /** `setup_connect` answers in order; the last one repeats. */
  signIns: SignIn[] = [{ kind: "ready", sudo: "passwordless" }];
  /** `setup_install_key`'s answer. */
  installAnswer: SignIn = { kind: "ready", sudo: "passwordless" };
  /** `setup_run` answers in order; the last one repeats. */
  runs: FakeRun[] = [];
  /** Where `setup_run` sends its lines (the test wires it to the page's event handler). */
  emit: ((line: SetupLine) => void) | null = null;
  publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFake drift@mac";

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
      setupDefaults: (host: string, account: string) => (log("setupDefaults", host, account), Promise.resolve(fakeDefaults(host, account))),
      setupValidate: (o: SetupOptions) => {
        log("setupValidate", o);
        const issues: SetupIssue[] = [];
        if (o.name.trim() === "") issues.push("name");
        if (o.port === 0) issues.push("port");
        return Promise.resolve(issues);
      },
      setupPublicKey: () => (log("setupPublicKey"), ok(this.publicKey)),
      setupForgetHost: (host: string, port: number) => (log("setupForgetHost", host, port), ok(null)),
      setupConnect: (t: Target, trust: Trust) => {
        log("setupConnect", t, trust);
        const answer = this.signIns.length > 1 ? this.signIns.shift()! : this.signIns[0]!;
        return ok(answer);
      },
      setupInstallKey: (t: Target, password: string) => (log("setupInstallKey", t, password), ok(this.installAnswer)),
      setupRun: async (o: SetupOptions, mode: RunMode, sudo: string | null) => {
        log("setupRun", o, mode, sudo);
        const run = this.runs.length > 1 ? this.runs.shift()! : this.runs[0];
        if (!run) throw new Error("no scripted setup run");
        for (const line of run.lines) this.emit?.(line);
        await Promise.resolve();
        return run.result;
      },
      setupDisconnect: () => (log("setupDisconnect"), ok(null)),
      takeConnectLink: () => {
        log("takeConnectLink");
        const link = this.pendingLink;
        this.pendingLink = null;
        return Promise.resolve(link);
      },
    } as unknown as ConnectionsApi;
  }
}

/** Starts a Connections page against `fake`; `clock.now` drives pills and countdowns. */
export async function startConnections(fake = new FakeConnectionsApi(), clock = { now: 1_000_000 }, copied: string[] = []) {
  const root = document.createElement("main");
  root.id = "app";
  document.body.replaceChildren(root);
  const app = new ConnectionsApp(root, fake.api(), () => clock.now, (t) => {
    copied.push(t);
    return Promise.resolve();
  });
  fake.emit = (line) => app.onSetupLine({ line });
  await app.start();
  await flush();
  return { root, app, fake, clock };
}

/** "+" in the toolbar: the New Connection sheet. */
export async function newConnection(root: ParentNode): Promise<void> {
  clickNamed(root.querySelector("header.toolbar")!, "New Connection");
  await flush();
}

function clickNamed(root: ParentNode, name: string): void {
  const b = Array.from(root.querySelectorAll("button")).find((x) => (x.getAttribute("aria-label") ?? x.textContent ?? "").trim() === name);
  if (!b) throw new Error(`no button "${name}"`);
  b.click();
}
