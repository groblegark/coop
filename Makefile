.PHONY: check ci fmt install coverage outdated try-claude try-claudeless try-gemini docker-test-image try-docker-claudeless try-docker-claude try-docker-gemini test-docker capture-claude

# Quick checks
#
# Excluded:
#   SKIP `cargo audit`
#   SKIP `cargo deny`
#
check:
	cargo fmt --all
	cargo clippy --all -- -D warnings
	quench check --fix
	cargo build --all
	cargo test --all

# Full pre-release checks
ci:
	cargo fmt --all
	cargo clippy --all -- -D warnings
	quench check --fix
	cargo build --all
	cargo test --all
	cargo audit
	cargo deny check licenses bans sources

# Format code
fmt:
	cargo fmt --all

# Add license headers (--ci required for --license)
license:
	quench check --ci --fix --license

# Build and install coop to ~/.local/bin
install:
	@scripts/install

# Generate coverage report
coverage:
	@scripts/coverage

# Check for outdated dependencies
outdated:
	cargo outdated

# Launch coop wrapping claude with browser terminal
try-claude:
	@COOP_AGENT=claude bun tests/debug/start.ts -- claude

# Launch coop wrapping claudeless with browser terminal
# Usage: make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_hello.toml
try-claudeless:
	@COOP_AGENT=claude bun tests/debug/start.ts -- claudeless --scenario $(SCENARIO)

# Launch coop wrapping gemini with browser terminal
try-gemini:
	@COOP_AGENT=gemini bun tests/debug/start.ts -- gemini

# Build Docker claudeless image (for testing)
docker-test-image:
	docker build --target claudeless -t coop:test .

# Launch coop + claudeless in Docker with browser terminal
# Usage: make try-docker-claudeless SCENARIO=claude_hello.toml
try-docker-claudeless:
	@bun tests/debug/start-docker.ts claudeless --scenario $(or $(SCENARIO),claude_hello.toml)

# Launch coop + claude CLI in Docker with browser terminal
# Usage: make try-docker-claude PROFILE=trusted
try-docker-claude:
	@bun tests/debug/start-docker.ts claude $(if $(PROFILE),--profile $(PROFILE))

# Launch coop + gemini CLI in Docker with browser terminal
try-docker-gemini:
	@bun tests/debug/start-docker.ts gemini

# Capture state changes during claude onboarding (interactive)
# Usage: make capture-claude CONFIG=empty    (full onboarding)
#        make capture-claude CONFIG=auth-only (skip login)
#        make capture-claude CONFIG=trusted   (skip to idle)
capture-claude:
	@bun tests/debug/capture-claude.ts --config $(or $(CONFIG),empty)

# Run Docker e2e tests (builds test image first)
test-docker: docker-test-image
	COOP_DOCKER_TESTS=1 cargo test --test docker_e2e -- --test-threads=1
