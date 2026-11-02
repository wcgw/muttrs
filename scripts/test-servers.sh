#!/bin/sh
# Starts, or stops, the mail servers muttrs-mail's server tests run against, in containers:
#
# - GreenMail: IMAP (3143), IMAPS (3993), SMTP (3025), SMTPS (3465), any user and password, XOAUTH2 with any token;
# - Dovecot: IMAP with STARTTLS (31143), IMAPS (31993), submission with STARTTLS (31587), any user with the
#   password "secret"; what it's given to send, it relays to GreenMail.
#
# Both present a certificate for localhost from a CA made here, under target/test-servers. Then:
#
#   scripts/test-servers.sh up
#   MUTTRS_TEST_CA=target/test-servers/ca.pem cargo test -p muttrs-mail --test servers -- --ignored
#   scripts/test-servers.sh down
#
# CONTAINER_ENGINE picks podman (the default) or docker.
set -eu

engine=${CONTAINER_ENGINE:-podman}
dir=$(cd "$(dirname "$0")/.." && pwd)/target/test-servers

certificates() {
	mkdir -p "$dir"
	[ -f "$dir/ca.pem" ] && return
	openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=muttrs test CA" \
		-addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
		-keyout "$dir/ca.key" -out "$dir/ca.pem" 2>/dev/null
	openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
		-keyout "$dir/server.key" -out "$dir/server.csr" 2>/dev/null
	printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n' \
		>"$dir/server.ext"
	openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" -CAcreateserial \
		-days 3650 -extfile "$dir/server.ext" -out "$dir/server.pem" 2>/dev/null
	openssl pkcs12 -export -inkey "$dir/server.key" -in "$dir/server.pem" -name greenmail \
		-passout pass:muttrs -out "$dir/server.p12"
	chmod a+r "$dir"/*
}

# What Dovecot's submission service relays to: GreenMail, on the network the two share.
relay() {
	printf 'submission_relay_host = muttrs-greenmail\nsubmission_relay_port = 3025\n' >"$dir/dovecot-relay.conf"
	chmod a+r "$dir/dovecot-relay.conf"
}

case "${1:-up}" in
up)
	certificates
	relay
	"$engine" rm -f muttrs-greenmail muttrs-dovecot >/dev/null 2>&1 || true
	"$engine" network create muttrs-test >/dev/null 2>&1 || true
	"$engine" run -d --rm --name muttrs-greenmail --network muttrs-test \
		-p 127.0.0.1:3143:3143 -p 127.0.0.1:3993:3993 -p 127.0.0.1:3025:3025 -p 127.0.0.1:3465:3465 \
		-v "$dir:/certs:ro,Z" \
		-e GREENMAIL_OPTS="-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled \
-Dgreenmail.tls.keystore.file=/certs/server.p12 -Dgreenmail.tls.keystore.password=muttrs" \
		docker.io/greenmail/standalone:latest >/dev/null
	"$engine" run -d --rm --name muttrs-dovecot --network muttrs-test \
		-p 127.0.0.1:31143:31143 -p 127.0.0.1:31993:31993 -p 127.0.0.1:31587:31587 \
		-v "$dir/server.pem:/etc/dovecot/ssl/tls.crt:ro,Z" -v "$dir/server.key:/etc/dovecot/ssl/tls.key:ro,Z" \
		-v "$dir/dovecot-relay.conf:/etc/dovecot/conf.d/muttrs-relay.conf:ro,Z" \
		-e USER_PASSWORD=secret \
		docker.io/dovecot/dovecot:latest >/dev/null
	echo "GreenMail and Dovecot are starting; their CA is $dir/ca.pem"
	;;
down)
	"$engine" rm -f muttrs-greenmail muttrs-dovecot >/dev/null
	"$engine" network rm muttrs-test >/dev/null 2>&1 || true
	;;
*)
	echo "usage: $0 [up|down]" >&2
	exit 2
	;;
esac
