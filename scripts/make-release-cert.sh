#!/usr/bin/env bash
# Makes the certificate the release workflow signs Tilde with, once.
#
# There's no Apple Developer ID, so releases are signed with a self-made certificate. It
# isn't trusted by Gatekeeper (the first launch still needs allowing), but it gives every
# release the same identity, so Full Disk Access and the other permissions carry over
# when Tilde is updated. Ad-hoc signing would lose them on every update.
#
#   scripts/make-release-cert.sh            writes the certificate and prints what to do
#   scripts/make-release-cert.sh --upload   also stores it as GitHub Actions secrets (gh)
#
# Keep the .p12 and its password safe: a new certificate means everyone grants the
# permissions again. Anyone holding it can sign apps that Tilde's permissions apply to.
set -euo pipefail

NAME="Tilde Release Signing"
DIR="$HOME/Library/Application Support/Tilde/release-signing"
P12="$DIR/tilde-release.p12"
PASSWORD_FILE="$DIR/password"

if [[ -f "$P12" ]]; then
  echo "The certificate already exists: $P12"
else
  mkdir -p "$DIR"
  chmod 700 "$DIR"
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  password="$(/usr/bin/openssl rand -hex 24)"
  (umask 077 && printf '%s' "$password" > "$PASSWORD_FILE")
  cat > "$work/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
  # 20 years: the identity has to outlive every version people have installed.
  /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \
    -keyout "$work/key.pem" -out "$work/cert.pem" -config "$work/cert.cnf" 2>/dev/null
  (umask 077 && /usr/bin/openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$P12" -passout "pass:$password" -name "$NAME")
  echo "Made $P12"
fi

if [[ "${1:-}" == --upload ]]; then
  base64 -i "$P12" | gh secret set TILDE_SIGNING_P12
  gh secret set TILDE_SIGNING_PASSWORD < "$PASSWORD_FILE"
  echo "Stored as the TILDE_SIGNING_P12 and TILDE_SIGNING_PASSWORD secrets."
else
  echo
  echo "Store it as GitHub Actions secrets with:  scripts/make-release-cert.sh --upload"
  echo "Back up $DIR somewhere safe (a password manager)."
fi
