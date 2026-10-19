# Self-hosting rooms

CFVirtual serves an authoritative WebSocket room backend, either embedded in
`room create` or separately through `serve`. It exposes no web frontend, account
system, or built-in TLS. Clients connect at `/` or `/ws` using protocol v1; see
[PROTOCOL.md](PROTOCOL.md) for the wire schema and limits.

Angle-bracket addresses/codes below are placeholders. `your_handle` always means
your own public Codeforces handle. The server needs outbound HTTPS access to the
public Codeforces API for profile validation, selection, and submission polling.

## LAN: embedded host

On the host:

```sh
cfvirtual room create --handle your_handle --listen 0.0.0.0 --port 7331 --problems 5 --duration 120
```

The host stays in the lobby until `Enter` starts the contest. Share the displayed
room code and a reachable LAN address. Binding `0.0.0.0` exposes port 7331 on all
IPv4 interfaces; it is not an address a client should use to connect. Permit that
TCP port in the host firewall for the intended network.

On another terminal/machine:

```sh
cfvirtual join --server 'ws://<lan-host>:7331' --code '<room-code>' --handle your_handle
```

Add `--spectate` to observe. Players join only before start; spectators may join
afterward. The host chooses the problem set and can add `--bots N`. Room defaults
are five problems, 120 minutes, zero bots, and hidden rating/tag metadata.

An optional `--password '<room-password>'` on room creation adds a soft gate.
Joiners are prompted when a password is required. Avoid putting a real password
in shared scripts, terminal recordings, or shell history.

## Separate headless server

For a backend on the same machine as a proxy or host client:

```sh
cfvirtual serve --listen 127.0.0.1:7331
cfvirtual room create --server ws://127.0.0.1:7331 --handle your_handle --bots 3
```

Run these in separate terminals. `serve` provides the room service; it does not
itself create a contest or start its clock. Remote `room create --server` prepares
the contest on that server and attaches the host to its lobby. Host settings are
authoritative; joining clients cannot replace them.

For a LAN-accessible headless service use `--listen 0.0.0.0:7331`, then connect
with its reachable address. The `serve --listen` argument is a socket address;
embedded `room create` uses a separate address and `--port`.

## Remote access with a TLS reverse proxy

Keep CFVirtual bound to loopback. Terminate TLS at a WebSocket-capable proxy and
forward to `127.0.0.1:7331`. For example, this Caddy configuration uses the
reserved illustrative domain `contest.example.org`; replace it with a domain
you control and configure its DNS:

```caddyfile
contest.example.org {
    reverse_proxy 127.0.0.1:7331
}
```

Caddy handles WebSocket upgrades. Use a valid certificate and allow inbound TLS
traffic to the proxy. With another proxy, preserve upgrade headers and allow
long-lived connections rather than short HTTP request timeouts.

```sh
cfvirtual serve --listen 127.0.0.1:7331 --public-url 'wss://<public-host>'
cfvirtual room create --server 'wss://<public-host>' --handle your_handle
cfvirtual join --server 'wss://<public-host>' --code '<room-code>' --handle your_handle
```

`--public-url` is an advertised client address only. It does not install a proxy,
create certificates, bind the advertised host, or configure port forwarding.

## Temporary cloudflared tunnel

With `cloudflared` installed separately, keep the server local and run:

```sh
cfvirtual serve --listen 127.0.0.1:7331
```

In another terminal:

```sh
cloudflared tunnel --url http://127.0.0.1:7331
```

Use the hostname printed by the tunnel, replacing its `https://` scheme with
`wss://`. Then create the room through it:

```sh
cfvirtual room create --server 'wss://<tunnel-host>' --handle your_handle
cfvirtual join --server 'wss://<tunnel-host>' --code '<room-code>' --handle your_handle
```

Leave both processes running. A temporary tunnel URL can change on restart and
does not promise availability. A named tunnel is an operator-managed alternative;
CFVirtual never needs tunnel-account credentials. Proxy/tunnel operators are
part of the transport trust boundary.

## Trust, data, and operation

- Anyone can name a public Codeforces handle; the server validates existence,
  not ownership. Rooms provide no anti-cheat guarantee.
- Room codes are routing identifiers. Passwords are an in-memory soft gate,
  stored as salted SHA-256 hashes, not strong account authentication.
- Reconnect tokens are private bearer credentials held in the client session.
  Do not log or share them. Use `wss://` outside trusted LANs because `ws://`
  exposes traffic, including handshakes, on the network.
- The process-wide API limit is at least two seconds per request; each room
  nominally polls humans every 20 seconds. More rooms/players and API backoff can
  increase latency. Bots and spectators do not cause API lookups during polling.
- At the deadline the room enters finalization. Two extra polling passes collect
  delayed verdicts on in-window submissions, bounded to 180 seconds after an
  in-flight call. Keep the backend running until final results arrive.
- Per room: 32 players including the host, eight spectators, and 16 bots. The
  instance retains at most 128 rooms. Frames/messages are limited to 64 KiB and
  send queues to 32 messages. Disconnected participants keep their slots.
- Run the backend under a dedicated service environment with writable application
  data storage. On Linux, use `XDG_DATA_HOME` to choose the parent data directory;
  CFVirtual stores `cfvirtual/cfvirtual.db` and `cfvirtual/problemset.json` there.
  Back up SQLite consistently (stop the process or use SQLite's backup facility;
  an active database can have WAL sidecars).
- Contest snapshots persist locally; active rooms, passwords, and reconnect
  sessions do not survive a restart. Restarting also clears retained room slots.
  There is no built-in daemon installation, multi-instance shared room state, or
  database-driven room resurrection.

## Troubleshooting

| Symptom | Check |
|---|---|
| Address already in use | Choose a different `--port` or `serve --listen` socket. |
| Client cannot connect | Reachable host address, bind interface, firewall, proxy upgrade support, and `ws://` versus `wss://`. |
| Room not found after restart | Create a new room; saved results do not restore active sessions. |
| Lobby preparation fails | Server-side API reachability and the public handle; allow for rate-limited preparation. |
| Score update is delayed | API polling/backoff and finalization; submit manually on the selected problem before cutoff. |
| Disconnected after slow output | Retry with `r` while the client retains its session; the server returns a fresh snapshot. |

Do not use a browser's plain HTTP GET as a health endpoint: `/` and `/ws` expect
a WebSocket upgrade, not an HTML page.
