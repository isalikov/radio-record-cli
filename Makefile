APP := radiome

.DEFAULT_GOAL := help

.PHONY: help build run test test-audio check fmt clean

help:
	@printf "%s\n" \
		"radiome targets:" \
		"  make build  - build the app" \
		"  make run    - run the app" \
		"  make test   - run tests" \
		"  make test-audio - test mpv with silent audio output" \
		"  make check  - check formatting and lint code" \
		"  make fmt    - format code" \
		"  make clean  - remove build artifacts"

build:
	cargo build

run:
	cargo run

test:
	cargo test

test-audio:
	cargo test mpv_falls_back_measures_real_audio_and_accepts_volume_and_pause -- --ignored

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
