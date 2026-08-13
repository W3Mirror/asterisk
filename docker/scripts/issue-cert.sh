#!/usr/bin/env bash
#
# issue-cert.sh - obtain (or renew) the Let's Encrypt certificate for the
# public Meta SIP-TLS trunk hostname (sip-trunk.w3.run) and install it
# where docker/etc-asterisk/pjsip.conf's [transport-tls] expects it.
#
# WHEN TO RUN THIS:
#   - Once, by hand, after DNS for sip-trunk.w3.run is live and pointing
#     at THIS host's public IP (see the runbook / `curl -4 -s ifconfig.me`)
#     -- until then certbot's HTTP-01 challenge cannot reach this host and
#     the command below fails loudly (that's fine, it means DNS isn't
#     ready yet, not that anything else is broken).
#   - Automatically thereafter, via certbot's own systemd timer
#     (certbot.timer, installed with the certbot package) calling `certbot
#     renew`, which in turn calls THIS script again through the
#     --deploy-hook wired up on first successful issuance below. You do
#     not need to add your own cron job for renewal -- certbot already
#     has one; this script is what makes a renewal actually reach
#     Asterisk instead of just refreshing files under /etc/letsencrypt.
#
# WHAT IT DOES:
#   1. Runs `certbot certonly --standalone` for sip-trunk.w3.run. Standalone
#      means certbot itself briefly binds :80 to answer the HTTP-01
#      challenge -- this only works because nothing else on the host is
#      listening on :80 (verified before implementing this: `ss -ltnp`
#      showed it free; Caddy only publishes 7231, not 80). If that ever
#      changes, switch to --webroot or a DNS-01 plugin instead.
#   2. Copies /etc/letsencrypt/live/sip-trunk.w3.run/{fullchain,privkey}.pem
#      into docker/etc-asterisk/secrets/tls/ (gitignored -- see
#      secrets/.gitignore), replacing the self-signed placeholder the
#      first time this runs.
#   3. Fixes ownership/permissions so the asterisk container (runs as
#      uid 999, no host UID mapping) can read them through the read-only
#      bind mount -- matching the existing world-readable convention used
#      for the other files under secrets/ (pjsip_auth.conf, ari_secret.conf).
#   4. Reloads res_pjsip.so. NOTE: this does NOT require allow_reload=yes
#      on [transport-tls] -- Asterisk 22's PJSIP transport reload logic
#      (res/res_pjsip/config_transport.c) specifically detects a changed
#      cert/key file's mtime even when the transport config text (the
#      file *paths*) is unchanged, and restarts just that TLS transport
#      in place (pjsip_tls_transport_restart()) rather than requiring a
#      full config-level reload of the transport object.
#
# Usage:
#   sudo docker/scripts/issue-cert.sh              # issue or renew + install
#   sudo docker/scripts/issue-cert.sh --deploy-only # skip certbot, just
#                                                    # (re)install whatever
#                                                    # is currently in
#                                                    # /etc/letsencrypt/live
#                                                    # and reload -- used
#                                                    # as the certbot
#                                                    # deploy-hook itself
set -euo pipefail

DOMAIN="sip-trunk.w3.run"
EMAIL="research@w3dev.email"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST_DIR="$REPO_ROOT/docker/etc-asterisk/secrets/tls"
LE_LIVE="/etc/letsencrypt/live/$DOMAIN"

log() { echo "[issue-cert] $*" >&2; }

require_root() {
	if [ "$(id -u)" != "0" ]; then
		echo "must run as root (certbot needs :80, and we chown/chmod the installed cert)" >&2
		exit 1
	fi
}

install_from_live() {
	if [ ! -f "$LE_LIVE/fullchain.pem" ] || [ ! -f "$LE_LIVE/privkey.pem" ]; then
		echo "[issue-cert] ERROR: $LE_LIVE does not contain fullchain.pem/privkey.pem -- certbot has not issued a cert for $DOMAIN yet" >&2
		exit 1
	fi

	mkdir -p "$DEST_DIR"
	install -o root -g root -m 0644 "$LE_LIVE/fullchain.pem" "$DEST_DIR/fullchain.pem"
	install -o root -g root -m 0644 "$LE_LIVE/privkey.pem" "$DEST_DIR/privkey.pem"
	log "installed cert+key into $DEST_DIR (mode 0644, matches secrets/*.conf convention -- readable by the container's uid 999 through the read-only bind mount)"

	# Reload PJSIP so the running [transport-tls] picks up the new
	# cert/key bytes (see the module comment above re: why allow_reload
	# is not needed for this specific case).
	if command -v docker >/dev/null 2>&1 && (cd "$REPO_ROOT" && docker compose ps asterisk 2>/dev/null | grep -q Up); then
		(cd "$REPO_ROOT" && docker compose exec -T asterisk asterisk -rx "module reload res_pjsip.so")
		log "reloaded res_pjsip.so"
		(cd "$REPO_ROOT" && docker compose exec -T asterisk asterisk -rx "pjsip show transports")
	else
		log "WARNING: asterisk container not running via docker compose in $REPO_ROOT -- cert installed but NOT reloaded into a live process"
	fi
}

issue_or_renew() {
	if ! command -v certbot >/dev/null 2>&1; then
		log "certbot not found, installing..."
		apt-get update -qq
		apt-get install -y certbot
	fi

	if [ -d "$LE_LIVE" ]; then
		log "existing cert found for $DOMAIN, renewing (no-op if not due yet)"
		certbot renew --cert-name "$DOMAIN" --non-interactive
	else
		log "no existing cert for $DOMAIN, requesting a new one via HTTP-01 (--standalone, needs :80 free)"
		# --deploy-hook registers this script (in --deploy-only mode) to
		# run automatically on every future `certbot renew` success --
		# this is what makes certbot's own renewal timer (systemctl list-timers
		# certbot.timer) propagate to Asterisk without any separate cron job.
		certbot certonly --standalone \
			-d "$DOMAIN" \
			--non-interactive --agree-tos -m "$EMAIL" \
			--deploy-hook "$REPO_ROOT/docker/scripts/issue-cert.sh --deploy-only"
	fi
}

main() {
	require_root
	if [ "${1:-}" = "--deploy-only" ]; then
		install_from_live
		return
	fi
	issue_or_renew
	install_from_live
}

main "$@"
