#!/bin/sh

# Removes the radio-record binary installed by scripts/install.sh.
# Settings are kept unless RADIO_RECORD_PURGE=1 is set or --purge is passed.

set -eu

die() {
	printf '%s\n' "$*" >&2
	exit 1
}

prefix=${RADIO_RECORD_PREFIX:-$HOME/.local}
bin_dir=$prefix/bin
binary=$bin_dir/radio-record
purge=${RADIO_RECORD_PURGE:-0}

for arg in "$@"; do
	case "$arg" in
		--purge)
			purge=1
			;;
		-h|--help)
			printf '%s\n' \
				"Usage: uninstall.sh [--purge]" \
				"" \
				"Removes $binary." \
				"  --purge   also delete favorites and volume (settings.json)" \
				"" \
				"Environment: RADIO_RECORD_PREFIX (default \$HOME/.local), RADIO_RECORD_PURGE=1"
			exit 0
			;;
		*)
			die "Unknown argument: $arg"
			;;
	esac
done

# Same precedence as the app: RADIO_RECORD_CONFIG_DIR, $XDG_CONFIG_HOME/radio-record, ~/.config/radio-record.
if [ -n "${RADIO_RECORD_CONFIG_DIR:-}" ]; then
	config_dir=$RADIO_RECORD_CONFIG_DIR
elif [ -n "${XDG_CONFIG_HOME:-}" ]; then
	config_dir=$XDG_CONFIG_HOME/radio-record
else
	config_dir=$HOME/.config/radio-record
fi

if [ -f "$binary" ]; then
	rm -f "$binary"
	printf '%s\n' "Removed $binary"
else
	printf '%s\n' "No binary at $binary"
fi

if [ "$purge" = 1 ]; then
	if [ -d "$config_dir" ]; then
		rm -rf "$config_dir"
		printf '%s\n' "Removed settings in $config_dir"
	else
		printf '%s\n' "No settings directory at $config_dir"
	fi
elif [ -d "$config_dir" ]; then
	printf '%s\n' "Kept settings in $config_dir (run with --purge to delete)"
fi

# The installer's cargo fallback records the crate in $prefix/.crates.toml and
# $prefix/.crates2.json. Those files are shared with any other crates installed
# to the same prefix, so they are left in place.
