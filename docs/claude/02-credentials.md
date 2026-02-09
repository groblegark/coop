# Claude Code Credential Shape

How Claude Code stores authentication state across its config directory
(`~/.claude/` or `$CLAUDE_CONFIG_DIR`).

Captured from a full onboarding flow (empty config -> OAuth -> onboarded -> trusted+idle).

## Files

### `.claude.json` -- account metadata and state

Primary config file. Grows through 4 stages during onboarding:

| Stage | Key fields added |
|-------|-----------------|
| 1. Pre-auth | `cachedGrowthBookFeatures`, `firstStartTime`, `userID`, migration flags |
| 2. Post-auth | `oauthAccount`, `claudeCodeFirstTokenDate` |
| 3. Onboarded | `hasCompletedOnboarding`, `lastOnboardingVersion` |
| 4. Trusted | `projects.{workspace}` (trust + tools + MCP), `settings.json`, caches |

The `oauthAccount` field is the identity payload:
```json
{
  "accountUuid": "...",
  "emailAddress": "...",
  "organizationUuid": "...",
  "billingType": "stripe_subscription",
  "displayName": "...",
  "organizationRole": "admin",
  "organizationName": "..."
}
```

### `.credentials.json` -- OAuth tokens

Written after successful OAuth flow. Single top-level key `claudeAiOauth`:

```json
{
  "claudeAiOauth": {
    "accessToken": "sk-ant-oat01-...",
    "refreshToken": "sk-ant-ort01-...",
    "expiresAt": 1770705499628,
    "scopes": ["user:inference", "user:mcp_servers", "user:profile", "user:sessions:claude_code"],
    "subscriptionType": "max",
    "rateLimitTier": "default_claude_max_20x"
  }
}
```

### `settings.json` -- global settings

Created during onboarding. Defaults to `{}`.

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `CLAUDE_CODE_OAUTH_TOKEN` | OAuth access token (alternative to `.credentials.json`) |
| `ANTHROPIC_API_KEY` | API key auth (no OAuth, direct API access) |
| `CLAUDE_CONFIG_DIR` | Override config directory location |

## Credential Switch (Issue #36)

The minimum credential switch for a session requires:

1. Replace `.credentials.json` (or set `CLAUDE_CODE_OAUTH_TOKEN` env var)
2. Update `oauthAccount` in `.claude.json` to match the new identity
3. Optionally set `CLAUDE_CODE_OAUTH_TOKEN` in the process environment

The `hasCompletedOnboarding`, `lastOnboardingVersion`, and `projects.*` fields
can remain from the previous session -- they are per-install, not per-identity.
