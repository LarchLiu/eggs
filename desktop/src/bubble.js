(() => {
  const tauri = (typeof window !== "undefined" && window.__TAURI__) || null;
  if (!tauri) {
    console.error("Tauri globals missing; this page must run inside the Tauri webview");
    return;
  }
  const { invoke } = tauri.core;
  const { listen } = tauri.event;
  let bubbleLayout = {
    width: 236,
    min_height: 20,
    max_height: 84,
  };

  function readBubbleId() {
    const raw = window.location.hash.replace(/^#/, "");
    try {
      return decodeURIComponent(raw);
    } catch {
      return raw;
    }
  }

  function appendHighlightedLine(target, line) {
    const text = String(line || "");
    const match = text.match(/^(?:(\d{2}:\d{2})\s+)?([^:\n]{1,40}):(.*)$/s);
    if (!match) {
      target.appendChild(document.createTextNode(text));
      return;
    }

    const [, timePrefix, label, rest] = match;
    const normalized = label.trim();
    const labelClass = labelClassFor(normalized);
    if (!labelClass) {
      target.appendChild(document.createTextNode(text));
      return;
    }

    if (timePrefix) {
      target.appendChild(document.createTextNode(`${timePrefix} `));
    }
    const labelEl = document.createElement("span");
    labelEl.className = `event-label ${labelClass}`;
    labelEl.textContent = normalized;
    target.appendChild(labelEl);
    target.appendChild(document.createTextNode(`:${rest}`));
  }

  function labelClassFor(label) {
    if (label === "Permission") return "permission-label";
    if (label === "Agent") return "agent-label";
    if (isToolLabel(label)) return "tool-label";
    return "";
  }

  function isToolLabel(label) {
    return new Set([
      "Read file",
      "Write file",
      "Search",
      "Web search",
      "Web fetch",
      "MCP search",
      "MCP fetch",
      "MCP tool",
      "Run command",
      "Tool done",
    ]).has(label);
  }

  function renderTextLines(text, lines) {
    text.replaceChildren();
    lines.forEach((line, idx) => {
      if (idx > 0) {
        text.appendChild(document.createTextNode("\n"));
      }
      appendHighlightedLine(text, line);
    });
  }

  function render(root, meta, text, payload) {
    const source = (payload && payload.source) || "hook";
    const mode = payload && payload.mode ? payload.mode : "hook";
    root.className = "bubble";
    root.classList.add(source.replace(/_/g, "-"));
    root.classList.add(mode);
    meta.textContent = "";
    const lines = Array.isArray(payload && payload.messages) ? payload.messages : [];
    const times = Array.isArray(payload && payload.message_times) ? payload.message_times : [];
    if ((mode === "chat" || mode === "hook") && lines.length > 0) {
      const merged = lines.map((line, idx) => {
        const ts = typeof times[idx] === "number" ? times[idx] : null;
        if (!ts) return line;
        const d = new Date(ts);
        const hh = String(d.getHours()).padStart(2, "0");
        const mm = String(d.getMinutes()).padStart(2, "0");
        return `${hh}:${mm} ${line}`;
      });
      renderTextLines(text, merged.reverse());
    } else {
      renderTextLines(text, [(payload && payload.text) || ""]);
    }
    applyDynamicHeight(root, text).catch(() => {});
  }

  async function applyDynamicHeight(root, text) {
    const computed = window.getComputedStyle(text);
    const lineHeight = resolveLineHeightPx(text, computed);
    const maxTextHeight = lineHeight * 3;

    text.style.maxHeight = "none";
    text.style.overflowY = "hidden";
    const naturalTextHeight = text.scrollHeight;

    const visibleTextHeight = Math.min(naturalTextHeight, maxTextHeight);
    text.style.maxHeight = `${Math.ceil(visibleTextHeight)}px`;
    const hasScroll = naturalTextHeight > maxTextHeight;
    text.style.overflowY = hasScroll ? "auto" : "hidden";
    text.classList.toggle("has-scroll", hasScroll);

    const rootStyle = window.getComputedStyle(root);
    const paddingTop = parseFloat(rootStyle.paddingTop) || 0;
    const paddingBottom = parseFloat(rootStyle.paddingBottom) || 0;
    const borderTop = parseFloat(rootStyle.borderTopWidth) || 0;
    const borderBottom = parseFloat(rootStyle.borderBottomWidth) || 0;
    const chromeHeight = paddingTop + paddingBottom + borderTop + borderBottom;
    const desiredHeight = Math.min(
      bubbleLayout.max_height,
      Math.max(bubbleLayout.min_height, Math.ceil(chromeHeight + visibleTextHeight)),
    );

    document.documentElement.style.height = `${desiredHeight}px`;
    document.body.style.height = `${desiredHeight}px`;
    const bubbleId = readBubbleId();
    await invoke("bubble_resize", { bubbleId, height: desiredHeight });
  }

  function resolveLineHeightPx(textEl, computed) {
    const parsed = parseFloat(computed.lineHeight);
    if (Number.isFinite(parsed) && parsed > 0) {
      return parsed;
    }
    const probe = document.createElement("span");
    probe.textContent = "A";
    probe.style.position = "absolute";
    probe.style.visibility = "hidden";
    probe.style.pointerEvents = "none";
    probe.style.whiteSpace = "pre";
    probe.style.fontFamily = computed.fontFamily;
    probe.style.fontSize = computed.fontSize;
    probe.style.fontWeight = computed.fontWeight;
    probe.style.letterSpacing = computed.letterSpacing;
    probe.style.lineHeight = computed.lineHeight;
    textEl.appendChild(probe);
    const measured = probe.getBoundingClientRect().height;
    probe.remove();
    if (Number.isFinite(measured) && measured > 0) {
      return measured;
    }
    const fontSize = parseFloat(computed.fontSize);
    if (Number.isFinite(fontSize) && fontSize > 0) {
      const ratio =
        parseFloat(
          getComputedStyle(document.documentElement).getPropertyValue("--line-h"),
        ) || 1.25;
      return fontSize * ratio;
    }
    return 16;
  }

  async function main() {
    const bubbleId = readBubbleId();
    if (!bubbleId) {
      console.error("bubble.html opened without a bubble id fragment");
      return;
    }

    try {
      const cfg = await invoke("bubble_constraints");
      if (cfg && typeof cfg === "object") {
        bubbleLayout = {
          width: Number(cfg.width) || bubbleLayout.width,
          min_height: Number(cfg.min_height) || bubbleLayout.min_height,
          max_height: Number(cfg.max_height) || bubbleLayout.max_height,
        };
      }
    } catch (e) {
      console.warn("bubble_constraints failed; using defaults", e);
    }

    let init;
    try {
      init = await invoke("get_bubble_init", { bubbleId });
    } catch (e) {
      console.error(`get_bubble_init(${bubbleId}) failed:`, e);
      return;
    }

    const root = document.getElementById("bubble");
    const meta = document.getElementById("meta");
    const text = document.getElementById("text");
    const closeBtn = document.getElementById("close");
    render(root, meta, text, init || {});

    root.addEventListener("mouseenter", () => {
      invoke("bubble_hover", { bubbleId, hovering: true }).catch(() => {});
    });
    root.addEventListener("mouseleave", () => {
      invoke("bubble_hover", { bubbleId, hovering: false }).catch(() => {});
    });
    if (closeBtn) {
      closeBtn.addEventListener("click", (evt) => {
        evt.stopPropagation();
        if (root.classList.contains("chat") || root.classList.contains("hook")) {
          invoke("bubble_dismiss", { bubbleId }).catch(() => {});
        }
      });
    }

    await listen("bubble-update", (evt) => {
      const payload = evt && evt.payload ? evt.payload : {};
      if (payload.id && payload.id !== bubbleId) return;
      render(root, meta, text, payload);
    });
  }

  main();
})();
