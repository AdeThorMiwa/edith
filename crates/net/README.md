# filmcraft-net

The one place FilmCraft reaches the internet and holds credentials (L2; roadmap F02 in
[docs/upcoming-features.md](../../docs/upcoming-features.md)).

- `NetState` / `shared()`: the global Offline switch and the request log (500 entries, URLs reduced
  to scheme, host and path; errors scrubbed).
- `redact`: `url`, `host`, `message`. Anything shown or logged goes through it.
- Features, all off by default and never enabled for wasm: `http` (client), `keychain` (OS secret
  store), `oauth` (PKCE and the loopback redirect receiver).

Secrets never enter a project file, a log, a command result or a screenshot. Commands can ask
whether a secret exists, never read it back.
