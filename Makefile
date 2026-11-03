.PHONY: up down test bench demo fmt clippy

up:
	docker compose up -d

down:
	docker compose down

test:
	cargo test

bench:
	cargo run --bin ap-harness -- bench

demo:
	cargo run --bin ap-gateway

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

