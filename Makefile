.PHONY: build install update test check fmt clippy clean build-web build-web-dev wasm-target coverage dev run

# Run tama in dev mode: proxy (:11434) + web UI (:11435) as a single foreground process
run: build-frontend-dev
	cargo run --bin tama serve

# Run Leptos frontend dev server with hot reload on http://localhost:8080
dev: wasm-target
	cd crates/tama && trunk serve --port 8080 --public-url /tama --no-default-features --features csr

# Ensure the wasm32 target is installed (idempotent — safe to run multiple times)
wasm-target:
	rustup target add wasm32-unknown-unknown

# Build the Leptos WASM frontend into crates/tama/dist/ (required before any Rust release build)
build-frontend: wasm-target
	cd crates/tama && trunk build --release --public-url /tama --no-default-features --features csr

# Development WASM build (unoptimised, faster iteration)
build-frontend-dev: wasm-target
	cd crates/tama && trunk build --public-url /tama --no-default-features --features csr

# Full release build: frontend first, then the Rust workspace
build: build-frontend
	cargo build --release --workspace

# Install tama binary (includes web UI via default feature)
install: build-frontend
	cargo install --path crates/tama --force

# Stop service, rebuild + reinstall (frontend + backend), restart service
update: build-frontend
	cargo build --release --workspace
	tama service stop || true
	cargo install --path crates/tama --force
	tama service start

# Run all tests including the tama SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package tama --features ssr

check: fmt-check clippy test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Lint everything including the server-side tama code
clippy:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --package tama --features ssr -- -D warnings

clean:
	cargo clean
	rm -rf crates/tama/dist

# Aliases kept for backwards compat — both now build the main tama binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace

# Run code coverage analysis with cargo-tarpaulin (HTML report in target/coverage/)
coverage:
	cargo tarpaulin --workspace --features ssr --out Html --output-dir target/coverage --timeout 300
