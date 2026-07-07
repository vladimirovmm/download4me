check:
	cargo clippy --no-deps --all-targets -- -Dwarnings
	cargo +nightly fmt --check

fix:
	cargo fix  --allow-dirty --allow-staged --all-targets --all
	cargo clippy --fix --no-deps --allow-dirty --allow-staged --all-targets --all
	cargo +nightly fmt --all

build:
	cargo build
