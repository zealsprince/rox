#!/usr/bin/env bash
# Rebuilds the patched vendor copies Cargo's [patch.crates-io] points at:
# each crate's crates.io source with the patches from patches/<crate>/
# applied. The nix shellHook runs this on shell entry; run it by hand once
# before building without nix. Per-crate stamp, so it's a no-op when the
# crate version and its patches are unchanged.
set -euo pipefail
cd "$(dirname "$0")/.."

# name version sha256-of-the-.crate-tarball (from Cargo.lock's checksum)
# The nix package build duplicates this table in flake.nix (vendoredCrate
# calls), bump both together.
crates=(
    "gpui 0.2.2 979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707"
    "gpui-component 0.5.1 d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a"
)

checksum() {
    if command -v sha256sum >/dev/null; then
        sha256sum
    else
        shasum -a 256
    fi | cut -d' ' -f1
}

vendor_one() {
    local name=$1 version=$2 sha256=$3
    local crate="$name-$version.crate"
    local out="vendor/$name"
    local stamp="$out/.rox-stamp"
    local patches=(patches/"$name"/*.patch)

    local want="$version-$(cat "${patches[@]}" | checksum)"
    if [[ -f $stamp && $(<"$stamp") == "$want" ]]; then
        return 0
    fi

    local tmp
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' RETURN

    # Cargo already verified anything in its cache, so prefer that over the wire.
    local cached
    cached=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/cache" -name "$crate" 2>/dev/null | head -1 || true)
    if [[ -n $cached ]]; then
        cp "$cached" "$tmp/$crate"
    else
        curl -fsSL "https://static.crates.io/crates/$name/$crate" -o "$tmp/$crate"
    fi
    if [[ $(checksum <"$tmp/$crate") != "$sha256" ]]; then
        echo "vendor-gpui: checksum mismatch for $crate" >&2
        exit 1
    fi

    tar -xzf "$tmp/$crate" -C "$tmp"
    for p in "${patches[@]}"; do
        patch -p1 -d "$tmp/$name-$version" --no-backup-if-mismatch --quiet <"$p"
    done

    rm -rf "$out"
    mkdir -p vendor
    mv "$tmp/$name-$version" "$out"
    echo "$want" >"$stamp"
    echo "vendor-gpui: $name $version patched into $out"
}

for entry in "${crates[@]}"; do
    # shellcheck disable=SC2086
    vendor_one $entry
done
