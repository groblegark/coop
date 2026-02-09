// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { join } from "node:path";
import { $ } from "bun";
import { scriptDir } from "./setup";

/** Load key=value pairs from tests/debug/.env (if it exists). */
export async function loadEnvFile(): Promise<void> {
	const envPath = join(scriptDir(), ".env");
	const file = Bun.file(envPath);
	if (!(await file.exists())) return;

	const text = await file.text();
	for (const line of text.split("\n")) {
		const trimmed = line.replace(/#.*$/, "").trim();
		if (!trimmed) continue;
		const eq = trimmed.indexOf("=");
		if (eq < 0) continue;
		const key = trimmed.slice(0, eq).trim();
		const value = trimmed.slice(eq + 1).trim();
		if (key && !(key in process.env)) {
			process.env[key] = value;
		}
	}
}

/**
 * Resolve OAuth token via fallback chain:
 * env var → macOS Keychain → ~/.claude/.credentials.json
 */
export async function resolveOAuthToken(): Promise<string> {
	// 1. Environment variable
	if (process.env.CLAUDE_CODE_OAUTH_TOKEN) {
		return process.env.CLAUDE_CODE_OAUTH_TOKEN;
	}

	// 2. macOS Keychain
	try {
		const kcJson =
			await $`security find-generic-password -s "Claude Code-credentials" -w`
				.quiet()
				.text();
		const data = JSON.parse(kcJson);
		const token = data?.claudeAiOauth?.accessToken;
		if (token) return token;
	} catch {
		// not found or not macOS
	}

	// 3. ~/.claude/.credentials.json
	const credPath = join(process.env.HOME ?? "", ".claude/.credentials.json");
	try {
		const data = await Bun.file(credPath).json();
		const token = data?.claudeAiOauth?.accessToken;
		if (token) return token;
	} catch {
		// file doesn't exist
	}

	throw new Error(
		"No OAuth token found. Set CLAUDE_CODE_OAUTH_TOKEN, add tests/debug/.env, or log in with 'claude'.",
	);
}

/**
 * Resolve oauthAccount JSON via fallback chain:
 * env var → ~/.claude/.claude.json
 */
export async function resolveOAuthAccount(): Promise<Record<string, unknown>> {
	// 1. Environment variable
	if (process.env.CLAUDE_OAUTH_ACCOUNT) {
		return JSON.parse(process.env.CLAUDE_OAUTH_ACCOUNT);
	}

	// 2. ~/.claude/.claude.json
	const claudePath = join(process.env.HOME ?? "", ".claude/.claude.json");
	try {
		const data = await Bun.file(claudePath).json();
		if (data?.oauthAccount) return data.oauthAccount;
	} catch {
		// file doesn't exist
	}

	throw new Error(
		"No oauthAccount found. Set CLAUDE_OAUTH_ACCOUNT in env or tests/debug/.env, or log in with 'claude'.",
	);
}

export async function writeCredentials(
	configDir: string,
	token: string,
	account: Record<string, unknown>,
	baseJson: Record<string, unknown>,
): Promise<void> {
	// Write .credentials.json
	const credentials = {
		claudeAiOauth: {
			accessToken: token,
			refreshToken: "",
			expiresAt: 9999999999999,
			scopes: ["user:inference", "user:profile", "user:sessions:claude_code"],
		},
	};
	await Bun.write(
		join(configDir, ".credentials.json"),
		JSON.stringify(credentials),
	);

	// Detect lastOnboardingVersion from claude --version
	let onboardingVer = "0.0.0";
	try {
		const ver = await $`claude --version`.quiet().text();
		const match = ver.match(/(\d+\.\d+\.\d+)/);
		if (match) onboardingVer = match[1];
	} catch {
		// claude not installed
	}

	// Inject oauthAccount + lastOnboardingVersion into base json
	const claudeJson = {
		...baseJson,
		oauthAccount: account,
		lastOnboardingVersion: onboardingVer,
	};
	await Bun.write(
		join(configDir, ".claude.json"),
		JSON.stringify(claudeJson, null, 2),
	);
}

export async function detectOnboardingVersion(): Promise<string> {
	try {
		const ver = await $`claude --version`.quiet().text();
		const match = ver.match(/(\d+\.\d+\.\d+)/);
		return match?.[1] ?? "0.0.0";
	} catch {
		return "0.0.0";
	}
}
