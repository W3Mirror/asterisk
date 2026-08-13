#!/usr/bin/env bash
#
# meta-allowlist.sh - restrict the public Meta SIP-TLS trunk (compose.yml's
# asterisk service, "5061:5061/tcp" + "10000-10100:10000-10100/udp") to
# Meta's own egress ranges (AS32934), at the HOST firewall level.
#
# WHY THIS EXISTS: Docker's published-port DNAT happens in the `nat`
# table's PREROUTING chain, ahead of the host firewall's INPUT chain --
# ufw (default deny incoming) does NOT see or filter this traffic at all
# (see docs-internal/operations/lessons-learned.mdx). The chain that DOES
# see it, after DNAT, is `DOCKER-USER` in the `filter` table -- Docker
# guarantees it never overwrites rules placed there itself. That's where
# this script installs an allowlist.
#
# WHAT IT DOES, each run (safe to re-run any time -- fully idempotent):
#   1. Fetches AS32934's currently-announced IPv4 prefixes from RADB via
#      whois (rotates/grows over time -- this is why it's a script and
#      not a hardcoded list).
#   2. Bulk-loads them into an ipset (hash:net) named meta-wa-v4, via a
#      swap so there's never a window with an empty/partial set live.
#      ~1000 prefixes as of writing -- ipset is what makes matching that
#      many ranges per-packet cheap; 1000 individual iptables -A rules
#      would not be.
#   3. Ensures a dedicated `META-SIP` chain exists in iptables (filter
#      table) with rules referencing that ipset, and that DOCKER-USER
#      jumps to it. The chain's own rules are only written once (checked
#      with `-C` before `-A`) -- refreshing just swaps ipset membership,
#      it does NOT touch the iptables rules themselves, so counters and
#      rule order are stable across refreshes.
#
# IPv6: AS32934 also announces IPv6 (route6: objects), but this host's
# ip6tables is non-functional ("Incompatible with this kernel" -- nf_tables
# backend mismatch) and the `aistack` Docker network has IPv6 disabled, so
# nothing is published on IPv6 for this service in the first place (no
# gap to allowlist). If that ever changes, extend this script with the
# same pattern using `ip6tables`/`ipset ... inet6` and AS32934's route6:
# objects.
#
# PERSISTENCE: iptables/ipset state does NOT survive a reboot on its own.
# Rather than snapshot-and-restore (which would freeze today's AS32934
# ranges, going stale the moment they rotate), this is re-run from cron
# on @reboot as well as on a weekly timer -- see the crontab entries this
# script's own install step (below) or the runbook documents. Re-running
# from scratch after boot is simpler and self-healing compared to
# maintaining a separate iptables-persistent/netfilter-persistent save
# file that duplicates this logic.
#
# Usage: sudo docker/scripts/meta-allowlist.sh
# Verify: iptables -L META-SIP -n -v | head
set -euo pipefail

SIP_PORT=5061
RTP_PORTS="10000:10100"
CHAIN="META-SIP"
SET_V4="meta-wa-v4"
SET_V4_TMP="meta-wa-v4-tmp"
WHOIS_HOST="whois.radb.net"
ASN="AS32934"

log() { echo "[meta-allowlist] $*" >&2; }

require_root() {
	if [ "$(id -u)" != "0" ]; then
		echo "must run as root (iptables/ipset)" >&2
		exit 1
	fi
}

require_cmds() {
	for c in whois ipset iptables; do
		command -v "$c" >/dev/null 2>&1 || { echo "missing required command: $c" >&2; exit 1; }
	done
}

# --- 1. Fetch AS32934's IPv4 prefixes -------------------------------------
fetch_prefixes_v4() {
	whois -h "$WHOIS_HOST" -- "-i origin $ASN" 2>/dev/null \
		| awk '/^route:/ {print $2}' \
		| sort -u
}

# --- 2. Load into an ipset via swap (no downtime, no partial-set window) --
refresh_ipset() {
	local prefixes
	prefixes="$(fetch_prefixes_v4)"
	local count
	count=$(echo "$prefixes" | grep -c . || true)

	if [ "$count" -lt 10 ]; then
		log "WARNING: whois returned only $count prefixes for $ASN -- looks wrong (expected ~600+), leaving existing ipset ($SET_V4) untouched"
		return 1
	fi
	log "fetched $count IPv4 prefixes for $ASN"

	ipset create "$SET_V4" hash:net family inet -exist
	ipset create "$SET_V4_TMP" hash:net family inet -exist
	ipset flush "$SET_V4_TMP"

	{
		echo "create $SET_V4_TMP hash:net family inet -exist"
		while IFS= read -r prefix; do
			[ -n "$prefix" ] && echo "add $SET_V4_TMP $prefix -exist"
		done <<<"$prefixes"
	} | ipset restore -exist

	ipset swap "$SET_V4" "$SET_V4_TMP"
	ipset destroy "$SET_V4_TMP"
	log "swapped $count prefixes into ipset '$SET_V4'"
}

# --- 3. Ensure the META-SIP chain + rules + DOCKER-USER jump exist --------
ensure_chain_and_rules() {
	iptables -N "$CHAIN" 2>/dev/null || true

	# Order matters: ACCEPT the allowlisted source for the trunk's ports
	# first, then DROP everything else destined for those same ports,
	# then RETURN so all other DOCKER-USER-evaluated traffic (every other
	# published port, inter-container forwarding, etc.) is completely
	# unaffected and falls through to Docker's normal rules.
	iptables -C "$CHAIN" -p tcp --dport "$SIP_PORT" -m set --match-set "$SET_V4" src -j ACCEPT 2>/dev/null \
		|| iptables -A "$CHAIN" -p tcp --dport "$SIP_PORT" -m set --match-set "$SET_V4" src -j ACCEPT

	iptables -C "$CHAIN" -p udp --dport "$RTP_PORTS" -m set --match-set "$SET_V4" src -j ACCEPT 2>/dev/null \
		|| iptables -A "$CHAIN" -p udp --dport "$RTP_PORTS" -m set --match-set "$SET_V4" src -j ACCEPT

	iptables -C "$CHAIN" -p tcp --dport "$SIP_PORT" -j DROP 2>/dev/null \
		|| iptables -A "$CHAIN" -p tcp --dport "$SIP_PORT" -j DROP

	iptables -C "$CHAIN" -p udp --dport "$RTP_PORTS" -j DROP 2>/dev/null \
		|| iptables -A "$CHAIN" -p udp --dport "$RTP_PORTS" -j DROP

	iptables -C "$CHAIN" -j RETURN 2>/dev/null \
		|| iptables -A "$CHAIN" -j RETURN

	# Jump from DOCKER-USER into META-SIP, at the top (position 1) so it
	# runs before Docker's own DOCKER-USER RETURN. Only insert once.
	iptables -C DOCKER-USER -j "$CHAIN" 2>/dev/null \
		|| iptables -I DOCKER-USER 1 -j "$CHAIN"

	log "META-SIP chain + DOCKER-USER jump in place"
}

main() {
	require_root
	require_cmds
	refresh_ipset
	ensure_chain_and_rules
	log "done. iptables -L $CHAIN -n -v to inspect; ipset list $SET_V4 | head to inspect membership."
}

main "$@"
