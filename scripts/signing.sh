# Signing Tilde.app with a stable, local certificate. Sourced by install.sh and update.sh.
#
# macOS ties privacy permissions (Full Disk Access, Accessibility, Desktop/Documents/
# Downloads) to an app's signature. An ad-hoc signature is a hash of the build, so every
# build looks like a new app and loses them. Signed with the same certificate, every build
# has the same identity ("this bundle ID, this certificate"), so permissions carry over.
#
# The certificate is self-made and only for this Mac. It lives in its own keychain file
# (~/Library/Keychains/tilde-signing.keychain-db), not the login keychain, so creating and
# using it needs no password prompts. Its password is random and kept, readable only by
# you, in ~/Library/Application Support/Tilde/signing-keychain-password. Anyone who can
# use that key can sign an app Tilde's permissions would apply to, so it stays on this Mac.
#
#   TILDE_SIGN_IDENTITY=<name or hash>  sign with your own identity instead
#   TILDE_ADHOC=1                       sign ad hoc (permissions reset on every install)

SIGN_NAME="Tilde Local Signing"
SIGN_KEYCHAIN="$HOME/Library/Keychains/tilde-signing.keychain-db"
SIGN_PASSWORD_FILE="$HOME/Library/Application Support/Tilde/signing-keychain-password"

# Creates the keychain and certificate the first time.
ensure_signing_identity() {
  if [[ -f "$SIGN_KEYCHAIN" && -f "$SIGN_PASSWORD_FILE" ]]; then
    return
  fi
  echo "Creating a local signing certificate (once) so updates keep their permissions…"
  local work password
  work="$(mktemp -d)"
  mkdir -p "$(dirname "$SIGN_PASSWORD_FILE")"
  password="$(/usr/bin/openssl rand -hex 24)"
  (umask 077 && printf '%s' "$password" > "$SIGN_PASSWORD_FILE")
  cat > "$work/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $SIGN_NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
  /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \
    -keyout "$work/key.pem" -out "$work/cert.pem" -config "$work/cert.cnf" 2>/dev/null
  /usr/bin/openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$work/id.p12" -passout "pass:$password" -name "$SIGN_NAME"
  rm -f "$SIGN_KEYCHAIN"
  security create-keychain -p "$password" "$SIGN_KEYCHAIN"
  # No auto-lock; the scripts unlock it anyway.
  security set-keychain-settings "$SIGN_KEYCHAIN"
  security unlock-keychain -p "$password" "$SIGN_KEYCHAIN"
  security import "$work/id.p12" -k "$SIGN_KEYCHAIN" -P "$password" -T /usr/bin/codesign >/dev/null
  # Lets codesign use the key without asking.
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$SIGN_KEYCHAIN" >/dev/null
  rm -rf "$work"
}

# Signs the app at $1 with bundle ID $2.
sign_app() {
  local app="$1" id="$2"
  if [[ -n "${TILDE_SIGN_IDENTITY:-}" ]]; then
    codesign --force --sign "$TILDE_SIGN_IDENTITY" --identifier "$id" "$app"
    return
  fi
  if [[ "${TILDE_ADHOC:-}" == 1 ]]; then
    codesign --force --sign - --identifier "$id" "$app"
    return
  fi
  ensure_signing_identity
  security unlock-keychain -p "$(cat "$SIGN_PASSWORD_FILE")" "$SIGN_KEYCHAIN"
  # codesign only finds identities in keychains on the search list: add this one for the
  # moment of signing, then put the list back exactly as it was.
  local saved=()
  while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%\"}"
    saved+=("${line#\"}")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "${saved[@]}" "$SIGN_KEYCHAIN"
  local status=0
  codesign --force --sign "$SIGN_NAME" --identifier "$id" "$app" || status=$?
  security list-keychains -d user -s "${saved[@]}"
  return $status
}

# Whether the app at $1 is signed with the local certificate (so its permissions carry
# over to the next build signed the same way).
signed_locally() {
  codesign -d -r- "$1" 2>&1 | grep -q 'certificate leaf'
}
