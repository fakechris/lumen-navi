# Optional MIT cua-driver binary (not in git)

Lumen Cua embeds [cua-driver](https://github.com/trycua/cua) **MIT only** as
an Act engine. Observe never uses it. Do not fetch `cua-agent[omni]` (AGPL).

`scripts/macos/prepare-cua-app.sh` copies `cua-driver` into
`Lumen Cua.app/Contents/Helpers/` from, in order:

1. `$CUA_DRIVER_BIN`
2. `apps/cua/vendor/cua-driver` (this directory)
3. `$CUA_DRIVER_FETCH=1` (default on) — GitHub release `cua-driver-rs-v$CUA_DRIVER_VERSION`

Fetch is pinned: `checksums-$CUA_DRIVER_VERSION.txt` in this directory must
list the SHA-256 of `cua-driver-rs-$VERSION-darwin-universal-binary.tar.gz`.
A mismatch fails the helper build. Override with `$CUA_DRIVER_SHA256`.

The helper then signs the nested binary with the same identity and flags as
`lumen-cua` (no Hardened Runtime unless the host has matching entitlements)
and spawns `cua-driver serve --embedded` as a child, so TCC stays on
`com.lumenopen.cua`.

On ensure, Lumen Cua writes `driver-policy.yaml` (background-only input)
and `driver-managed-policy.rego` (block browser `⌘L`) next to the private
socket. Agents should attach as MCP server `computer-use`:

```
cua-driver mcp --embedded --socket "$HOME/Library/Application Support/Lumen/Cua/run/driver.sock"
```
