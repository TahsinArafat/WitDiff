fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace --all-features

check: fmt lint test

run:
	cargo run -p witdiff -- --help
