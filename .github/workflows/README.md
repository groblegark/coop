# GitHub Actions Workflows (Disabled)

All CI/CD has moved to [RWX](https://www.rwx.com/). Workflow configs live in the `.rwx/` directory.

## RWX Workflows

| RWX workflow | Replaces | Purpose |
|---|---|---|
| `.rwx/ci.yml` | *(new)* | Lint (fmt, clippy), audit, deny, tests |
| `.rwx/docker.yml` | `.github/workflows/build.yml` | Build native OCI images and push to GHCR (empty, claude, gemini, coopmux, claudeless) |
| `.rwx/release.yml` | `.github/workflows/release.yml` | Cross-compile Linux binaries, create GitHub Release, dispatch gastown rebuild |

## Disabled Workflows

- `build.yml.disabled` -- was "Build and Push to ECR" (manual dispatch only; already noted as disabled in favor of GHCR)
- `release.yml.disabled` -- was "Release" (build multi-arch binaries + GitHub Release on tag push)
