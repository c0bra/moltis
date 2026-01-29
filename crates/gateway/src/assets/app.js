(function () {
  "use strict";

  var $ = function (id) { return document.getElementById(id); };
  var msgBox = $("messages");
  var input = $("chatInput");
  var sendBtn = $("sendBtn");
  var dot = $("statusDot");
  var sText = $("statusText");
  var methodsToggle = $("methodsToggle");
  var methodsPanel = $("methodsPanel");
  var rpcMethod = $("rpcMethod");
  var rpcParams = $("rpcParams");
  var rpcSend = $("rpcSend");
  var rpcResult = $("rpcResult");

  var modelSelect = $("modelSelect");

  var ws = null;
  var reqId = 0;
  var connected = false;
  var reconnectDelay = 1000;
  var streamEl = null;
  var streamText = "";
  var pending = {};
  var models = [];

  // ── Theme ────────────────────────────────────────────────────────

  function getSystemTheme() {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  function applyTheme(mode) {
    var resolved = mode === "system" ? getSystemTheme() : mode;
    document.documentElement.setAttribute("data-theme", resolved);
    document.documentElement.style.colorScheme = resolved;
    updateThemeButtons(mode);
  }

  function updateThemeButtons(activeMode) {
    var buttons = document.querySelectorAll(".theme-btn");
    buttons.forEach(function (btn) {
      btn.classList.toggle("active", btn.getAttribute("data-theme-val") === activeMode);
    });
  }

  function initTheme() {
    var saved = localStorage.getItem("moltis-theme") || "system";
    applyTheme(saved);

    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", function () {
      var current = localStorage.getItem("moltis-theme") || "system";
      if (current === "system") applyTheme("system");
    });

    $("themeToggle").addEventListener("click", function (e) {
      var btn = e.target.closest(".theme-btn");
      if (!btn) return;
      var mode = btn.getAttribute("data-theme-val");
      localStorage.setItem("moltis-theme", mode);
      applyTheme(mode);
    });
  }

  initTheme();

  // ── Helpers ──────────────────────────────────────────────────────

  function nextId() { return "ui-" + (++reqId); }

  function setStatus(state, text) {
    dot.className = "status-dot " + state;
    sText.textContent = text;
    sendBtn.disabled = state !== "connected";
  }

  // Escape HTML entities to prevent XSS — all user/LLM text is escaped
  // before being processed by renderMarkdown, which produces safe HTML
  // from the already-escaped input.
  function esc(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  }

  // Simple markdown: input is ALREADY HTML-escaped via esc(), so the
  // resulting HTML only contains tags we explicitly create.
  function renderMarkdown(raw) {
    var s = esc(raw);
    s = s.replace(/```(\w*)\n([\s\S]*?)```/g, function (_, lang, code) {
      return "<pre><code>" + code + "</code></pre>";
    });
    s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
    s = s.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
    return s;
  }

  // Sets element content. When isHtml is true the content MUST have
  // been produced by renderMarkdown (which escapes first).
  function addMsg(cls, content, isHtml) {
    var el = document.createElement("div");
    el.className = "msg " + cls;
    if (isHtml) {
      el.innerHTML = content; // safe: content is escaped via esc() then formatted
    } else {
      el.textContent = content;
    }
    msgBox.appendChild(el);
    msgBox.scrollTop = msgBox.scrollHeight;
    return el;
  }

  function removeThinking() {
    var el = document.getElementById("thinkingIndicator");
    if (el) el.remove();
  }

  // ── WebSocket ────────────────────────────────────────────────────

  function connect() {
    setStatus("connecting", "connecting...");
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(proto + "//" + location.host + "/ws");

    ws.onopen = function () {
      var id = nextId();
      ws.send(JSON.stringify({
        type: "req", id: id, method: "connect",
        params: {
          minProtocol: 3, maxProtocol: 3,
          client: { id: "web-chat-ui", version: "0.1.0", platform: "browser", mode: "operator" }
        }
      }));
      pending[id] = function (frame) {
        var hello = frame.ok && frame.payload;
        if (hello && hello.type === "hello-ok") {
          connected = true;
          reconnectDelay = 1000;
          setStatus("connected", "connected (v" + hello.protocol + ")");
          var now = new Date();
          var ts = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
          addMsg("system", "Connected to moltis gateway v" + hello.server.version + " at " + ts);
          fetchModels();
        } else {
          setStatus("", "handshake failed");
          var reason = (frame.error && frame.error.message) || "unknown error";
          addMsg("error", "Handshake failed: " + reason);
        }
      };
    };

    ws.onmessage = function (evt) {
      var frame;
      try { frame = JSON.parse(evt.data); } catch (e) { return; }

      if (frame.type === "res") {
        var cb = pending[frame.id];
        if (cb) { delete pending[frame.id]; cb(frame); }
        return;
      }

      if (frame.type === "event") {
        if (frame.event === "chat") {
          var p = frame.payload || {};
          if (p.state === "thinking") {
            removeThinking();
            var thinkEl = document.createElement("div");
            thinkEl.className = "msg assistant thinking";
            thinkEl.id = "thinkingIndicator";
            // Safe: static hardcoded content, no user input
            var dots = document.createElement("span");
            dots.className = "thinking-dots";
            dots.innerHTML = "<span></span><span></span><span></span>";
            thinkEl.appendChild(dots);
            msgBox.appendChild(thinkEl);
            msgBox.scrollTop = msgBox.scrollHeight;
          } else if (p.state === "thinking_done") {
            removeThinking();
          } else if (p.state === "tool_call_start") {
            removeThinking();
            var toolStartEl = document.createElement("div");
            toolStartEl.className = "msg system tool-event";
            // Safe: toolName comes from server-registered tool names, not user input,
            // and is set via textContent which never interprets HTML.
            toolStartEl.textContent = "\u2699 Running: " + (p.toolName || "tool") + "\u2026";
            toolStartEl.id = "tool-" + p.toolCallId;
            msgBox.appendChild(toolStartEl);
            msgBox.scrollTop = msgBox.scrollHeight;
          } else if (p.state === "tool_call_end") {
            var toolDoneEl = document.getElementById("tool-" + p.toolCallId);
            if (toolDoneEl) {
              toolDoneEl.textContent = (p.success ? "\u2713" : "\u2717") + " " + (p.toolName || "tool");
              toolDoneEl.className = "msg system tool-event " + (p.success ? "tool-ok" : "tool-err");
            }
          } else if (p.state === "delta" && p.text) {
            removeThinking();
            if (!streamEl) {
              streamText = "";
              streamEl = document.createElement("div");
              streamEl.className = "msg assistant";
              msgBox.appendChild(streamEl);
            }
            streamText += p.text;
            // Safe: renderMarkdown calls esc() first to escape all HTML entities,
            // then only adds our own formatting tags (pre, code, strong)
            streamEl.innerHTML = renderMarkdown(streamText);
            msgBox.scrollTop = msgBox.scrollHeight;
          } else if (p.state === "final") {
            removeThinking();
            if (p.text && streamEl) {
              // Safe: renderMarkdown escapes via esc() before formatting
              streamEl.innerHTML = renderMarkdown(p.text);
            } else if (p.text && !streamEl) {
              addMsg("assistant", renderMarkdown(p.text), true);
            }
            streamEl = null;
            streamText = "";
          } else if (p.state === "error") {
            removeThinking();
            addMsg("error", "Chat error: " + (p.message || "unknown"));
            streamEl = null;
            streamText = "";
          }
        }
        return;
      }
    };

    ws.onclose = function () {
      connected = false;
      setStatus("", "disconnected — reconnecting…");
      streamEl = null;
      streamText = "";
      scheduleReconnect();
    };

    ws.onerror = function () {};
  }

  var reconnectTimer = null;

  function scheduleReconnect() {
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(function () {
      reconnectTimer = null;
      reconnectDelay = Math.min(reconnectDelay * 1.5, 5000);
      connect();
    }, reconnectDelay);
  }

  // Reconnect immediately when the tab becomes visible again.
  document.addEventListener("visibilitychange", function () {
    if (!document.hidden && !connected) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
      reconnectDelay = 1000;
      connect();
    }
  });

  function fetchModels() {
    sendRpc("models.list", {}).then(function (res) {
      if (!res || !res.ok) return;
      models = res.payload || [];
      var saved = localStorage.getItem("moltis-model") || "";
      modelSelect.textContent = "";
      if (models.length === 0) {
        var opt = document.createElement("option");
        opt.value = "";
        opt.textContent = "no models";
        modelSelect.appendChild(opt);
        modelSelect.classList.add("hidden");
        return;
      }
      models.forEach(function (m) {
        var opt = document.createElement("option");
        opt.value = m.id;
        opt.textContent = m.displayName || m.id;
        if (m.id === saved) opt.selected = true;
        modelSelect.appendChild(opt);
      });
      modelSelect.classList.remove("hidden");
    });
  }

  modelSelect.addEventListener("change", function () {
    localStorage.setItem("moltis-model", modelSelect.value);
  });

  function sendRpc(method, params) {
    return new Promise(function (resolve) {
      var id = nextId();
      pending[id] = resolve;
      ws.send(JSON.stringify({ type: "req", id: id, method: method, params: params }));
    });
  }

  function sendChat() {
    var text = input.value.trim();
    if (!text || !connected) return;
    input.value = "";
    autoResize();
    addMsg("user", renderMarkdown(text), true);
    var chatParams = { text: text };
    var selectedModel = modelSelect.value;
    if (selectedModel) chatParams.model = selectedModel;
    sendRpc("chat.send", chatParams).then(function (res) {
      if (res && !res.ok && res.error) {
        addMsg("error", res.error.message || "Request failed");
      }
    });
  }

  function autoResize() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 120) + "px";
  }

  // ── Event listeners ──────────────────────────────────────────────

  input.addEventListener("input", autoResize);
  input.addEventListener("keydown", function (e) {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendChat(); }
  });
  sendBtn.addEventListener("click", sendChat);

  methodsToggle.addEventListener("click", function () {
    methodsPanel.classList.toggle("hidden");
  });

  rpcSend.addEventListener("click", function () {
    var method = rpcMethod.value.trim();
    if (!method || !connected) return;
    var params;
    var raw = rpcParams.value.trim();
    if (raw) {
      try { params = JSON.parse(raw); } catch (e) {
        rpcResult.textContent = "Invalid JSON: " + e.message;
        return;
      }
    }
    rpcResult.textContent = "calling...";
    sendRpc(method, params).then(function (res) {
      rpcResult.textContent = JSON.stringify(res, null, 2);
    });
  });

  connect();
  input.focus();
})();
