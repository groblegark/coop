.PHONY: check ci fmt install coverage outdated try-claude try-claudeless try-gemini docker-test-image try-docker-claudeless try-docker-claude try-docker-gemini test-docker

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
	@COOP_AGENT=claude tests/debug/start.sh -- claude

# Launch coop wrapping claudeless with browser terminal
# Usage: make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_hello.toml
try-claudeless:
	@COOP_AGENT=claude tests/debug/start.sh -- claudeless --scenario $(SCENARIO)

# Launch coop wrapping gemini with browser terminal
try-gemini:
	@COOP_AGENT=gemini tests/debug/start.sh -- gemini

# Build Docker test image
docker-test-image:
	docker build --target test -t coop:test .

# Launch coop + claudeless in Docker with browser terminal
# Usage: make try-docker-claudeless SCENARIO=claude_hello.toml
try-docker-claudeless:
	@tests/debug/start-docker.sh claudeless --scenario $(or $(SCENARIO),claude_hello.toml)

# Launch coop + claude CLI in Docker with browser terminal
try-docker-claude:
	@tests/debug/start-docker.sh claude

# Launch coop + gemini CLI in Docker with browser terminal
try-docker-gemini:
	@tests/debug/start-docker.sh gemini

# Run Docker e2e tests (builds test image first)
test-docker: docker-test-image
	COOP_DOCKER_TESTS=1 cargo test --test docker_e2e -- --test-threads=1
