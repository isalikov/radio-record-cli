#!/bin/sh

set -eu

die() {
	printf '%s\n' "$*" >&2
	exit 1
}

have() {
	command -v "$1" >/dev/null 2>&1
}

repo=${RADIO_RECORD_REPO:-isalikov/radio-record-cli}
version=${RADIO_RECORD_VERSION:-latest}
prefix=${RADIO_RECORD_PREFIX:-$HOME/.local}
bin_dir=$prefix/bin
release_base=${RADIO_RECORD_RELEASE_BASE_URL:-https://github.com/$repo/releases/$version/download}

os=$(uname -s)
arch=$(uname -m)

case "$os" in
	Darwin)
		platform=macos
		;;
	Linux)
		platform=linux
		;;
	*)
		die "Unsupported operating system: $os"
		;;
esac

case "$arch" in
	x86_64|amd64)
		arch=x86_64
		;;
	arm64|aarch64)
		arch=aarch64
		;;
	*)
		die "Unsupported architecture: $arch"
		;;
esac

tmp_dir=$(mktemp -d 2>/dev/null || mktemp -d -t radio-record-install)
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

asset="radio-record-${platform}-${arch}.tar.gz"
asset_url="$release_base/$asset"

printf '%s\n' "Installing radio-record for ${platform}/${arch}"

if curl -fsSL "$asset_url" -o "$tmp_dir/$asset"; then
	tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
	install -d "$bin_dir"
	install -m 755 "$tmp_dir/radio-record" "$bin_dir/radio-record"
	printf '%s\n' "Installed to $bin_dir/radio-record"
else
	if have cargo; then
		printf '%s\n' "Release asset not found, building from source with cargo"
		if [ "$platform" = linux ]; then
			printf '%s\n' "Linux source builds need ALSA headers and pkg-config (Debian/Ubuntu: sudo apt install libasound2-dev pkg-config)"
		fi
		cargo install --locked --git "https://github.com/$repo.git" --force --root "$prefix" radio-record
		printf '%s\n' "Installed to $bin_dir/radio-record"
	else
		die "No release asset found at $asset_url and cargo is not installed"
	fi
fi

case ":$PATH:" in
	*":$bin_dir:"*)
		;;
	*)
		printf '%s\n' "Add $bin_dir to PATH if radio-record is not found after installation"
		;;
esac
