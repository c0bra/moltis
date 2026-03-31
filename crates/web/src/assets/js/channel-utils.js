// ── Shared channel RPC wrappers and validation ────────────────
//
// Used by page-channels.js and onboarding-view.js.

import { sendRpc } from "./helpers.js";

export var MATRIX_DOCS_URL = "https://docs.moltis.org/matrix.html";
export var MATRIX_DEFAULT_HOMESERVER = "https://matrix.org";
export var CHANNEL_STORAGE_NOTE =
	"Channels added or edited in the web UI are stored in Moltis's internal database (data_dir()/moltis.db). They are not written back to moltis.toml.";

/**
 * Validate required channel fields before submission.
 * @param {string} type - channel type
 * @param {string} accountId - account identifier
 * @param {string} credential - primary credential (token or app password)
 * @param {{ matrixAuthMode?: string, matrixUserId?: string }} [options]
 * @returns {{ valid: true } | { valid: false, error: string }}
 */
export function validateChannelFields(type, accountId, credential, options = {}) {
	if (!accountId.trim()) {
		return { valid: false, error: "Account ID is required." };
	}
	if (!credential.trim()) {
		if (type === "matrix") {
			return { valid: false, error: matrixCredentialError(options.matrixAuthMode) };
		}
		return {
			valid: false,
			error: type === "msteams" ? "App password is required." : "Bot token is required.",
		};
	}
	if (
		type === "matrix" &&
		normalizeMatrixAuthMode(options.matrixAuthMode) === "password" &&
		!String(options.matrixUserId || "").trim()
	) {
		return { valid: false, error: "Matrix user ID is required for password login." };
	}
	return { valid: true };
}

export function normalizeMatrixAuthMode(authMode) {
	return authMode === "password" ? "password" : "access_token";
}

export function matrixCredentialLabel(authMode) {
	return normalizeMatrixAuthMode(authMode) === "password" ? "Password" : "Access Token";
}

export function matrixCredentialPlaceholder(authMode) {
	return normalizeMatrixAuthMode(authMode) === "password" ? "Account password" : "syt_...";
}

export function matrixCredentialError(authMode) {
	return normalizeMatrixAuthMode(authMode) === "password" ? "Password is required." : "Access token is required.";
}

function randomSuffix(length) {
	if (typeof window !== "undefined" && window.crypto?.getRandomValues) {
		var bytes = new Uint8Array(length);
		window.crypto.getRandomValues(bytes);
		return Array.from(bytes, (byte) => (byte % 36).toString(36)).join("");
	}
	var value = "";
	while (value.length < length) {
		value += Math.floor(Math.random() * 36).toString(36);
	}
	return value.slice(0, length);
}

function slugifyMatrixAccountPart(value) {
	return String(value || "")
		.toLowerCase()
		.trim()
		.replace(/^@/, "")
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/-+/g, "-")
		.replace(/^-|-$/g, "");
}

function matrixHomeserverHost(homeserver) {
	var raw = String(homeserver || "").trim();
	if (!raw) return "";
	if (!/^https?:\/\//i.test(raw)) raw = `https://${raw}`;
	try {
		return new URL(raw).hostname;
	} catch (_error) {
		return "";
	}
}

/**
 * Generate a local Matrix account identifier for Moltis.
 * Prefer the Matrix user ID when present, otherwise derive from homeserver.
 * @param {{ userId?: string, homeserver?: string }} options
 * @returns {string}
 */
export function deriveMatrixAccountId(options = {}) {
	var userSlug = slugifyMatrixAccountPart(options.userId);
	if (userSlug) return userSlug.slice(0, 80);

	var hostSlug = slugifyMatrixAccountPart(matrixHomeserverHost(options.homeserver));
	var base = hostSlug || "matrix";
	return `${base}-${randomSuffix(6)}`.slice(0, 80);
}

/**
 * Normalize Matrix OTP cooldown input to a positive integer.
 * @param {string | number | null | undefined} value
 * @param {number} [fallback]
 * @returns {number}
 */
export function normalizeMatrixOtpCooldown(value, fallback = 300) {
	var parsed = Number.parseInt(String(value || ""), 10);
	return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/**
 * Parse an optional advanced channel config JSON object.
 * @param {string | null | undefined} text
 * @returns {{ ok: true, value: Record<string, unknown> } | { ok: false, error: string }}
 */
export function parseChannelConfigPatch(text) {
	var raw = String(text || "").trim();
	if (!raw) return { ok: true, value: {} };
	try {
		var value = JSON.parse(raw);
		if (!(value && typeof value === "object" && !Array.isArray(value))) {
			return { ok: false, error: "Advanced config must be a JSON object." };
		}
		return { ok: true, value };
	} catch (error) {
		var message = error instanceof Error ? error.message : String(error || "unknown error");
		return { ok: false, error: `Advanced config JSON is invalid: ${message}` };
	}
}

/**
 * Add a new channel (e.g. Telegram bot).
 * @param {string} type - channel type, e.g. "telegram"
 * @param {string} accountId - bot username / account identifier
 * @param {object} config - channel-specific config (token, dm_policy, etc.)
 */
export function addChannel(type, accountId, config) {
	return sendRpc("channels.add", { type, account_id: accountId, config });
}

/**
 * Fetch the current status of all configured channels.
 * Resolves with the RPC response; payload has `{ channels: [] }`.
 */
export function fetchChannelStatus() {
	return sendRpc("channels.status", {});
}

/**
 * Default base URL for Teams webhook endpoints (current page origin).
 */
export function defaultTeamsBaseUrl() {
	if (typeof window === "undefined") return "";
	return window.location?.origin || "";
}

/**
 * Normalise a user-provided base URL into `protocol://host`.
 */
export function normalizeBaseUrlForWebhook(baseUrl) {
	var raw = (baseUrl || "").trim();
	if (!raw) raw = defaultTeamsBaseUrl();
	if (!raw) return "";
	if (!/^https?:\/\//i.test(raw)) raw = `https://${raw}`;
	try {
		var parsed = new URL(raw);
		return `${parsed.protocol}//${parsed.host}`;
	} catch (_e) {
		return "";
	}
}

/**
 * Generate a random 48-hex-char webhook secret.
 */
export function generateWebhookSecretHex() {
	if (typeof window !== "undefined" && window.crypto?.getRandomValues) {
		var bytes = new Uint8Array(24);
		window.crypto.getRandomValues(bytes);
		return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
	}
	var value = "";
	while (value.length < 48) {
		value += Math.floor(Math.random() * 16).toString(16);
	}
	return value.slice(0, 48);
}

/**
 * Build the full Teams messaging endpoint URL.
 */
export function buildTeamsEndpoint(baseUrl, accountId, webhookSecret) {
	var normalizedBase = normalizeBaseUrlForWebhook(baseUrl);
	var account = (accountId || "").trim();
	var secret = (webhookSecret || "").trim();
	if (!(normalizedBase && account && secret)) return "";
	return `${normalizedBase}/api/channels/msteams/${encodeURIComponent(account)}/webhook?secret=${encodeURIComponent(secret)}`;
}
