# P2P, direct data plane, and port mapping

Status: implemented. This document describes the peer-to-peer design added on
top of the original relay-only tunnel.

## 1. Goals

- **P2P connections**: a peer reaches an exposed local port directly over
  [iroh](https://www.iroh.computer/) (hole-punched QUIC), with an encrypted
  relay as automatic fallback. Payload never has to cross `telesh-server`.
- **Port mirroring / mapping**: expose a local port over the P2P data plane
  (`telesh p2p`), mirror a remote service onto a local port
  (`telesh connect`), and optionally open a real public TCP port through the
  router (UPnP/PCP/NAT-PMP).
- **No regression**: the existing WebSocket relay path for HTTP and raw-TCP
  tunnels keeps working and remains the fallback.

Non-goals: an L3 overlay/VPN, and a browser WebRTC client. The control plane
stays on the existing WebSocket.

## 2. Architecture

```text
  telesh connect <name>            telesh-server                 telesh p2p <port>
  (visitor)                        (control + directory +         (agent)
                                    optional relay/gateway)

  1) WS Visitor + P2PLookup ──────► directory lookup
  2) ◄───────────────────────────── P2PAddr{ EndpointAddr }
  3) iroh connect(EndpointAddr) ═══ hole-punched QUIC ═════════► Endpoint.accept()
       │                                                        accept_bi() → 127.0.0.1:<port>
       └── fallback → iroh relay (self-hosted or n0 default)

  HTTP/TCP ingress:
  public client → server ingress ─(iroh dial, fallback WS)────► agent → local service
```

- **Control plane** (WebSocket): auth, subdomain assignment, the P2P directory,
  and the direct-plane address announcements.
- **Data plane** (iroh QUIC): one iroh bi-stream per public connection. The
  relay is only used when a direct path cannot be established.

## 3. Identity

`crates/telesh/src/state.rs` persists a 32-byte iroh `SecretKey` alongside
`agent_id`. The public half is the `EndpointId`, so an agent keeps the same
identity across reconnects and restarts. Older state files (no key) are
upgraded in place on load.

## 4. Wire protocol additions

All frames are appended to the existing `Frame` enum (bincode keeps variant
order, so new variants go at the end). See `crates/telesh-protocol/src/message.rs`.

| Frame | Direction | Purpose |
|---|---|---|
| `P2PAnnounce(P2PAnnounce)` | agent → server | register `account/name -> EndpointAddr` (account from the session) |
| `P2PLookup { account, name }` | peer → server | resolve `account/name` |
| `P2PAddr(P2PAddrResp)` | server → peer | lookup result (address + opaque token) |
| `Lookup(LookupHello)` | peer → server (first) | lookup-only connection; no registration |
| `IrohReady(EndpointAddr)` | agent → server | announce direct-plane address (re-sent on change) |
| `Traffic(TrafficReport)` | agent → server | periodic counters for traffic the server cannot see |
| `TcpMapped(TcpMapped)` | agent → server | public TCP endpoint from router port mapping |
| `TokenCheck { id, account, name, token }` | agent → server | validate an opaque access token |
| `TokenCheckResp { id, ok }` | server → agent | verdict for `TokenCheck` |

`EndpointAddr` comes from `iroh-base` and is re-exported by `telesh-protocol`.

### 4.1 Stream preamble

The first bytes of every data stream carry a credential and the service the
caller wants, followed by the raw payload:

```
kind 0 (shared secret): [0][u16 secret_len][secret][u16 service_len][service]
kind 1 (access token) : [1][u16 token_len][token][u16 service_len][service]
```

The acceptor checks a kind-0 secret itself; a kind-1 token it forwards to the
server (`TokenCheck`). Then it resolves `service` to a local port. http/tcp
agents send an empty service name, because they expose a single target. The
acceptor answers with `STREAM_ACK_OK`/`STREAM_ACK_ERR`.

## 4b. Multi-service exposure

One `telesh p2p` process can expose several ports over a **single** iroh
endpoint (and therefore a single `EndpointId` and one UDP socket):

```bash
telesh p2p --expose web=8000 --expose db=5432
telesh connect web -l 8000
telesh connect db  -l 5432
```

Each name is announced as its own directory entry pointing at the same
`EndpointAddr`; the visitor's requested name in the stream preamble selects the
local target. The single positional form (`telesh p2p 8000 -s name`) still
works and defaults the name to the endpoint id.

## 5. Roles

### 5.1 `telesh p2p <port>` / `--expose NAME=PORT` (expose)

1. Load the persisted `SecretKey`.
2. Build one iroh `Endpoint` (ALPN `telesh/p2p/1`); a custom relay
   (`TELESH_RELAY`) and port mapping (`TELESH_NO_PORTMAP`) are env knobs.
3. Open the WebSocket control connection and announce a `P2PAnnounce` per
   service (all sharing the endpoint address). The server replies
   `P2PAnnounceResp` per name; a name already owned by a *different* endpoint is
   rejected (`NameTaken`) and the agent exits loudly instead of silently
   serving under a stale identity.
4. Accept iroh connections: each inbound bi-stream is authenticated (§7), the
   requested service name is resolved to a local port, and bytes are pumped.
5. Keep the control connection alive (keepalive + per-service `Traffic`).

### 5.2 `telesh connect <name> -l <local-port>` (mirror)

1. Open a lookup-only control connection (`Hello` with client kind `Lookup`),
   send `P2PLookup`, and stay subscribed: the server pushes `P2PAddr` whenever
   that name is (re-)announced.
2. A **supervisor** task keeps exactly one live iroh connection to the latest
   address. It reconnects when the connection drops *or* when a *different*
   address is pushed, so an agent that restarts with new addresses is picked up
   **without a re-lookup**. Same-address re-announcements are ignored.
3. Listen on the local port. Each accepted local connection is attached to a
   live iroh bi-stream, retrying while the link recovers — but **only until the
   agent acks** (`STREAM_ACK_OK`), i.e. before any payload is forwarded. After
   the ack we pump and never retry. The wait window is a fixed 30s; a
   definitive refusal (`STREAM_ACK_ERR`) fails immediately.
4. The target is `ACCOUNT/NAME` (e.g. `telesh connect 000012345/web`); the
   account is the service owner's 9-digit handle.
5. `run_connect` returns only if the control subscription itself ends; the
   caller then resubscribes.

### 5.3 HTTP / raw-TCP agents

`client::run_once` additionally builds an iroh endpoint and announces it with
`IrohReady`, re-announcing whenever the address changes (e.g. once a home relay
is connected — observed via `watch_addr()`). This does not block tunnel
establishment.

The server stores the address per `agent_id` in the registry. On ingress
(`ingress::open_stream`), it first tries to dial the agent over iroh; on any
failure (dial error, prelude write) it falls back to the original WebSocket
multiplexed relay path (`open_stream_relay`).

## 6. Server

- **Directory** (`state::P2P_DIR`): `"<account>/<name>" -> P2pEntry { account,
  name, endpoint_id, P2PAnnounce }`. Names are **namespaced per account**, so two
  accounts may both expose `web`. Within an account a name is **owned by its
  `endpoint_id`** (a cryptographic peer identity), claimed atomically: the same
  endpoint may re-claim/replace its own entry (reconnects), a different endpoint
  is rejected rather than overwriting the owner, and a rejected claim is **not**
  pushed to subscribers. Entries are removed when the owning agent's control
  connection drops.
- **Lookup connections** are not registered: `handle_lookup` only answers
  `P2PLookup` and keepalives.
- **Gateway** (`gateway.rs`): the server binds its own iroh endpoint (ephemeral
  identity, port mapping disabled) and dials agents. Disable with
  `TELESH_NO_IROH=1`.
- **Embedded relay** (`--relay-serve <addr>`): runs an in-process iroh relay
  so a self-hosted deployment needs no external relay. Plain HTTP by default;
  TLS and ACME/Let's Encrypt come from the relay config file
  (`$TELESH_RELAY_CONFIG`, default `~/.telesh/relay.toml`), which enables HTTPS
  and the QUIC address-discovery port. Agents point at the relay with
  `TELESH_RELAY=http(s)://<host>:<port>`.

## 7. Security

- iroh connections are encrypted and the peer `EndpointId` is authenticated by
  QUIC/TLS. The `EndpointId` alone does not grant access, so the agent applies
  an application-level check.
- **Stream preamble**: the dialer (a peer or the server edge) sends a tagged
  credential as the first bytes of every stream:
  - kind 0, shared secret: `[0][u16 len][secret][u16 service_len][service]`
  - kind 1, access token: `[1][u16 len][token][u16 service_len][service]`
  The agent checks a kind-0 secret locally, and a kind-1 token by asking the
  server (`TokenCheck`). With no secret configured the agent accepts kind 0,
  matching the control plane.
- **Stream ack**: the acceptor replies with one byte after the preamble and the
  local dial: `STREAM_ACK_OK` (payload may flow) or `STREAM_ACK_ERR` (definitive
  refusal). This is the commit point for the dialer's pre-payload retry.
- **Opaque access tokens, checked on-line**: the peer gets a short-lived token
  from `P2PLookup` and presents it on the stream preamble (kind 1). The agent
  cannot verify it, so it asks the server (`TokenCheck` → `TokenCheckResp`); the
  server stores only the token hash, scoped to `account_id` + `name`. **No
  server key and no shared secret are needed on the data plane.** Trade-off:
  the server must be reachable to validate — the token is a grant, not an
  offline capability. A separate `--lookup-secret` (or the master secret) gates
  who may look up in the first place.
- The preamble is application-level authorization, not transport encryption;
  it is distinct from the control-plane `auth_key` exchange.

## 8. Port mapping

Two distinct things:

1. **iroh portmapper** (default on): maps the endpoint's UDP port via
   UPnP/PCP/NAT-PMP to improve direct connectivity. `TELESH_NO_PORTMAP=1`
   disables it (avoids SSDP probing / firewall prompts).
2. **`telesh p2p --map-tcp`**: asks the local gateway to map the exposed TCP
   port, so the service is publicly reachable *without* iroh or the server. The
   resulting `IP:port` is logged and reported via `TcpMapped`. Fails on CGNAT,
   disabled UPnP, etc., in which case the tunnel continues over iroh.

`portmapper` is used directly for (2); iroh uses it internally for (1). Note
iroh's built-in portmapper only maps its own UDP port, not arbitrary TCP ports.

## 9. Metrics

The server cannot observe pure P2P payload, so the agent reports it:

- Every P2P stream is counted in the agent (`P2P_BYTES_SENT` /
  `P2P_BYTES_RECV`).
- `run_expose` sends a `Traffic` report every 10s with the deltas.
- The server records them as `p2p:<name>` in the traffic counters, visible in
  the dashboard and `/metrics`.
- For the HTTP/TCP iroh gateway path the server pumps the bytes itself and
  counts them under the usual `http:<sub>` / `tcp:<port>` keys.

## 10. Configuration

Agent (global flags):

| Flag | Env | Meaning |
|---|---|---|
| `--state-file` | `TELESH_STATE` | identity file (iroh key + cached session) |
| `--enroll <s>` | `TELESH_ENROLLMENT_SECRET` | enrollment secret for first registration |
| `--no-reconnect` | | exit instead of reconnecting |
| `--dashboard-addr` | `TELESH_INSPECTOR_ADDR` | local inspector address |

Agent env-only knobs: `TELESH_RELAY` (custom iroh relay; default n0),
`TELESH_NO_PORTMAP=1` (disable UDP gateway probing).

`telesh p2p`: `<PORT>` or `--expose NAME=PORT` (repeatable), `-s/--name`,
`--map-tcp`.
`telesh connect`: `<ACCOUNT/NAME>`, `-l/--local-port`.

Server:

| Flag | Env | Meaning |
|---|---|---|
| `--secret <s>` | `TELESH_SECRET` | master secret (token signing + lookup auth) |
| `--lookup-secret <s>` | `TELESH_LOOKUP_SECRET` | lower-privilege secret for lookups |
| `--db <path>` | `TELESH_DB` | accounts/sessions SQLite (default `~/.telesh/server.sqlite`) |
| `--enrollment-secret <s>` | `TELESH_ENROLLMENT_SECRET` | gate first registration |
| `--relay-serve <addr>` | `TELESH_RELAY_SERVE` | run an embedded iroh relay |

Server env-only knobs: `TELESH_RELAY` (relay the gateway dials through),
`TELESH_NO_IROH=1` (disable the direct data plane).

Embedded-relay TLS/ACME come from the relay config file (`$TELESH_RELAY_CONFIG`,
default `~/.telesh/relay.toml`): `tls_cert`, `tls_key`, `https_addr`,
`quic_addr`, `acme`, `acme_email`, `acme_staging`, `acme_cache`.

## 11. Limitations / future work

- The embedded relay supports manual PEM TLS and ACME/Let's Encrypt
  (TLS-ALPN-01 on the HTTPS listener); no other certificate modes yet.
- Access tokens are opaque and server-verified, so the server must be reachable
  for every data-plane stream. They are grants, not per-peer identities.
- An accepted local connection is retried across a P2P reconnect only before
  the stream ack; once payload has flowed there is no replay (it would duplicate
  data on the peer). Connections still queued when the 30s window expires are
  dropped.
- Service-name ownership holds only while the endpoint is connected. When it
  disconnects the name is freed, so another peer holding the master secret could
  claim it. Persistent `name -> endpoint_id` reservations would close that gap;
  a per-agent credential instead of one shared secret would close it properly.
- The control subscription is the only thing that forces a re-lookup: if the
  visitor's WebSocket drops, `run_connect` returns and the caller resubscribes.
  Address changes alone never trigger one.
- A local connection that arrives during the brief reconnect window waits up to
  `CONNECT_TIMEOUT` for a live connection, then gives up.
- MSRV is 1.91 (iroh 1.2).