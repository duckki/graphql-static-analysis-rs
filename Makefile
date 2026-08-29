build:
	cargo build
	cargo test
	cargo clippy --all-targets --all-features -- -D warnings
	cargo +nightly fmt --all -- --check

fmt:
	cargo +nightly fmt --all
