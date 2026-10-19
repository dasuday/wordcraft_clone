#!/usr/bin/env bash
# Fetch the draw.io web app (Apache-2.0, https://github.com/jgraph/drawio) into vendor/drawio,
# which is what the diagram editor is served from: no network, no third-party embed site.
#
# Usage: packaging/fetch-drawio.sh [version]        (default: v31.4.2)
#        packaging/fetch-drawio.sh --verify [version]
#
# Sources: the Gitee mirror first (fast from China), GitHub second. Every version is pinned to the
# commit its tag points at: a mirror whose checkout is anything else is thrown away rather than
# packaged, so what ships is the code that was reviewed.
#
# Fetching writes MANIFEST.sha256 beside the files — the sha256 of every one of them. The VERSION
# file only says which tag was asked for, so that is not what packaging trusts: both package
# scripts run `--verify` first and refuse to embed a directory whose files have been edited, added
# or removed since. `--verify` exits non-zero and names what differs.
#
# The app's parts don't all share its license: the web app itself is Apache-2.0, but img/,
# stencils/ and shapes/ carry their own (restricted) notice, templates/ is CC-BY-4.0 and
# js/libavoid-js/ is LGPL-2.1. Each ships unmodified with its LICENSE file, and the script fails
# if one of those files is missing — see the draw.io row in ATTRIBUTION.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/vendor/drawio"
VERIFY=0
if [ "${1:-}" = "--verify" ]; then
  VERIFY=1
  shift
fi
VERSION="${1:-v31.4.2}"
WORK="${TMPDIR:-/tmp}/wordcraft-drawio-$VERSION"

# What the directory was when it was checked: sha256 of everything except the manifest itself.
MANIFEST="MANIFEST.sha256"

write_manifest() {
  ( cd "$DEST" \
    && find . -type f ! -name "$MANIFEST" -exec shasum -a 256 {} + \
    | LC_ALL=C sort -k2 \
    | sed 's|  \./|  |' ) > "$DEST/$MANIFEST"
}

# Does vendor/drawio still hold what was fetched — no file edited, added or removed?
verify_manifest() {
  if [ ! -f "$DEST/$MANIFEST" ]; then
    echo "error: $DEST/$MANIFEST is missing — run packaging/fetch-drawio.sh $VERSION again" >&2
    return 1
  fi
  bad="$( cd "$DEST" && shasum -a 256 -c "$MANIFEST" 2>&1 | grep -v ': OK$' || true )"
  listed="$(grep -c . "$DEST/$MANIFEST" || true)"
  present="$(cd "$DEST" && find . -type f ! -name "$MANIFEST" | wc -l | tr -d ' ')"
  if [ -n "$bad" ] || [ "$listed" != "$present" ]; then
    echo "error: vendor/drawio doesn't match its manifest — delete it and run packaging/fetch-drawio.sh $VERSION again" >&2
    [ -z "$bad" ] || printf '%s\n' "$bad" >&2
    [ "$listed" = "$present" ] || echo "  $present files in the directory, $listed in the manifest" >&2
    return 1
  fi
}

# The commit each tag pointed at when we pinned it. bash 3.2 (macOS's) has no associative arrays.
expected_commit() {
  case "$1" in
    v31.4.2) echo "074a2ea4f2be105b1fe7cae9a26ecc15761dcef6" ;;
    *) echo "" ;;
  esac
}

# Licenses that must be in the copy: the assets' own terms ride along with the assets.
LICENSES="stencils/LICENSE shapes/LICENSE img/LICENSE templates/LICENSE js/libavoid-js/LICENSE"

MIRRORS=(
  "https://gitee.com/mirrors/drawio.git"
  "https://github.com/jgraph/drawio.git"
)

EXPECTED="$(expected_commit "$VERSION")"
[ -n "$EXPECTED" ] || { echo "error: no pinned commit for $VERSION — add one to expected_commit() first" >&2; exit 1; }

# Packaging asks this before embedding the directory, and never fetches.
if [ "$VERIFY" = 1 ]; then
  [ -f "$DEST/VERSION" ] || { echo "error: vendor/drawio isn't there — run packaging/fetch-drawio.sh $VERSION" >&2; exit 1; }
  at="$(cat "$DEST/VERSION")"
  [ "$at" = "$VERSION" ] || { echo "error: vendor/drawio is $at, not $VERSION — fetch that version or verify $at" >&2; exit 1; }
  verify_manifest || exit 1
  echo "vendor/drawio is $VERSION and matches its manifest ($(grep -c . "$DEST/$MANIFEST") files)"
  exit 0
fi

if [ -f "$DEST/VERSION" ] && [ "$(cat "$DEST/VERSION")" = "$VERSION" ]; then
  # A cache that still matches what was fetched is reused; one that doesn't is fetched again.
  if verify_manifest; then
    echo "vendor/drawio is already at $VERSION and matches its manifest (delete it to fetch again)"
    exit 0
  fi
  echo "==> vendor/drawio is no longer what was fetched: fetching it again"
fi

echo "==> fetching draw.io $VERSION ($EXPECTED)"
rm -rf "$WORK"
for mirror in "${MIRRORS[@]}"; do
  echo "    trying $mirror"
  if GIT_TERMINAL_PROMPT=0 git clone --depth 1 --branch "$VERSION" "$mirror" "$WORK" 2>/dev/null; then
    got="$(git -C "$WORK" rev-parse HEAD)"
    if [ "$got" != "$EXPECTED" ]; then
      echo "error: $mirror's $VERSION is $got, not $EXPECTED — refusing it" >&2
      rm -rf "$WORK"
      continue
    fi
    break
  fi
  rm -rf "$WORK"
done
[ -d "$WORK/src/main/webapp" ] || { echo "error: could not fetch draw.io $VERSION" >&2; exit 1; }
[ "$(git -C "$WORK" rev-parse HEAD)" = "$EXPECTED" ] || { echo "error: the checkout isn't the pinned commit" >&2; exit 1; }

WEBAPP="$WORK/src/main/webapp"
echo "==> installing the web app into vendor/drawio"
rm -rf "$DEST"
mkdir -p "$DEST"
# Everything the editor loads at runtime; the pruned files are the hosted-service integrations,
# the dev sources' source maps and the Java servlet packaging, none of which a local editor uses.
rsync -a \
  --exclude 'WEB-INF/' --exclude 'META-INF/' \
  --exclude '*.map' \
  --exclude 'js/integrate.min.js' \
  --exclude 'js/viewer-static.min.js' \
  --exclude 'service-worker.js' --exclude 'workbox-*.js' \
  --exclude 'connect/' \
  --exclude 'dropbox.html' --exclude 'onedrive3.html' --exclude 'github.html' \
  --exclude 'gitlab.html' --exclude 'teams.html' --exclude 'monday-app-association.json' \
  --exclude 'clear.html' --exclude 'export3.html' --exclude 'open.html' \
  --exclude 'vsdxImporter.html' \
  "$WEBAPP/" "$DEST/"

# The parts that aren't Apache-2.0 must keep the notices that say so.
for f in $LICENSES; do
  [ -f "$DEST/$f" ] || { echo "error: $f is missing from vendor/drawio — the assets' licenses have to ship with them" >&2; exit 1; }
done

printf '%s\n' "$VERSION" > "$DEST/VERSION"
{
  printf 'draw.io %s — https://github.com/jgraph/drawio\n' "$VERSION"
  printf 'Web app: Apache-2.0. img/, stencils/, shapes/: their own notice (LICENSE in each);\n'
  printf 'templates/: CC-BY-4.0; js/libavoid-js/: LGPL-2.1. See ATTRIBUTION.md.\n'
} > "$DEST/README.md"

rm -rf "$WORK"
write_manifest
echo "==> vendor/drawio: $(du -sh "$DEST" | cut -f1), $(grep -c . "$DEST/$MANIFEST") files hashed into $MANIFEST"
