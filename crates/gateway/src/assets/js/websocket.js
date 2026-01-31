// ── WebSocket ─────────────────────────────────────────────────
import * as S from "./state.js";
import { nextId, sendRpc, renderMarkdown, formatTokens } from "./helpers.js";
import { eventListeners } from "./events.js";
import { currentPage, mount } from "./router.js";
import { fetchModels } from "./models.js";
import { fetchSessions, setSessionReplying, setSessionUnread, bumpSessionCount, switchSession } from "./sessions.js";
import { fetchProjects } from "./projects.js";
import { chatAddMsg, chatAddErrorCard, chatAddErrorMsg, removeThinking, renderApprovalCard, stripChannelPrefix, appendChannelFooter, updateTokenBar } from "./chat-ui.js";
import { renderCompactCard } from "./page-chat.js";
import { updateLogsAlert, clearLogsAlert } from "./logs-alert.js";

export function connect() {
  setStatus("connecting", "connecting...");
  var proto = location.protocol === "https:" ? "wss:" : "ws:";
  S.setWs(new WebSocket(proto + "//" + location.host + "/ws"));

  S.ws.onopen = function () {
    var id = nextId();
    S.ws.send(JSON.stringify({
      type: "req", id: id, method: "connect",
      params: {
        minProtocol: 3, maxProtocol: 3,
        client: { id: "web-chat-ui", version: "0.1.0", platform: "browser", mode: "operator" }
      }
    }));
    S.pending[id] = function (frame) {
      var hello = frame.ok && frame.payload;
      if (hello && hello.type === "hello-ok") {
        S.setConnected(true);
        S.setReconnectDelay(1000);
        setStatus("connected", "connected (v" + hello.protocol + ")");
        var now = new Date();
        var ts = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
        chatAddMsg("system", "Connected to moltis gateway v" + hello.server.version + " at " + ts);
        fetchModels();
        fetchSessions();
        fetchProjects();
        sendRpc("logs.status", {}).then(function (res) {
          if (res && res.ok) {
            var p = res.payload || {};
            S.setUnseenErrors(p.unseen_errors || 0);
            S.setUnseenWarns(p.unseen_warns || 0);
            if (currentPage === "/logs") clearLogsAlert();
            else updateLogsAlert();
          }
        });
        mount(currentPage);
      } else {
        setStatus("", "handshake failed");
        var reason = (frame.error && frame.error.message) || "unknown error";
        chatAddMsg("error", "Handshake failed: " + reason);
      }
    };
  };

  S.ws.onmessage = function (evt) {
    var frame;
    try { frame = JSON.parse(evt.data); } catch (e) { return; }

    if (frame.type === "res") {
      var cb = S.pending[frame.id];
      if (cb) { delete S.pending[frame.id]; cb(frame); }
      return;
    }

    if (frame.type === "event") {
      var listeners = eventListeners[frame.event] || [];
      listeners.forEach(function(h) { h(frame.payload || {}); });
      if (frame.event === "chat") {
        var p = frame.payload || {};
        var eventSession = p.sessionKey || S.activeSessionKey;
        var isActive = eventSession === S.activeSessionKey;
        var isChatPage = currentPage === "/";

        if (p.sessionKey && !S.sessions.find(function (s) { return s.key === p.sessionKey; })) {
          fetchSessions();
        }

        if (p.state === "thinking" && isActive && isChatPage) {
          removeThinking();
          var thinkEl = document.createElement("div");
          thinkEl.className = "msg assistant thinking";
          thinkEl.id = "thinkingIndicator";
          var thinkDots = document.createElement("span");
          thinkDots.className = "thinking-dots";
          // Safe: static hardcoded HTML, no user input
          thinkDots.innerHTML = "<span></span><span></span><span></span>";
          thinkEl.appendChild(thinkDots);
          S.chatMsgBox.appendChild(thinkEl);
          S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
        } else if (p.state === "thinking_text" && isActive && isChatPage) {
          var indicator = document.getElementById("thinkingIndicator");
          if (indicator) {
            while (indicator.firstChild) indicator.removeChild(indicator.firstChild);
            var textEl = document.createElement("span");
            textEl.className = "thinking-text";
            textEl.textContent = p.text;
            indicator.appendChild(textEl);
            S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
          }
        } else if (p.state === "thinking_done" && isActive && isChatPage) {
          removeThinking();
        } else if (p.state === "tool_call_start" && isActive && isChatPage) {
          removeThinking();
          var card = document.createElement("div");
          card.className = "msg exec-card running";
          card.id = "tool-" + p.toolCallId;
          var prompt = document.createElement("div");
          prompt.className = "exec-prompt";
          var cmd = (p.toolName === "exec" && p.arguments && p.arguments.command)
            ? p.arguments.command : (p.toolName || "tool");
          var promptChar = document.createElement("span");
          promptChar.className = "exec-prompt-char";
          promptChar.textContent = "$";
          prompt.appendChild(promptChar);
          var cmdSpan = document.createElement("span");
          cmdSpan.textContent = " " + cmd;
          prompt.appendChild(cmdSpan);
          card.appendChild(prompt);
          var spin = document.createElement("div");
          spin.className = "exec-status";
          spin.textContent = "running\u2026";
          card.appendChild(spin);
          S.chatMsgBox.appendChild(card);
          S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
        } else if (p.state === "tool_call_end" && isActive && isChatPage) {
          var toolCard = document.getElementById("tool-" + p.toolCallId);
          if (toolCard) {
            toolCard.className = "msg exec-card " + (p.success ? "exec-ok" : "exec-err");
            var toolSpin = toolCard.querySelector(".exec-status");
            if (toolSpin) toolSpin.remove();
            if (p.success && p.result) {
              var out = (p.result.stdout || "").replace(/\n+$/, "");
              S.setLastToolOutput(out);
              if (out) {
                var outEl = document.createElement("pre");
                outEl.className = "exec-output";
                outEl.textContent = out;
                toolCard.appendChild(outEl);
              }
              var stderrText = (p.result.stderr || "").replace(/\n+$/, "");
              if (stderrText) {
                var errEl = document.createElement("pre");
                errEl.className = "exec-output exec-stderr";
                errEl.textContent = stderrText;
                toolCard.appendChild(errEl);
              }
              if (p.result.exit_code !== undefined && p.result.exit_code !== 0) {
                var codeEl = document.createElement("div");
                codeEl.className = "exec-exit";
                codeEl.textContent = "exit " + p.result.exit_code;
                toolCard.appendChild(codeEl);
              }
            } else if (!p.success && p.error && p.error.detail) {
              var errMsg = document.createElement("div");
              errMsg.className = "exec-error-detail";
              errMsg.textContent = p.error.detail;
              toolCard.appendChild(errMsg);
            }
          }
        } else if (p.state === "channel_user" && isChatPage) {
          if (p.sessionKey && p.sessionKey !== S.activeSessionKey) {
            switchSession(p.sessionKey);
          }
          isActive = p.sessionKey ? (p.sessionKey === S.activeSessionKey) : isActive;
          if (!isActive) return;
          var cleanText = stripChannelPrefix(p.text || "");
          var el = chatAddMsg("user", renderMarkdown(cleanText), true);
          if (el && p.channel) {
            appendChannelFooter(el, p.channel);
          }
        } else if (p.state === "delta" && p.text && isActive && isChatPage) {
          removeThinking();
          if (!S.streamEl) {
            S.setStreamText("");
            S.setStreamEl(document.createElement("div"));
            S.streamEl.className = "msg assistant";
            S.chatMsgBox.appendChild(S.streamEl);
          }
          S.setStreamText(S.streamText + p.text);
          // Safe: renderMarkdown calls esc() first
          S.streamEl.innerHTML = renderMarkdown(S.streamText);
          S.chatMsgBox.scrollTop = S.chatMsgBox.scrollHeight;
        } else if (p.state === "final") {
          bumpSessionCount(eventSession, 1);
          setSessionReplying(eventSession, false);
          if (!isActive) {
            setSessionUnread(eventSession, true);
          }
          if (isActive && isChatPage) {
            removeThinking();
            var isEcho = S.lastToolOutput && p.text
              && p.text.replace(/[`\s]/g, "").indexOf(S.lastToolOutput.replace(/\s/g, "").substring(0, 80)) !== -1;
            var msgEl = null;
            if (!isEcho) {
              if (p.text && S.streamEl) {
                // Safe: renderMarkdown calls esc() first
                S.streamEl.innerHTML = renderMarkdown(p.text);
                msgEl = S.streamEl;
              } else if (p.text && !S.streamEl) {
                msgEl = chatAddMsg("assistant", renderMarkdown(p.text), true);
              }
            } else if (S.streamEl) {
              S.streamEl.remove();
            }
            if (msgEl && p.model) {
              var footer = document.createElement("div");
              footer.className = "msg-model-footer";
              var footerText = p.provider ? p.provider + " / " + p.model : p.model;
              if (p.inputTokens || p.outputTokens) {
                footerText += " \u00b7 " + formatTokens(p.inputTokens || 0) + " in / " + formatTokens(p.outputTokens || 0) + " out";
              }
              footer.textContent = footerText;
              msgEl.appendChild(footer);
            }
            if (p.inputTokens || p.outputTokens) {
              S.sessionTokens.input += (p.inputTokens || 0);
              S.sessionTokens.output += (p.outputTokens || 0);
              updateTokenBar();
            }
            S.setStreamEl(null);
            S.setStreamText("");
            S.setLastToolOutput("");
          }
        } else if (p.state === "auto_compact") {
          if (isActive && isChatPage) {
            if (p.phase === "start") {
              chatAddMsg("system", "Compacting conversation (context limit reached)\u2026");
            } else if (p.phase === "done") {
              if (S.chatMsgBox && S.chatMsgBox.lastChild) S.chatMsgBox.removeChild(S.chatMsgBox.lastChild);
              renderCompactCard(p);
              S.setSessionTokens({ input: 0, output: 0 });
              updateTokenBar();
            } else if (p.phase === "error") {
              if (S.chatMsgBox && S.chatMsgBox.lastChild) S.chatMsgBox.removeChild(S.chatMsgBox.lastChild);
              chatAddMsg("error", "Auto-compact failed: " + (p.error || "unknown error"));
            }
          }
        } else if (p.state === "error") {
          setSessionReplying(eventSession, false);
          if (isActive && isChatPage) {
            removeThinking();
            if (p.error && p.error.title) {
              chatAddErrorCard(p.error);
            } else {
              chatAddErrorMsg(p.message || "unknown");
            }
            S.setStreamEl(null);
            S.setStreamText("");
          }
        }
      }
      if (frame.event === "exec.approval.requested") {
        var ap = frame.payload || {};
        renderApprovalCard(ap.requestId, ap.command);
      }
      if (frame.event === "logs.entry") {
        var logPayload = frame.payload || {};
        if (S.logsEventHandler) S.logsEventHandler(logPayload);
        if (currentPage !== "/logs") {
          var ll = (logPayload.level || "").toUpperCase();
          if (ll === "ERROR") { S.setUnseenErrors(S.unseenErrors + 1); updateLogsAlert(); }
          else if (ll === "WARN") { S.setUnseenWarns(S.unseenWarns + 1); updateLogsAlert(); }
        }
      }
      return;
    }
  };

  S.ws.onclose = function () {
    S.setConnected(false);
    setStatus("", "disconnected \u2014 reconnecting\u2026");
    S.setStreamEl(null);
    S.setStreamText("");
    scheduleReconnect();
  };

  S.ws.onerror = function () {};
}

function setStatus(state, text) {
  var dot = S.$("statusDot");
  var sText = S.$("statusText");
  dot.className = "status-dot " + state;
  sText.textContent = text;
  var sendBtn = S.$("sendBtn");
  if (sendBtn) sendBtn.disabled = state !== "connected";
}

var reconnectTimer = null;

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(function () {
    reconnectTimer = null;
    S.setReconnectDelay(Math.min(S.reconnectDelay * 1.5, 5000));
    connect();
  }, S.reconnectDelay);
}

document.addEventListener("visibilitychange", function () {
  if (!document.hidden && !S.connected) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
    S.setReconnectDelay(1000);
    connect();
  }
});
