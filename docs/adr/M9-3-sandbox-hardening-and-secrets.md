# M9-3 — App Sandbox, Hardened Runtime, and keeping credentials out of logs and memory

- Status: accepted
- Task: plan M9-3 (credentials never in logs; one-time redirect credentials zeroized; TOFU pins
  for the system daemon *and* the redirect target; App Sandbox + Hardened Runtime +
  `NSLocalNetworkUsageDescription`; the errno 65 screen links to Local Network settings)
- Code: `src-tauri/entitlements.plist`, `src-tauri/tauri.conf.json`,
  `crates/drift-rdp/src/logging.rs`, `src-tauri/src/lib.rs`, `tests/e2e/src/lib.rs`,
  `third_party/ironrdp-patches/0007-fix-never-log-credentials.patch`
- Tests: `src-tauri/tests/m9_3_hardening.rs`, `crates/drift-rdp/tests/m9_3_logging.rs`,
  `crates/drift-rdp/tests/m9_3_log_filter.rs`, `crates/drift-rdp/tests/m9_3_zeroize.rs`,
  `crates/drift-rdp/tests/m9_3_tofu.rs`, `docs/acceptance.md` § M9-3

## 1. Credentials in logs: refuse to enable the targets that leak

The plan asks for "a redaction test on e2e logs". Redaction turned out to be the weaker half of
the answer. Running a whole Remote Login connection under a process-wide `TRACE` subscriber and
searching the output for each credential — in text, byte-list, hex **and UTF-16LE** form —
found three leaks that a redactor would only have covered inside `cargo xtask e2e`:

1. `sspi` (CredSSP/NTLM, a dependency, not vendored) is `#[instrument]`ed down to the buffers it
   writes: at `DEBUG`/`TRACE` it logs the TS credentials, i.e. the **profile password** as a
   decimal byte list of its UTF-16LE encoding, plus the user name as hex.
2. The fork's own RDSTLS state logged `username = %credentials.username`, i.e. the one-time
   logon name of the redirected leg.
3. Upstream's `ClientInfoPdu` `debug!` prints `Credentials`, whose `Debug` hid the password but
   printed the user name — which, with Server Redirection, is itself a one-time secret.

Decisions:

- **(1) is a policy, not a patch.** `drift_rdp::logging::allows(target, level)` refuses
  `sspi` (and anything under it) below `INFO`, and every place Drift installs a subscriber
  (`drift-app`'s `init_logging`, `drift-e2e`'s `init_logging`) adds it as a second **global
  filter layer** beside the `EnvFilter`. `RUST_LOG=trace` therefore still gives full Drift and
  IronRDP tracing, but `sspi` stops at `INFO` — its warnings and errors (the useful part when
  NLA fails) survive, its credential spans do not.

  The first attempt appended an `sspi=info` *directive* to the `EnvFilter` instead. That is not
  enough, and `m9_3_log_filter.rs` caught it: `EnvFilter` matches the **most specific** target
  first, so `RUST_LOG=sspi::credssp=trace` still enables the `write_ts_credentials` span, and
  its credential field is then printed in the context of every event inside it — including the
  `WARN` ones the cap deliberately keeps. A predicate over the callsite's target cannot be
  out-specified that way. The policy stays a pure function in `drift-rdp` (no
  `tracing-subscriber` dependency there); the proof is behavioural, in `m9_3_log_filter.rs`
  (every `RUST_LOG` shape) and `m9_3_logging.rs` (a whole connection at `trace`, grepped).
- **(2) and (3) are fixed in the vendored fork**
  (`0007-fix-never-log-credentials.patch`): the RDSTLS line no longer names the user, and
  `Credentials`'s `Debug` prints `username: <13 chars>` instead of the value. The length keeps
  the line useful for debugging a wrong-user problem without disclosing a one-time logon name.
  Both are upstreamable and come with a test in `ironrdp-testsuite-core`.

## 2. One-time credentials: proven zeroized at the allocator

M3-1 already wrapped the redirect credentials in `Zeroizing` and clears the PDU's fields.
`Debug` assertions cannot show that the heap is clean, so `m9_3_zeroize.rs` installs a
`#[global_allocator]` that, while armed, scans every block being freed for a canary. The test
has a **positive control** (an unzeroized `String` must be detected, or the detector is broken)
and then asserts zero hits for the whole flow: PDU → `OneTimeCredentials` → `RdstlsCredentials`
→ drop. The window is guarded by a mutex because `cargo test` runs a binary's tests on threads.

## 3. TOFU pins cover the redirect target

`m9_3_tofu.rs` covers the case the g-r-d captures never show: a redirection PDU **without**
`LB_TARGET_CERTIFICATE`. The M3-1 path (byte-identical target certificate) does not apply, so
the redirected leg falls back to the pin Drift already trusts — the profile pin, or the one the
user accepted for this session. A redirect target that presents a *different* certificate fails
with `CertMismatch` and the one-time credentials are never sent.

Drift deliberately does **not** prompt in that case: the redirect goes to the same host and
port, so a changed certificate is not a new host to decide about, it is the certificate the user
just approved being swapped mid-connection by whoever controls the redirect. Prompting would
train users to click through exactly the attack the pin exists to stop.

## 4. Sandbox and Hardened Runtime

`bundle.macOS.entitlements = "entitlements.plist"` and `hardenedRuntime: true` in
`tauri.conf.json`; the entitlements contain exactly two keys:

```xml
<key>com.apple.security.app-sandbox</key><true/>
<key>com.apple.security.network.client</key><true/>
```

Everything else Drift needs is inside the default sandbox, and the test asserts that no
`temporary-exception`, no `network.server` and no Hardened-Runtime escape hatch
(`cs.disable-library-validation`, `cs.allow-unsigned-executable-memory`) creeps in later:

| Capability | Why no entitlement is needed |
|---|---|
| Keychain | A sandboxed app reaches its own generic-password items through the access group `<team>.<bundle id>`. Drift stores under the service `com.example.drift`, which the test pins to `tauri.conf.json`'s `identifier`; `keychain-access-groups` would only widen the reach. M3-2 uses the legacy `SecKeychain` API, which sandboxed apps may use for their own items — if the signed build ever disagrees, the answer is the data-protection keychain (`kSecUseDataProtectionKeychain`), not a wider entitlement (acceptance § M9-3). |
| Metal | `CAMetalLayer`, `MTLDevice` and `IOSurface`-backed textures are sandbox-safe. |
| VideoToolbox | Decode and encode sessions run in-process; no helper, no XPC of ours. |
| Pasteboard | `NSPasteboard.general` is the user's own pasteboard. |
| Config | `app_config_dir()` is redirected into the container; nothing hard-codes a path (`DRIFT_CONFIG_DIR` is a dev-only override). Drift never opens user files. |

**Local Network Privacy is not an entitlement** (plan §1.8): it is a user permission triggered
by the first LAN connection, needing `NSLocalNetworkUsageDescription` in `Info.plist` (present)
and a stable signing identity so it survives rebuilds. `DisconnectReason::LocalNetworkDenied`
(errno 65) already maps to an error screen whose action opens
`x-apple.systempreferences:…?Privacy_LocalNetwork`.

The sandbox itself only exists for a signed bundle, so `cargo test` can only pin the
configuration. The behaviour (container, Keychain without a prompt, Metal, VideoToolbox,
pasteboard, config location, the Local Network prompt) is a manual checklist item under
`docs/acceptance.md` § M9-3, to be run against the M9-5 signed build.

## Consequences

- `sspi` cannot be debugged at `TRACE` through Drift's subscriber. If that is ever needed, it
  must be a deliberate, separately-built binary — not a `RUST_LOG` away in a shipped app.
- The vendored fork carries one more patch to upstream.
- Turning the sandbox on changes where the app writes: an existing unsandboxed dev install's
  `~/Library/Application Support/com.example.drift` is not migrated into the container, and
  Keychain items written by a differently-signed build stay unreadable (the M6/M9 acceptance
  note about the `apple-tool:` partition already covers that class of surprise).
