.PHONY: build install update test check fmt clippy clean build-web build-web-dev coverage dev run build-frontend-dev

# Run tama in dev mode: proxy (:11434) + web UI (:11435) as a single foreground process
run: build-frontend-dev
	cargo run --bin tama serve

# Run the SvelteKit dev server with hot reload on http://localhost:5173
dev:
	cd crates/tama-web/ui && pnpm dev

# Build the SvelteKit frontend into crates/tama-web/ui/build/ (required before any Rust release build)
build-frontend:
	cd crates/tama-web/ui && pnpm install && pnpm build

# Development SvelteKit build (faster, for local testing)
build-frontend-dev:
	cd crates/tama-web/ui && pnpm install && pnpm build

# Full release build: frontend first, then the Rust workspace
build: build-frontend
	cargo build --release --workspace

# Install tama CLI (includes web UI via default feature)
install: build-frontend
	cargo install --path crates/tama-cli --force

# Stop service, rebuild + reinstall (frontend + backend), restart service
update: build-frontend
	cargo build --release --workspace
	tama service stop || true
	cargo install --path crates/tama-cli --force
	tama service start

# Run all tests
test: build-frontend-dev
	cargo test --workspace

check: fmt-check clippy test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Lint everything
clippy:
	cargo clippy --workspace --all-targets -- -D warnings

clean:
	cargo clean
	rm -rf crates/tama-web/ui/build

# Aliases kept for backwards compat — both now build the main tama binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace

# Run code coverage analysis with cargo-tarpaulin (HTML report in target/coverage/)
coverage:
	cargo tarpaulin --workspace --out Html --output-dir target/coverage --timeout 300
