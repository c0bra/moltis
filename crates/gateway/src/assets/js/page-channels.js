// ── Channels page (Preact + HTM + Signals) ──────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect } from "preact/hooks";
import { onEvent } from "./events.js";
import { sendRpc } from "./helpers.js";
import { updateNavCount } from "./nav-counts.js";
import { registerPage } from "./router.js";
import { connected, models as modelsSig } from "./signals.js";
import * as S from "./state.js";
import { ConfirmDialog, Modal, requestConfirm } from "./ui.js";

export function prefetchChannels() {
	sendRpc("channels.status", {}).then((res) => {
		if (res?.ok) S.setCachedChannels(res.payload?.channels || []);
	});
}

var channels = signal([]);
var senders = signal([]);
var activeTab = signal("channels");
var showAddModal = signal(false);
var editingChannel = signal(null);
var sendersAccount = signal("");

function loadChannels() {
	sendRpc("channels.status", {}).then((res) => {
		if (res?.ok) {
			var ch = res.payload?.channels || [];
			channels.value = ch;
			S.setCachedChannels(ch);
			updateNavCount("channels", ch.length);
		}
	});
}

function loadSenders() {
	var accountId = sendersAccount.value;
	if (!accountId) {
		senders.value = [];
		return;
	}
	sendRpc("channels.senders.list", { account_id: accountId }).then((res) => {
		if (res?.ok) senders.value = res.payload?.senders || [];
	});
}

// ── Telegram icon (inline SVG via htm) ──────────────────────
function TelegramIcon() {
	return html`<svg width="16" height="16" viewBox="0 0 24 24" fill="none"
    stroke="currentColor" stroke-width="1.5">
    <path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z" />
  </svg>`;
}

// ── Channel card ─────────────────────────────────────────────
function ChannelCard(props) {
	var ch = props.channel;

	function onRemove() {
		requestConfirm(`Remove ${ch.name || ch.account_id}?`).then((yes) => {
			if (!yes) return;
			sendRpc("channels.remove", { account_id: ch.account_id }).then((r) => {
				if (r?.ok) loadChannels();
			});
		});
	}

	var statusClass = ch.status === "connected" ? "configured" : "oauth";
	var sessionLine = "";
	if (ch.sessions && ch.sessions.length > 0) {
		var active = ch.sessions.filter((s) => s.active);
		sessionLine =
			active.length > 0
				? active.map((s) => `${s.label || s.key} (${s.messageCount} msgs)`).join(", ")
				: "No active session";
	}

	return html`<div class="provider-card" style="padding:12px 14px;border-radius:8px;margin-bottom:8px;">
    <div style="display:flex;align-items:center;gap:10px;">
      <span style="display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;border-radius:6px;background:var(--surface2);">
        <${TelegramIcon} />
      </span>
      <div style="display:flex;flex-direction:column;gap:2px;">
        <span class="text-sm text-[var(--text-strong)]">${ch.name || ch.account_id || "Telegram"}</span>
        ${ch.details && html`<span class="text-xs text-[var(--muted)]">${ch.details}</span>`}
        ${sessionLine && html`<span class="text-xs text-[var(--muted)]">${sessionLine}</span>`}
      </div>
      <span class="provider-item-badge ${statusClass}">${ch.status || "unknown"}</span>
    </div>
    <div style="display:flex;gap:6px;">
      <button class="session-action-btn" title="Edit ${ch.account_id || "channel"}"
        onClick=${() => {
					editingChannel.value = ch;
				}}>Edit</button>
      <button class="session-action-btn session-delete" title="Remove ${ch.account_id || "channel"}"
        onClick=${onRemove}>Remove</button>
    </div>
  </div>`;
}

// ── Channels tab ─────────────────────────────────────────────
function ChannelsTab() {
	if (channels.value.length === 0) {
		return html`<div style="text-align:center;padding:40px 0;">
      <div class="text-sm text-[var(--muted)]" style="margin-bottom:12px;">No Telegram bots connected.</div>
      <div class="text-xs text-[var(--muted)]">Click "+ Add Telegram Bot" to connect one using a token from @BotFather.</div>
    </div>`;
	}
	return html`${channels.value.map((ch) => html`<${ChannelCard} key=${ch.account_id} channel=${ch} />`)}`;
}

// ── Senders tab ──────────────────────────────────────────────
function SendersTab() {
	useEffect(() => {
		if (channels.value.length > 0 && !sendersAccount.value) {
			sendersAccount.value = channels.value[0].account_id;
		}
	}, [channels.value]);

	useEffect(() => {
		loadSenders();
	}, [sendersAccount.value]);

	if (channels.value.length === 0) {
		return html`<div class="text-sm text-[var(--muted)]">No channels configured.</div>`;
	}

	function onAction(identifier, action) {
		var rpc = action === "approve" ? "channels.senders.approve" : "channels.senders.deny";
		sendRpc(rpc, {
			account_id: sendersAccount.value,
			identifier: identifier,
		}).then(() => loadSenders());
	}

	return html`<div>
    <div style="margin-bottom:12px;">
      <label class="text-xs text-[var(--muted)]" style="margin-right:6px;">Account:</label>
      <select style="background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:4px 8px;font-size:12px;"
        value=${sendersAccount.value} onChange=${(e) => {
					sendersAccount.value = e.target.value;
				}}>
        ${channels.value.map(
					(ch) => html`<option key=${ch.account_id} value=${ch.account_id}>${ch.name || ch.account_id}</option>`,
				)}
      </select>
    </div>
    ${senders.value.length === 0 && html`<div class="text-sm text-[var(--muted)] senders-empty">No messages received yet for this account.</div>`}
    ${
			senders.value.length > 0 &&
			html`<table class="senders-table">
      <thead><tr>
        <th class="senders-th">Sender</th><th class="senders-th">Username</th>
        <th class="senders-th">Messages</th><th class="senders-th">Last Seen</th>
        <th class="senders-th">Status</th><th class="senders-th">Action</th>
      </tr></thead>
      <tbody>
        ${senders.value.map((s) => {
					var identifier = s.username || s.peer_id;
					var lastSeenMs = s.last_seen ? s.last_seen * 1000 : 0;
					return html`<tr key=${s.peer_id}>
            <td class="senders-td">${s.sender_name || s.peer_id}</td>
            <td class="senders-td" style="color:var(--muted);">${s.username ? `@${s.username}` : "\u2014"}</td>
            <td class="senders-td">${s.message_count}</td>
            <td class="senders-td" style="color:var(--muted);font-size:12px;">${lastSeenMs ? html`<time data-epoch-ms="${lastSeenMs}">${new Date(lastSeenMs).toISOString()}</time>` : "\u2014"}</td>
            <td class="senders-td">
              <span class="provider-item-badge ${s.allowed ? "configured" : "oauth"}">${s.allowed ? "Allowed" : "Denied"}</span>
            </td>
            <td class="senders-td">
              ${
								s.allowed
									? html`<button class="session-action-btn session-delete" onClick=${() => onAction(identifier, "deny")}>Deny</button>`
									: html`<button class="session-action-btn" style="background:var(--accent-dim);color:white;" onClick=${() => onAction(identifier, "approve")}>Approve</button>`
							}
            </td>
          </tr>`;
				})}
      </tbody>
    </table>`
		}
  </div>`;
}

// ── Add channel modal ────────────────────────────────────────
function AddChannelModal() {
	var error = signal("");
	var saving = signal(false);

	function onSubmit(e) {
		e.preventDefault();
		var form = e.target.closest(".channel-form");
		var accountId = form.querySelector("[data-field=accountId]").value.trim();
		var token = form.querySelector("[data-field=token]").value.trim();
		if (!accountId) {
			error.value = "Bot username is required.";
			return;
		}
		if (!token) {
			error.value = "Bot token is required.";
			return;
		}
		var allowlist = form
			.querySelector("[data-field=allowlist]")
			.value.trim()
			.split(/\n/)
			.map((s) => s.trim())
			.filter(Boolean);
		error.value = "";
		saving.value = true;
		var addConfig = {
			token: token,
			dm_policy: form.querySelector("[data-field=dmPolicy]").value,
			mention_mode: form.querySelector("[data-field=mentionMode]").value,
			allowlist: allowlist,
		};
		var model = form.querySelector("[data-field=model]").value;
		if (model) addConfig.model = model;
		sendRpc("channels.add", {
			type: "telegram",
			account_id: accountId,
			config: addConfig,
		}).then((res) => {
			saving.value = false;
			if (res?.ok) {
				showAddModal.value = false;
				loadChannels();
			} else {
				error.value = (res?.error && (res.error.message || res.error.detail)) || "Failed to connect bot.";
			}
		});
	}

	var selectStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;cursor:pointer;";
	var inputStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;";

	return html`<${Modal} show=${showAddModal.value} onClose=${() => {
		showAddModal.value = false;
	}} title="Add Telegram Bot">
    <div class="channel-form">
      <div class="channel-card">
        <span class="text-xs font-medium text-[var(--text-strong)]">How to create a Telegram bot</span>
        <div class="text-xs text-[var(--muted)] channel-help">1. Open <a href="https://t.me/BotFather" target="_blank" class="text-[var(--accent)]" style="text-decoration:underline;">@BotFather</a> in Telegram</div>
        <div class="text-xs text-[var(--muted)]">2. Send /newbot and follow the prompts to choose a name and username</div>
        <div class="text-xs text-[var(--muted)]">3. Copy the bot token (looks like 123456:ABC-DEF...) and paste it below</div>
        <div class="text-xs text-[var(--muted)] channel-help" style="margin-top:2px;">See the <a href="https://core.telegram.org/bots/tutorial" target="_blank" class="text-[var(--accent)]" style="text-decoration:underline;">Telegram Bot Tutorial</a> for more details.</div>
      </div>
      <label class="text-xs text-[var(--muted)]">Bot username</label>
      <input data-field="accountId" type="text" placeholder="e.g. my_assistant_bot" style=${inputStyle} />
      <label class="text-xs text-[var(--muted)]">Bot Token (from @BotFather)</label>
      <input data-field="token" type="password" placeholder="123456:ABC-DEF..." style=${inputStyle} />
      <label class="text-xs text-[var(--muted)]">DM Policy</label>
      <select data-field="dmPolicy" style=${selectStyle}>
        <option value="open">Open (anyone)</option>
        <option value="allowlist">Allowlist only</option>
        <option value="disabled">Disabled</option>
      </select>
      <label class="text-xs text-[var(--muted)]">Group Mention Mode</label>
      <select data-field="mentionMode" style=${selectStyle}>
        <option value="mention">Must @mention bot</option>
        <option value="always">Always respond</option>
        <option value="none">Don't respond in groups</option>
      </select>
      <label class="text-xs text-[var(--muted)]">Default Model</label>
      <select data-field="model" style=${selectStyle}>
        <option value="">(server default)</option>
        ${modelsSig.value.map((m) => html`<option key=${m.id} value=${m.id}>${m.displayName || m.id}</option>`)}
      </select>
      <label class="text-xs text-[var(--muted)]">DM Allowlist (one username per line)</label>
      <textarea data-field="allowlist" placeholder="user1\nuser2" rows="3"
        style="font-family:var(--font-body);resize:vertical;background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;" />
      ${error.value && html`<div class="text-xs text-[var(--error)] channel-error" style="display:block;">${error.value}</div>`}
      <button class="bg-[var(--accent-dim)] text-white border-none px-4 py-2 rounded text-sm cursor-pointer hover:bg-[var(--accent)] transition-colors"
        onClick=${onSubmit} disabled=${saving.value}>
        ${saving.value ? "Connecting\u2026" : "Connect Bot"}
      </button>
    </div>
  </${Modal}>`;
}

// ── Edit channel modal ───────────────────────────────────────
function EditChannelModal() {
	var ch = editingChannel.value;
	if (!ch) return null;
	var cfg = ch.config || {};
	var error = signal("");
	var saving = signal(false);

	function onSave(e) {
		e.preventDefault();
		var form = e.target.closest(".channel-form");
		var allowlist = form
			.querySelector("[data-field=allowlist]")
			.value.trim()
			.split(/\n/)
			.map((s) => s.trim())
			.filter(Boolean);
		error.value = "";
		saving.value = true;
		var updateConfig = {
			token: cfg.token || "",
			dm_policy: form.querySelector("[data-field=dmPolicy]").value,
			mention_mode: form.querySelector("[data-field=mentionMode]").value,
			allowlist: allowlist,
		};
		var model = form.querySelector("[data-field=model]").value;
		if (model) updateConfig.model = model;
		sendRpc("channels.update", {
			account_id: ch.account_id,
			config: updateConfig,
		}).then((res) => {
			saving.value = false;
			if (res?.ok) {
				editingChannel.value = null;
				loadChannels();
			} else {
				error.value = (res?.error && (res.error.message || res.error.detail)) || "Failed to update bot.";
			}
		});
	}

	var selectStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;cursor:pointer;";

	return html`<${Modal} show=${true} onClose=${() => {
		editingChannel.value = null;
	}} title="Edit Telegram Bot">
    <div class="channel-form">
      <div class="text-sm text-[var(--text-strong)]">${ch.name || ch.account_id}</div>
      <label class="text-xs text-[var(--muted)]">DM Policy</label>
      <select data-field="dmPolicy" style=${selectStyle} value=${cfg.dm_policy || "open"}>
        <option value="open">Open (anyone)</option>
        <option value="allowlist">Allowlist only</option>
        <option value="disabled">Disabled</option>
      </select>
      <label class="text-xs text-[var(--muted)]">Group Mention Mode</label>
      <select data-field="mentionMode" style=${selectStyle} value=${cfg.mention_mode || "mention"}>
        <option value="mention">Must @mention bot</option>
        <option value="always">Always respond</option>
        <option value="none">Don't respond in groups</option>
      </select>
      <label class="text-xs text-[var(--muted)]">Default Model</label>
      <select data-field="model" style=${selectStyle} value=${cfg.model || ""}>
        <option value="">(server default)</option>
        ${modelsSig.value.map((m) => {
					return html`<option key=${m.id} value=${m.id}>${m.displayName || m.id}</option>`;
				})}
      </select>
      <label class="text-xs text-[var(--muted)]">DM Allowlist (one username per line)</label>
      <textarea data-field="allowlist" rows="3"
        style="font-family:var(--font-body);resize:vertical;background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;">${(cfg.allowlist || []).join("\n")}</textarea>
      ${error.value && html`<div class="text-xs text-[var(--error)] channel-error" style="display:block;">${error.value}</div>`}
      <button class="bg-[var(--accent-dim)] text-white border-none px-4 py-2 rounded text-sm cursor-pointer hover:bg-[var(--accent)] transition-colors"
        onClick=${onSave} disabled=${saving.value}>
        ${saving.value ? "Saving\u2026" : "Save Changes"}
      </button>
    </div>
  </${Modal}>`;
}

// ── Main page component ──────────────────────────────────────
function ChannelsPage() {
	useEffect(() => {
		// Use prefetched cache for instant render
		if (S.cachedChannels !== null) channels.value = S.cachedChannels;
		loadChannels();

		var unsub = onEvent("channel", (p) => {
			if (p.kind === "inbound_message" && activeTab.value === "senders" && sendersAccount.value === p.account_id) {
				loadSenders();
			}
		});
		S.setChannelEventUnsub(unsub);

		return () => {
			if (unsub) unsub();
			S.setChannelEventUnsub(null);
		};
	}, []);

	return html`
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-medium text-[var(--text-strong)]">Channels</h2>
        <div style="display:flex;gap:4px;margin-left:12px;">
          <button class="session-action-btn" style=${activeTab.value === "channels" ? "font-weight:600;" : ""}
            onClick=${() => {
							activeTab.value = "channels";
						}}>Channels</button>
          <button class="session-action-btn" style=${activeTab.value === "senders" ? "font-weight:600;" : ""}
            onClick=${() => {
							activeTab.value = "senders";
						}}>Senders</button>
        </div>
        ${
					activeTab.value === "channels" &&
					html`
          <button class="bg-[var(--accent-dim)] text-white border-none px-3 py-1.5 rounded text-xs cursor-pointer hover:bg-[var(--accent)] transition-colors"
            onClick=${() => {
							if (connected.value) showAddModal.value = true;
						}}>+ Add Telegram Bot</button>
        `
				}
      </div>
      ${activeTab.value === "channels" ? html`<${ChannelsTab} />` : html`<${SendersTab} />`}
    </div>
    <${AddChannelModal} />
    <${EditChannelModal} />
    <${ConfirmDialog} />
  `;
}

registerPage(
	"/channels",
	function initChannels(container) {
		container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
		activeTab.value = "channels";
		showAddModal.value = false;
		editingChannel.value = null;
		sendersAccount.value = "";
		senders.value = [];
		render(html`<${ChannelsPage} />`, container);
	},
	function teardownChannels() {
		S.setRefreshChannelsPage(null);
		if (S.channelEventUnsub) {
			S.channelEventUnsub();
			S.setChannelEventUnsub(null);
		}
		var container = S.$("pageContent");
		if (container) render(null, container);
	},
);
