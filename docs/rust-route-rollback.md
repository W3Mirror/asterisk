# Rust route activation and rollback

`provider-routing::RouteController` is the configuration-level safety boundary
for moving provider traffic from Asterisk to the Rust engine. It wraps the
validated, credential-free `ProviderRouteTable` and starts every process in a
fail-closed Asterisk state, even when a profile is configured with Rust as its
primary target.

## Activation contract

- A deployment supervisor must call `activate_rust` with the controller's
  current generation.
- Activation is rejected when no profile explicitly targets Rust.
- The returned `RouteTransition` records the previous target, new target, and
  generation. A repeated activation at the current generation is idempotent.
- Unmatched/default routes remain on Asterisk; activation only changes matched
  profiles whose primary target is Rust.
- Every profile's fallback is validated as Asterisk, and the route table never
  stores credentials.

## Rollback contract

`rollback_to_asterisk` uses the same compare-and-swap generation check. A stale
deployment operation is rejected without changing state, while a current
rollback changes the active target and advances the generation. Repeating a
rollback is idempotent.

Rollback does not delete the provider table, call records, or authentication
context. It prevents new route-controller origination from resolving a provider
or mutating call state while the runtime remains on the Asterisk fallback.
Existing Rust calls are not force-terminated by this configuration switch;
drain/restart and call termination are separate lifecycle operations.

`CallRuntime::originate_with_route_controller` evaluates the controller before
transport compatibility checks, address resolution, or engine mutation. This
keeps an Asterisk rollback atomic and makes the no-resolution/no-mutation
property testable offline.

The controller is an offline migration safeguard. Provider credentials,
sanitized interoperability captures, production deployment, live calls, and
permission to enable Rust traffic remain external rollout gates. Until those
gates are approved, signaling, media, and call routing stay on Asterisk.
