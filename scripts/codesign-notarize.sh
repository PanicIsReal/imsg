#!/usr/bin/env bash
set -euo pipefail

dry=0
app=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry=1 ;;
    --) ;;
    *)
      if [[ -z "$app" && "$arg" != --* ]]; then
        app="$arg"
      fi
      ;;
  esac
done

if [[ "${APPLE_SKIP:-}" == "1" ]]; then
  echo "unsigned build"
  exit 0
fi

has_p12=0
if [[ -n "${DEVELOPER_ID_CERT_P12:-}" ]]; then
  has_p12=1
fi

identity=""
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Developer ID Application"; then
  identity="$(security find-identity -v -p codesigning | awk -F'\"' '/Developer ID Application/{print $2; exit}')"
fi

if [[ "$has_p12" -eq 0 && -z "$identity" ]]; then
  echo "unsigned build"
  exit 0
fi

if [[ "$dry" -eq 1 ]]; then
  echo "dry-run: would codesign and notarize ${app:-Imsg Setup.app}"
  exit 0
fi

if [[ -z "$app" ]]; then
  app="${1:-dist/imsg-macos-aarch64/Imsg Setup.app}"
fi
if [[ ! -e "$app" ]]; then
  echo "unsigned build"
  exit 0
fi

if [[ "$has_p12" -eq 1 && -z "$identity" ]]; then
  pass="${DEVELOPER_ID_CERT_PASSWORD:-}"
  keychain="${RUNNER_TEMP:-/tmp}/imsg-sign.keychain-db"
  security create-keychain -p "" "$keychain" || true
  security unlock-keychain -p "" "$keychain"
  if [[ -f "$DEVELOPER_ID_CERT_P12" ]]; then
    security import "$DEVELOPER_ID_CERT_P12" -k "$keychain" -P "$pass" -T /usr/bin/codesign -T /usr/bin/security
  else
    tmp="$(mktemp)"
    printf '%s' "$DEVELOPER_ID_CERT_P12" | base64 --decode > "$tmp"
    security import "$tmp" -k "$keychain" -P "$pass" -T /usr/bin/codesign -T /usr/bin/security
    rm -f "$tmp"
  fi
  identity="$(security find-identity -v -p codesigning "$keychain" | awk -F'\"' '/Developer ID Application/{print $2; exit}')"
fi

if [[ -z "$identity" ]]; then
  echo "unsigned build"
  exit 0
fi

codesign --force --options runtime --sign "$identity" --timestamp "$app"

if [[ -z "${NOTARY_APPLE_ID:-}" && -z "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  echo "unsigned build"
  exit 0
fi

zip="$(mktemp -t imsg-app).zip"
ditto -c -k --keepParent "$app" "$zip"
if [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  xcrun notarytool submit "$zip" --keychain-profile "$NOTARY_KEYCHAIN_PROFILE" --wait
else
  xcrun notarytool submit "$zip" \
    --apple-id "${NOTARY_APPLE_ID}" \
    --team-id "${NOTARY_TEAM_ID}" \
    --password "${NOTARY_PASSWORD}" \
    --wait
fi
xcrun stapler staple "$app"
rm -f "$zip"
echo "signed and notarized $app"
