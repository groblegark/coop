# Plan: Migrate from Docker to Native RWX OCI Image Building

> **Issue**: E2E workflow fails; Docker workflow uses `docker: true` tasks with inline
> Dockerfiles; the repo Dockerfile still uses musl/bookworm-slim while RWX tasks use
> ubuntu:24.04. This plan replaces all Docker-based image assembly with RWX native
> OCI building (`$RWX_IMAGE`), eliminating Docker-in-CI entirely.

## Current State

### Images built today (from `Dockerfile` + `.rwx/docker.yml`)

| Image tag | Base | Contents | Used by |
|-----------|------|----------|---------|
| `empty` | ubuntu:24.04 | coop binary + dev tools | General deployment |
| `claude` | ubuntu:24.04 | coop + Claude CLI | Claude agent pods |
| `gemini` | ubuntu:24.04 | coop + Gemini CLI + Node.js | Gemini agent pods |
| `claudeless` | ubuntu:24.04 | coop + claudeless + test scenarios | E2E docker tests (GHA) |
| `coopmux` | ubuntu:24.04 | coopmux + kubectl + k8s-launch.sh | K8s mux deployment |

### Problems

1. **`docker.yml` uses `docker: true` tasks** — spins up a Docker daemon inside RWX
   just to run `docker build` on an inline Dockerfile. This is slow (daemon startup,
   layer pulls) and defeats RWX's content-based caching.

2. **E2E workflow (`e2e.yml`) builds binaries natively** but still needs Node.js +
   Playwright installed via apt. The E2E test runs against a live `coopmux` process,
   not a Docker container, so Docker isn't needed there. The current E2E failure is
   unrelated to Docker but the `node` task's `use: code` dependency means it needs
   system-deps that come from a different branch of the DAG.

3. **The repo `Dockerfile` is stale** — still references musl targets and
   `debian:bookworm-slim` while all RWX workflows use `ubuntu:24.04`.

4. **`docker.yml` push tasks duplicate apt-get installs** — each variant
   (`push-empty`, `push-claude`, `push-gemini`) installs the same dev tools in its
   inline Dockerfile, with no caching between them.

## Proposed Architecture

### Phase 1: Migrate docker.yml to native RWX OCI images

Replace `docker: true` + inline Dockerfiles with `$RWX_IMAGE`-based image building
and `rwx image push`.

**New DAG for `.rwx/docker.yml`:**

```
build-deps ──┐
code ────────┤
rust ────────┘── cargo-deps ── build-coop (artifact: coop-binary)
                                    │
                 ┌──────────────────┼──────────────────┐
                 ▼                  ▼                  ▼
           image-empty        image-claude        image-gemini
           (base+coop)       (base+coop+claude)  (base+coop+gemini+node)
                 │                  │                  │
                 ▼                  ▼                  ▼
           push-empty         push-claude         push-gemini
```

Each `image-*` task:
- Starts from a clean `base` (no `use:` from build tasks — mimics multi-stage)
- Installs runtime deps via `run:`
- Copies the coop binary from `build-coop` artifact
- Sets `$RWX_IMAGE/entrypoint`, `$RWX_IMAGE/command`

Each `push-*` task:
- Uses `rwx image push <task-id> --to ghcr.io/groblegark/coop:<tag>`
- Handles tagging logic (sha, latest, version)

**Example `image-empty` task:**
```yaml
base:
  image: ubuntu:24.04
  config: none   # <-- pristine base for OCI image building

tasks:
  # ... build tasks same as today ...

  - key: image-empty
    # No 'use:' — starts from clean base (multi-stage equivalent)
    run: |
      apt-get update && apt-get install -y --no-install-recommends \
        git python3 build-essential openssh-client \
        jq ripgrep fd-find tree \
        ca-certificates curl \
        && rm -rf /var/lib/apt/lists/*
      cp ${{ tasks.build-coop.artifacts.coop-binary }} /usr/local/bin/coop
      chmod +x /usr/local/bin/coop
      jq -n '["/usr/local/bin/coop"]' > $RWX_IMAGE/entrypoint.json
```

**Key concern**: The build tasks need `config: rwx/base 1.0.0` for cargo/rustup, but
the image tasks need `config: none` for clean OCI output. **This requires splitting
into two workflow files** (one for building, one for image assembly) or using the
`call:` directive to invoke a sub-workflow with a different `base:`.

**Recommendation**: Use `call:` to split:
- `.rwx/build.yml` — builds coop binary, outputs artifact (base: rwx/base)
- `.rwx/docker.yml` — assembles OCI images from artifact (base: ubuntu:24.04, config: none)

Or keep a single file and use `docker.yml`'s existing pattern where the build
tasks use `rwx/base` and the image tasks use separate sub-workflow files with
`config: none`.

### Phase 2: Add coopmux and claudeless images

The current `Dockerfile` defines `coopmux` and `claudeless` targets that aren't
in `docker.yml` yet. Add them:

- **`image-coopmux`**: coopmux binary + kubectl + k8s-launch.sh
- **`image-claudeless`**: coop binary + claudeless + test scenarios (for E2E)

### Phase 3: Fix E2E workflow

The E2E workflow doesn't use Docker at all — it builds native binaries and runs
Playwright tests against a live coopmux process. The current failure needs
investigation:

1. Check if the `node` task properly inherits system deps
2. Check if `playwright-deps` gets the right Node.js version
3. Check if the `e2e-tests` background process starts correctly

The E2E tests themselves are Playwright specs in `tests/e2e/specs/`:
- `mux-sessions.spec.ts`
- `mux-state-transitions.spec.ts`
- `mux-screen-rendering.spec.ts`
- `mux-keyboard-input.spec.ts`
- `mux-health-failure.spec.ts`

### Phase 4: Retire the Dockerfile

Once all images are built natively by RWX:
1. Delete `Dockerfile`
2. Remove `docker_e2e.rs` test (or migrate to use RWX-built images)
3. Update `CLAUDE.md` and `Makefile` references

## Migration Checklist

- [ ] Prototype `image-empty` with `$RWX_IMAGE` and `config: none`
- [ ] Verify `rwx image push` works with GHCR authentication
- [ ] Migrate `push-empty` → native OCI
- [ ] Migrate `push-claude` → native OCI (needs Claude CLI install)
- [ ] Migrate `push-gemini` → native OCI (needs Node.js + Gemini CLI)
- [ ] Add `image-coopmux` (kubectl + k8s-launch.sh)
- [ ] Add `image-claudeless` (for E2E testing)
- [ ] Investigate and fix E2E workflow failure
- [ ] Delete `Dockerfile`
- [ ] Update docs and Makefile

## Open Questions

1. **`config: none` vs `rwx/base`**: Can a single workflow file have tasks with
   different base configs? Or do we need sub-workflows via `call:`?

2. **GHCR auth for `rwx image push`**: The current approach uses
   `docker login` + `docker push`. Does `rwx image push` support GHCR directly,
   or do we still need `docker login` first? The docs show ECR with OIDC — need
   to verify GHCR token-based auth works.

3. **Image size**: RWX native images may have different layer structure than
   Docker images. Need to verify the resulting image sizes are comparable.

4. **Multi-arch**: The current `docker.yml` only builds linux/amd64 (via native
   RWX agents). The `Dockerfile` supports multi-arch via TARGETARCH. RWX native
   images are single-arch by default — may need parallel tasks per architecture.
