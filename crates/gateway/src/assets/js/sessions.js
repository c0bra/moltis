// ── Sessions: list, switch, status helpers ──────────────────

import {
	appendChannelFooter,
	chatAddMsg,
	highlightAndScroll,
	removeThinking,
	scrollChatToBottom,
	stripChannelPrefix,
	updateTokenBar,
} from "./chat-ui.js";
import { formatTokens, renderMarkdown, sendRpc } from "./helpers.js";
import { makeChatIcon, makeTelegramIcon } from "./icons.js";
import { updateSessionProjectSelect } from "./project-combo.js";
import { currentPrefix, navigate, sessionPath } from "./router.js";
import { updateSandboxUI } from "./sandbox.js";
import * as S from "./state.js";

// ── Fetch & render ──────────────────────────────────────────

export function fetchSessions() {
	sendRpc("sessions.list", {}).then((res) => {
		if (!res?.ok) return;
		S.setSessions(res.payload || []);
		renderSessionList();
	});
}

function createSessionIcon(s) {
	var iconWrap = document.createElement("span");
	iconWrap.className = "session-icon";
	var isTelegram = false;
	if (s.channelBinding) {
		try {
			var binding = JSON.parse(s.channelBinding);
			if (binding.channel_type === "telegram") isTelegram = true;
		} catch (_e) {
			/* ignore bad JSON */
		}
	}
	var icon = isTelegram ? makeTelegramIcon() : makeChatIcon();
	iconWrap.appendChild(icon);
	if (isTelegram) {
		iconWrap.style.color = s.activeChannel ? "var(--accent)" : "var(--muted)";
		iconWrap.style.opacity = s.activeChannel ? "1" : "0.5";
		iconWrap.title = s.activeChannel ? "Active Telegram session" : "Telegram session (inactive)";
	} else {
		iconWrap.style.color = "var(--muted)";
	}
	var spinner = document.createElement("span");
	spinner.className = "session-spinner";
	iconWrap.appendChild(spinner);
	return iconWrap;
}

function createSessionMeta(s) {
	var meta = document.createElement("div");
	meta.className = "session-meta";
	meta.setAttribute("data-session-key", s.key);
	var count = s.messageCount || 0;
	var metaText = `${count} msg${count !== 1 ? "s" : ""}`;
	if (s.worktree_branch) {
		metaText += ` \u00b7 \u2387 ${s.worktree_branch}`;
	}
	meta.textContent = metaText;
	return meta;
}

function createSessionActions(s, sessionList) {
	var actions = document.createElement("div");
	actions.className = "session-actions";

	if (s.key !== "main") {
		if (!s.channelBinding) {
			var renameBtn = document.createElement("button");
			renameBtn.className = "session-action-btn";
			renameBtn.textContent = "\u270F";
			renameBtn.title = "Rename";
			renameBtn.addEventListener("click", (e) => {
				e.stopPropagation();
				var newLabel = prompt("Rename session:", s.label || s.key);
				if (newLabel !== null) {
					sendRpc("sessions.patch", { key: s.key, label: newLabel }).then(fetchSessions);
				}
			});
			actions.appendChild(renameBtn);
		}

		var deleteBtn = document.createElement("button");
		deleteBtn.className = "session-action-btn session-delete";
		deleteBtn.textContent = "\u2715";
		deleteBtn.title = "Delete";
		deleteBtn.addEventListener("click", (e) => {
			e.stopPropagation();
			var metaEl = sessionList.querySelector(`.session-meta[data-session-key="${s.key}"]`);
			var msgCount = metaEl ? parseInt(metaEl.textContent, 10) || 0 : s.messageCount || 0;
			if (msgCount > 0 && !confirm("Delete this session?")) return;
			sendRpc("sessions.delete", { key: s.key }).then((res) => {
				if (res && !res.ok && res.error && res.error.indexOf("uncommitted changes") !== -1) {
					if (confirm("Worktree has uncommitted changes. Force delete?")) {
						sendRpc("sessions.delete", { key: s.key, force: true }).then(() => {
							if (S.activeSessionKey === s.key) switchSession("main");
							fetchSessions();
						});
					}
					return;
				}
				if (S.activeSessionKey === s.key) switchSession("main");
				fetchSessions();
			});
		});
		actions.appendChild(deleteBtn);
	}
	return actions;
}

export function renderSessionList() {
	var sessionList = S.$("sessionList");
	sessionList.textContent = "";
	var filtered = S.sessions;
	if (S.projectFilterId) {
		filtered = S.sessions.filter((s) => s.projectId === S.projectFilterId);
	}
	var tpl = document.getElementById("tpl-session-item");
	filtered.forEach((s) => {
		var frag = tpl.content.cloneNode(true);
		var item = frag.firstElementChild;
		item.className = `session-item${s.key === S.activeSessionKey ? " active" : ""}`;
		item.setAttribute("data-session-key", s.key);

		var iconWrap = item.querySelector(".session-icon");
		iconWrap.replaceWith(createSessionIcon(s));

		item.querySelector("[data-label-text]").textContent = s.label || s.key;

		var meta = item.querySelector(".session-meta");
		var newMeta = createSessionMeta(s);
		meta.replaceWith(newMeta);

		var actionsSlot = item.querySelector(".session-actions");
		actionsSlot.replaceWith(createSessionActions(s, sessionList));

		item.addEventListener("click", () => {
			if (currentPrefix !== "/chats") {
				navigate(sessionPath(s.key));
			} else {
				switchSession(s.key);
			}
		});

		sessionList.appendChild(item);
	});
}

// ── Braille spinner for active sessions ─────────────────────
var spinnerFrames = [
	"\u280B",
	"\u2819",
	"\u2839",
	"\u2838",
	"\u283C",
	"\u2834",
	"\u2826",
	"\u2827",
	"\u2807",
	"\u280F",
];
var spinnerIndex = 0;
setInterval(() => {
	spinnerIndex = (spinnerIndex + 1) % spinnerFrames.length;
	var els = document.querySelectorAll(".session-item.replying .session-spinner");
	for (var el of els) el.textContent = spinnerFrames[spinnerIndex];
}, 80);

// ── Status helpers ──────────────────────────────────────────

export function setSessionReplying(key, replying) {
	var sessionList = S.$("sessionList");
	var el = sessionList.querySelector(`.session-item[data-session-key="${key}"]`);
	if (el) el.classList.toggle("replying", replying);
}

export function setSessionUnread(key, unread) {
	var sessionList = S.$("sessionList");
	var el = sessionList.querySelector(`.session-item[data-session-key="${key}"]`);
	if (el) el.classList.toggle("unread", unread);
}

export function bumpSessionCount(key, increment) {
	var sessionList = S.$("sessionList");
	var el = sessionList.querySelector(`.session-meta[data-session-key="${key}"]`);
	if (!el) return;
	var current = parseInt(el.textContent, 10) || 0;
	var next = current + increment;
	el.textContent = `${next} msg${next !== 1 ? "s" : ""}`;
}

// ── New session button ──────────────────────────────────────
var newSessionBtn = S.$("newSessionBtn");
newSessionBtn.addEventListener("click", () => {
	var key = `session:${crypto.randomUUID()}`;
	navigate(sessionPath(key));
});

// ── Switch session ──────────────────────────────────────────

function restoreSessionState(entry, projectId) {
	var effectiveProjectId = entry.projectId || projectId || "";
	S.setActiveProjectId(effectiveProjectId);
	localStorage.setItem("moltis-project", S.activeProjectId);
	updateSessionProjectSelect(S.activeProjectId);
	if (entry.model && S.models.length > 0) {
		var found = S.models.find((m) => m.id === entry.model);
		if (found) {
			S.setSelectedModelId(found.id);
			if (S.modelComboLabel) S.modelComboLabel.textContent = found.displayName || found.id;
			localStorage.setItem("moltis-model", found.id);
		}
	}
	updateSandboxUI(entry.sandbox_enabled !== false);
}

function renderHistoryUserMessage(msg) {
	var userContent = msg.content || "";
	if (msg.channel) userContent = stripChannelPrefix(userContent);
	var userEl = chatAddMsg("user", renderMarkdown(userContent), true);
	if (userEl && msg.channel) appendChannelFooter(userEl, msg.channel);
	return userEl;
}

function createModelFooter(msg) {
	var ft = document.createElement("div");
	ft.className = "msg-model-footer";
	var ftText = msg.provider ? `${msg.provider} / ${msg.model}` : msg.model;
	if (msg.inputTokens || msg.outputTokens) {
		ftText += ` \u00b7 ${formatTokens(msg.inputTokens || 0)} in / ${formatTokens(msg.outputTokens || 0)} out`;
	}
	ft.textContent = ftText;
	return ft;
}

function renderHistoryAssistantMessage(msg) {
	var el = chatAddMsg("assistant", renderMarkdown(msg.content || ""), true);
	if (el && msg.model) {
		el.appendChild(createModelFooter(msg));
	}
	if (msg.inputTokens || msg.outputTokens) {
		S.sessionTokens.input += msg.inputTokens || 0;
		S.sessionTokens.output += msg.outputTokens || 0;
	}
	return el;
}

function makeThinkingDots() {
	var tpl = document.getElementById("tpl-thinking-dots");
	return tpl.content.cloneNode(true).firstElementChild;
}

function postHistoryLoadActions(key, searchContext, msgEls, sessionList) {
	sendRpc("chat.context", {}).then((ctxRes) => {
		if (ctxRes?.ok && ctxRes.payload && ctxRes.payload.tokenUsage) {
			S.setSessionContextWindow(ctxRes.payload.tokenUsage.contextWindow || 0);
		}
		updateTokenBar();
	});
	updateTokenBar();

	if (searchContext?.query && S.chatMsgBox) {
		highlightAndScroll(msgEls, searchContext.messageIndex, searchContext.query);
	} else {
		scrollChatToBottom();
	}

	var item = sessionList.querySelector(`.session-item[data-session-key="${key}"]`);
	if (item?.classList.contains("replying") && S.chatMsgBox) {
		removeThinking();
		var thinkEl = document.createElement("div");
		thinkEl.className = "msg assistant thinking";
		thinkEl.id = "thinkingIndicator";
		thinkEl.appendChild(makeThinkingDots());
		S.chatMsgBox.appendChild(thinkEl);
		scrollChatToBottom();
	}
	if (!sessionList.querySelector(`.session-meta[data-session-key="${key}"]`)) {
		fetchSessions();
	}
}

export function switchSession(key, searchContext, projectId) {
	var sessionList = S.$("sessionList");
	S.setActiveSessionKey(key);
	localStorage.setItem("moltis-session", key);
	history.replaceState(null, "", sessionPath(key));
	if (S.chatMsgBox) S.chatMsgBox.textContent = "";
	S.setStreamEl(null);
	S.setStreamText("");
	S.setLastHistoryIndex(-1);
	S.setSessionTokens({ input: 0, output: 0 });
	S.setSessionContextWindow(0);
	updateTokenBar();

	var items = sessionList.querySelectorAll(".session-item");
	items.forEach((el) => {
		var isTarget = el.getAttribute("data-session-key") === key;
		el.classList.toggle("active", isTarget);
		if (isTarget) el.classList.remove("unread");
	});

	S.setSessionSwitchInProgress(true);
	var switchParams = { key: key };
	if (projectId) switchParams.project_id = projectId;
	sendRpc("sessions.switch", switchParams).then((res) => {
		if (res?.ok && res.payload) {
			var entry = res.payload.entry || {};
			restoreSessionState(entry, projectId);
			var history = res.payload.history || [];
			var msgEls = [];
			S.setSessionTokens({ input: 0, output: 0 });
			S.setChatBatchLoading(true);
			history.forEach((msg) => {
				if (msg.role === "user") {
					msgEls.push(renderHistoryUserMessage(msg));
				} else if (msg.role === "assistant") {
					msgEls.push(renderHistoryAssistantMessage(msg));
				} else {
					msgEls.push(null);
				}
			});
			S.setChatBatchLoading(false);
			S.setLastHistoryIndex(history.length > 0 ? history.length - 1 : -1);
			S.setSessionSwitchInProgress(false);
			postHistoryLoadActions(key, searchContext, msgEls, sessionList);
		} else {
			S.setSessionSwitchInProgress(false);
		}
	});
}
