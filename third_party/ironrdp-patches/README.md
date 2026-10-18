# IronRDP patches

Drift's changes to the vendored IronRDP (`third_party/ironrdp`, upstream rev `b149f50`), one
`git format-patch` file per change, in apply order. They are the upstreaming queue: each applies
with `git am` (or `patch -p1`) onto upstream `b149f500b85124c513646494335fb6cee525d897`, and
applying all of them to the pristine vendored tree reproduces `third_party/ironrdp` exactly.

| Patch | Crates | Change |
|---|---|---|
| `0001-feat-connector-request-RDSTLS-on-redirected-connections.patch` | connector | Request `PROTOCOL_RDSTLS` when load-balance info is set and NLA is off (the connector part of the spike's `ironrdp-drift.patch`) |
| `0002-feat-pdu-decode-Enhanced-Security-Server-Redirection-PDU.patch` | pdu | `ServerRedirectionPdu` (share control `pduType` 0xA, `pad2Octets`, `RDP_SERVER_REDIRECTION_PACKET`), target certificate container, `ShareControlPdu::ServerRedirect` |
| `0003-feat-session-surface-ActiveStageOutput-ServerRedirect.patch` | pdu, session, client, web | `IoChannelPdu`/`ProcessorOutput`/`ActiveStageOutput::ServerRedirect` |
| `0004-feat-connector-RDSTLS-client-authentication.patch` | connector | `rdstls` module and `RdstlsCapabilities`/`RdstlsAuthResponse` connector states; `ConnectorErrorKind::RdstlsAuthFailed` |
| `0005-fix-cliprdr-validate-CB_TEMP_DIRECTORY-and-make-it-optional.patch` | cliprdr | `dataLen`/terminator validation; skip the optional PDU when the backend has no temp directory |
| `0006-feat-connector-zeroize-RdstlsCredentials-on-drop.patch` | connector | Zeroize the one-time RDSTLS credentials (GUID, user, domain, password) on drop (M3-1) |
| `0007-fix-never-log-credentials.patch` | connector, pdu | Stop logging credentials at DEBUG: the RDSTLS request no longer names the one-time user, and `Credentials`'s `Debug` prints the user name's length instead of its value (M9-3) |
| `0008-feat-ironrdp-server-per-connection-dvc-factories.patch` | server | `RdpServer::add_dvc_factory`: an embedder attaches its own dynamic virtual channels to each connection (Drift Host offers `Drift::Mac::v1`, HOST-2; covered by `drift-host`'s loopback test) |

Each patch squashes the Red test commit and the implementation commit of one change; the tests
live in `ironrdp-testsuite-core`. Regenerate them after changing a commit pair:

```sh
tree=$(git rev-parse <impl-commit>^{tree}); parent=$(git rev-parse <test-commit>^)
sha=$(printf '%s\n' "<upstream commit message>" | git commit-tree "$tree" -p "$parent")
git format-patch -1 "$sha" --relative=third_party/ironrdp --stdout > third_party/ironrdp-patches/NNNN-<slug>.patch
```

See `docs/adr/M0-2-ironrdp-vendored-fork.md` and `docs/adr/M0-2-ironrdp-redirect-rdstls-api.md`.
Do not open upstream PRs without the owner's consent.
