// RESP-1: the page the breakpoint harness loads. Renders one state of the real controllers
// (`ConnectionsApp`, `SessionApp`) from the `ui/test` fakes and fixtures, chosen by the URL hash,
// then sets `data-ready` on <html> (or `data-error` with the stack).
import { ConnectionsApp } from "../src/connectionsApp";
import { applyPlatform } from "../src/platform";
import { SessionApp } from "../src/sessionApp";
import { FakeConnectionsApi } from "../test/fakes";
import { certPrompt, entry, LNP_EXPLANATION, openConnection, profile, sessionView, stats } from "../test/helpers";

const wait = (ms = 50) => new Promise((r) => setTimeout(r, ms));

function named(sel: string, name: string): HTMLElement {
  const el = Array.from(document.querySelectorAll<HTMLElement>(sel)).find(
    (b) => (b.getAttribute("aria-label") ?? b.textContent ?? "").trim() === name,
  );
  if (!el) throw new Error(`no ${sel} named ${name}`);
  return el;
}

/** Long names and hosts on purpose: they are what overflows first. */
function entries() {
  const pin = "f3:e7:a2:68:ee:a5:64:38:f8:14:03:12:50:4d:44:97:6c:dc:c3:ae:f2:4a:98:92:da:97:72:fe:b6:e6:fe:81";
  return [
    entry(profile("remote-login", { name: "Homelab workstation (upstairs office)", host: "homelab-workstation.tail1234.ts.net" })),
    entry(profile("headless", { name: "Build box", host: "10.0.2.41", cert_pin: pin })),
    entry(profile("desktop-sharing", { name: "Living room", host: "living-room.local" })),
    entry(profile("remote-login", { name: "Lab", host: "192.168.50.200" })),
  ];
}

async function connections(state: string): Promise<void> {
  document.body.className = "connections";
  const fake = new FakeConnectionsApi();
  if (state !== "c-welcome") {
    fake.entries = entries();
    fake.open = [
      openConnection(fake.entries[0]!.profile.id, "live", { live_secs: 5400 }),
      openConnection(fake.entries[1]!.profile.id, "reconnecting"),
    ];
  }
  const app = new ConnectionsApp(document.getElementById("app")!, fake.api());
  await app.start();
  await wait();
  switch (state) {
    case "c-new":
      named("header.toolbar button", "New Connection").click();
      break;
    case "c-new-mode": {
      named("header.toolbar button", "New Connection").click();
      await wait();
      document.querySelector<HTMLInputElement>('.segment input[value="remote-login"]')?.click();
      break;
    }
    case "c-edit":
      await app.onIntent({ kind: "edit", profile_id: fake.entries[1]!.profile.id } as never);
      await wait();
      document.querySelector("details.advanced")?.setAttribute("open", "");
      break;
    case "c-menu": {
      const more = document.querySelectorAll<HTMLElement>(".icon-btn.more");
      more[more.length - 1]!.click();
      break;
    }
    case "c-confirm":
      document.querySelector<HTMLElement>(".icon-btn.more")!.click();
      await wait();
      Array.from(document.querySelectorAll<HTMLElement>(".menu [role=menuitem]"))
        .find((m) => (m.textContent ?? "").includes("Delete"))!
        .click();
      break;
    case "c-search": {
      // Compact toolbars hide the field behind a button; open it the way a user would.
      document.querySelector<HTMLElement>(".search-toggle:not([hidden])")?.click();
      await wait();
      const s = document.querySelector<HTMLInputElement>("input[type=search]")!;
      s.value = "zzz";
      s.dispatchEvent(new Event("input", { bubbles: true }));
      break;
    }
  }
  await wait();
}

const LIVE = { state: "connected", desktop: { width: 1280, height: 800 }, scale: 100 } as const;
const CONNECTING = { state: "connecting", leg: 1, stage: "tls" } as const;

async function session(state: string): Promise<void> {
  document.body.className = "session";
  const ok = (data: unknown) => Promise.resolve({ status: "ok", data });
  const ident = { profile_id: "x", name: "Homelab", host: "h", mode: "remote-login", status: "live", hint: null, show_stats: false };
  const controls = { visible: state === "s-keys", touch: "trackpad" };
  const api = new Proxy(
    {},
    {
      get: (_t, k) =>
        k === "windowIdentity" ? () => ok(ident) : k === "phoneControls" ? () => Promise.resolve(controls) : () => ok(null),
    },
  );
  const app = new SessionApp(document.getElementById("app")!, api as never);
  await app.start();
  const views: Record<string, unknown> = {
    "s-connecting": sessionView("connecting", { state: "connecting", leg: 2, stage: "nla" }),
    "s-certificate": sessionView("certificate", CONNECTING, { certificate: certPrompt() }),
    "s-error": sessionView("error", { state: "disconnected", reason: { kind: "network" } } as never, { explanation: LNP_EXPLANATION }),
    "s-reconnecting": sessionView("reconnecting", { state: "reconnecting", attempt: 3, next_in_ms: 4000 } as never),
    "s-banner": sessionView("greeter-hint", LIVE),
    "s-stats": sessionView("live", LIVE, { show_stats: true, stats: stats() }),
    "s-keys": sessionView("live", LIVE),
  };
  app.onSessionView(views[state] as never);
  await wait();
}

applyPlatform();
const state = location.hash.slice(1);
(state.startsWith("c-") ? connections(state) : session(state))
  .then(() => document.documentElement.setAttribute("data-ready", "1"))
  .catch((e: unknown) => {
    document.documentElement.setAttribute("data-ready", "error");
    document.documentElement.setAttribute("data-error", String(e instanceof Error ? e.stack : e));
  });
