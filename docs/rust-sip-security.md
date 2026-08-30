# Rust SIP source-address policy

`sip-security` provides the offline-verifiable source-address guard for the
Rust SIP/RTP edge. It evaluates the observed socket peer (`IpAddr` or
`SocketAddr`), never an address copied from a SIP header.

## Policy behavior

- CIDRs are parsed without external dependencies and canonicalized by masking
  host bits.
- IPv4 and IPv6 networks are kept separate; an address from the other family
  never matches.
- Deny rules are checked first, so a deny always wins over an allow.
- With no allowlist configured, sources are allowed by default unless denied.
- Once an allowlist is configured, including an explicitly empty list, only
  sources matching an allow rule are accepted.
- `clear_allowlist` removes the explicit boundary and restores the default
  allow behavior (subject to deny rules).

Both lists have independent, non-zero entry bounds. Duplicate canonical
networks are rejected, including equivalent inputs such as `192.0.2.1/24`
and `192.0.2.0/24`. The bounded vectors prevent a configuration reload from
retaining an unbounded number of rules.

## Example

```rust
use std::net::IpAddr;
use sip_security::SourceIpPolicy;

let mut policy = SourceIpPolicy::default();
policy.add_allow("2001:db8::/32")?;
policy.add_deny("2001:db8:bad::/48")?;

assert!(policy.allows("2001:db8::10".parse::<IpAddr>()?));
assert!(!policy.allows("2001:db8:bad::10".parse::<IpAddr>()?));
# Ok::<(), Box<dyn std::error::Error>>(())
```

This crate does not change the active Asterisk route. Wiring the policy into a
live listener requires provider/runtime evidence and a rollout decision; until
then Asterisk remains the compatibility fallback.
