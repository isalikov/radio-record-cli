APP := radio-record

.DEFAULT_GOAL := help

.PHONY: help build run test test-audio check fmt clean

help:
	@printf "%s\n" \
		"radio-record targets:" \
		"  make build  - build the app" \
		"  make run    - run the app" \
		"  make test   - run tests" \
		"  make test-audio - open the audio device with silent output" \
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
	cargo test live_device_opens_current_format_silently -- --ignored --nocapture

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
