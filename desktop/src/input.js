(() => {
  const tauri = (typeof window !== "undefined" && window.__TAURI__) || null;
  if (!tauri) {
    console.error("Tauri globals missing; this page must run inside the Tauri webview");
    return;
  }
  const { invoke } = tauri.core;

  function readBubbleId() {
    const raw = window.location.hash.replace(/^#/, "");
    try {
      return decodeURIComponent(raw);
    } catch {
      return raw;
    }
  }

  function resolveLineHeightPx(textEl, computed) {
    const parsed = parseFloat(computed.lineHeight);
    if (Number.isFinite(parsed) && parsed > 0) {
      return parsed;
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

  async function applyDynamicHeight(root, text, actions) {
    const computed = window.getComputedStyle(text);
    const lineHeight = resolveLineHeightPx(text, computed);
    const maxTextHeight = lineHeight * 3;

    text.style.height = "auto";
    text.style.overflowY = "hidden";
    const naturalHeight = text.scrollHeight;
    const visibleTextHeight = Math.min(Math.max(lineHeight, naturalHeight), maxTextHeight);
    text.style.height = `${visibleTextHeight}px`;
    const hasScroll = naturalHeight > maxTextHeight;
    text.style.overflowY = hasScroll ? "auto" : "hidden";

    const rootStyle = window.getComputedStyle(root);
    const paddingTop = parseFloat(rootStyle.paddingTop) || 0;
    const paddingBottom = parseFloat(rootStyle.paddingBottom) || 0;
    const borderTop = parseFloat(rootStyle.borderTopWidth) || 0;
    const borderBottom = parseFloat(rootStyle.borderBottomWidth) || 0;
    const gap = parseFloat(rootStyle.gap) || 0;
    const actionsHeight = actions.getBoundingClientRect().height;
    const chrome = paddingTop + paddingBottom + borderTop + borderBottom + gap + actionsHeight;

    const desired = Math.ceil(chrome + visibleTextHeight);
    document.documentElement.style.height = `${desired}px`;
    document.body.style.height = `${desired}px`;
    const bubbleId = readBubbleId();
    await invoke("bubble_resize", { bubbleId, height: desired });
  }

  async function main() {
    const root = document.getElementById("bubble");
    const text = document.getElementById("text");
    const sendBtn = document.getElementById("send");
    const cancelBtn = document.getElementById("cancel");
    const actions = document.getElementById("actions");

    sendBtn.disabled = true;

    const refresh = () => {
      sendBtn.disabled = text.value.trim().length === 0;
      applyDynamicHeight(root, text, actions).catch(() => {});
    };

    text.addEventListener("input", refresh);

    const submit = async () => {
      const value = text.value.trim();
      if (!value) return;
      sendBtn.disabled = true;
      try {
        await invoke("send_local_input", { text: value });
      } catch (e) {
        console.error("send_local_input failed", e);
        sendBtn.disabled = false;
      }
    };
    const cancel = async () => {
      try {
        await invoke("cancel_local_input");
      } catch (e) {
        console.error("cancel_local_input failed", e);
      }
    };

    sendBtn.addEventListener("click", submit);
    cancelBtn.addEventListener("click", cancel);

    text.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        submit();
      } else if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
    });

    await applyDynamicHeight(root, text, actions);
    text.focus();
  }

  main();
})();
