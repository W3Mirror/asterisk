# Rust provider-route configuration reload

`provider-routing::RouteController::replace_table` is the configuration
reload boundary for provider routes during the Asterisk-to-Rust migration. A
reload is an atomic, generation-checked table replacement. It is not an
implicit Rust rollout.

## Reload contract

- The caller supplies the generation it observed. A stale generation is
  rejected without changing the active table or route target.
- A changed table is installed atomically, advances the generation, and
  returns all routes to the fail-closed Asterisk target.
- A byte-for-byte identical table is idempotent: the table, generation, and
  active target remain unchanged.
- A generation at `u64::MAX` cannot be advanced. The replacement is rejected
  and the existing state remains intact.
- After a changed reload, a deployment supervisor must explicitly call
  `activate_rust` with the returned generation. Until that succeeds, even a
  profile configured with Rust as its primary target resolves to Asterisk.

This sequencing prevents a configuration update from silently enabling new
provider traffic. It also gives the supervisor a durable compare-and-swap
point at which it can validate the new table before reactivating Rust.

## Runtime boundary

`CallRuntime::originate_with_route_controller` evaluates the route controller
before provider resolution, transport checks, or call-engine mutation. A
reloaded table that is still on Asterisk therefore rejects Rust-only runtime
origination without resolving a host or creating a call. Existing calls are
not force-terminated by a configuration reload; drain/restart and terminal
cleanup remain separate lifecycle operations.

The route table remains credential-free and every configured fallback remains
Asterisk. Provider credentials, sanitized interoperability captures,
production deployment, live calls, and permission to enable Rust traffic are
separate rollout gates. Until those gates are approved, signaling, media, and
call routing stay on Asterisk.
