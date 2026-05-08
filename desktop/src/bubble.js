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

  function render(root, meta, text, payload) {
    const source = (payload && payload.source) || "hook";
    const mode = payload && payload.mode ? payload.mode : "hook";
    root.className = "bubble";
    root.classList.add(source.replace(/_/g, "-"));
    root.classList.add(mode);
    meta.textContent = "";
    const lines = Array.isArray(payload && payload.messages) ? payload.messages : [];
    const times = Array.isArray(payload && payload.message_times) ? payload.message_times : [];
    if (mode === "chat" && lines.length > 0) {
      const merged = lines.map((line, idx) => {
        const ts = typeof times[idx] === "number" ? times[idx] : null;
        if (!ts) return line;
        const d = new Date(ts);
        const hh = String(d.getHours()).padStart(2, "0");
        const mm = String(d.getMinutes()).padStart(2, "0");
        return `${hh}:${mm} ${line}`;
      });
      text.textContent = merged.reverse().join("\n");
    } else {
      text.textContent = (payload && payload.text) || "";
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
      return fontSize * 1.2;
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
    render(root, meta, text, init || {});

    root.addEventListener("mouseenter", () => {
      invoke("bubble_hover", { bubbleId, hovering: true }).catch(() => {});
    });
    root.addEventListener("mouseleave", () => {
      invoke("bubble_hover", { bubbleId, hovering: false }).catch(() => {});
    });
    root.addEventListener("click", () => {
      const mode = root.classList.contains("chat") ? "chat" : "hook";
      if (mode === "chat") {
        invoke("bubble_dismiss", { bubbleId }).catch(() => {});
      }
    });

    await listen("bubble-update", (evt) => {
      const payload = evt && evt.payload ? evt.payload : {};
      if (payload.id && payload.id !== bubbleId) return;
      render(root, meta, text, payload);
    });
  }

  main();
})();
