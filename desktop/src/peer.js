// Eggs peer overlay — frontend for one remote peer.
//
// One transparent always-on-top window per remote peer; this script runs in
// each. It identifies itself by the URL fragment (#<device_id>) that the Rust
// peer manager assigns at window creation time, asks Rust for the cached
// sprite paths via the `get_peer_init` command, then animates the same Codex
// atlas LAYOUT as the local pet.

(() => {
  const tauri = (typeof window !== "undefined" && window.__TAURI__) || null;
  if (!tauri) {
    console.error("Tauri globals missing; this page must run inside the Tauri webview");
    return;
  }
  const { invoke, convertFileSrc } = tauri.core;
  const { listen } = tauri.event;

  const CELL_W = 192;
  const CELL_H = 208;

  const BUILTIN_LAYOUT = [
    { state: "idle",          row: 0, frames: 6, durations: [280, 110, 110, 140, 140, 320] },
    { state: "running-right", row: 1, frames: 8, durations: [120, 120, 120, 120, 120, 120, 120, 220] },
    { state: "running-left",  row: 2, frames: 8, durations: [120, 120, 120, 120, 120, 120, 120, 220] },
    { state: "waving",        row: 3, frames: 4, durations: [140, 140, 140, 280] },
    { state: "jumping",       row: 4, frames: 5, durations: [140, 140, 140, 140, 280] },
    { state: "failed",        row: 5, frames: 8, durations: [140, 140, 140, 140, 140, 140, 140, 240] },
    { state: "waiting",       row: 6, frames: 6, durations: [150, 150, 150, 150, 150, 260] },
    { state: "running",       row: 7, frames: 6, durations: [120, 120, 120, 120, 120, 220] },
    { state: "review",        row: 8, frames: 6, durations: [150, 150, 150, 150, 150, 280] },
  ];

  function normalizeAnimDef(raw) {
    if (!raw || typeof raw !== "object") return null;
    const state = typeof raw.state === "string" ? raw.state.trim() : "";
    const row = Number(raw.row);
    const frames = Number(raw.frames);
    const rawDurations = Array.isArray(raw.durations) ? raw.durations : [];
    if (!state || !Number.isInteger(row) || row < 0 || !Number.isInteger(frames) || frames <= 0) {
      return null;
    }
    const durations = [];
    for (let i = 0; i < frames; i += 1) {
      const n = Number(rawDurations[i]);
      durations.push(Number.isFinite(n) && n > 0 ? Math.round(n) : 150);
    }
    return { state, row, frames, durations };
  }

  function buildLayout(manifest) {
    const extra = [];
    const seen = new Set(BUILTIN_LAYOUT.map((entry) => entry.state));
    const append = (list) => {
      if (!Array.isArray(list)) return;
      for (const raw of list) {
        const def = normalizeAnimDef(raw);
        if (!def) continue;
        if (seen.has(def.state)) continue;
        seen.add(def.state);
        extra.push(def);
      }
    };
    append(manifest && manifest.custom);
    append(manifest && manifest.hatch);
    const layout = [...BUILTIN_LAYOUT, ...extra];
    const byState = Object.fromEntries(layout.map((entry) => [entry.state, entry]));
    const hatchOrder = [];
    const hatchSeen = new Set();
    if (manifest && Array.isArray(manifest.hatch)) {
      for (const raw of manifest.hatch) {
        const def = normalizeAnimDef(raw);
        if (!def) continue;
        if (hatchSeen.has(def.state)) continue;
        hatchSeen.add(def.state);
        hatchOrder.push(def.state);
      }
    }
    return { layout, byState, hatchOrder };
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function hatchTotalDuration(row) {
    if (!row) return 0;
    const count = Math.max(0, Number(row.frames) || 0);
    if (count <= 0) return 0;
    let total = 0;
    for (let i = 0; i < count; i += 1) {
      const n = Number(row.durations && row.durations[i]);
      total += Number.isFinite(n) && n > 0 ? n : 150;
    }
    return total;
  }

  class PetView {
    constructor(el, scale = 1.0) {
      this.el = el;
      this.layout = BUILTIN_LAYOUT;
      this.byState = Object.fromEntries(BUILTIN_LAYOUT.map((entry) => [entry.state, entry]));
      this.row = BUILTIN_LAYOUT[0];
      this.frame = 0;
      this.timer = null;
      this.scale = scale;
      this.atlasWidth = null;
      this.atlasHeight = null;
      this.sheetLoadToken = 0;
      // Peers render facing the local pet: they live to the right of the
      // local sprite (see peers::position_for_peer), so flipping horizontally
      // makes both pets look at each other and turns "running-right" into
      // "running toward the user", which reads more naturally for face-to-
      // face presence.
      this.el.style.transform = "scaleX(-1)";
      this.applyScale();
      this.tick();
    }
    applyScale() {
      const w = Math.round(CELL_W * this.scale);
      const h = Math.round(CELL_H * this.scale);
      this.el.style.width = `${w}px`;
      this.el.style.height = `${h}px`;
      if (this.atlasWidth && this.atlasHeight) {
        this.el.style.backgroundSize = `${Math.round(this.atlasWidth * this.scale)}px ${Math.round(this.atlasHeight * this.scale)}px`;
      } else {
        this.el.style.backgroundSize = "";
      }
    }
    setScale(scale) {
      this.scale = scale;
      this.applyScale();
    }
    async setSheet(url) {
      const token = ++this.sheetLoadToken;
      try {
        const { width, height } = await loadAtlasSize(url);
        if (token !== this.sheetLoadToken) return;
        if (width >= CELL_W && height >= CELL_H) {
          this.atlasWidth = width;
          this.atlasHeight = height;
        }
        this.applyScale();
        this.el.style.backgroundImage = `url("${url}")`;
      } catch (e) {
        if (token !== this.sheetLoadToken) return;
        console.warn(`atlas size probe failed for ${url}`, e);
        this.atlasWidth = null;
        this.atlasHeight = null;
        this.applyScale();
        this.el.style.backgroundImage = `url("${url}")`;
      }
    }
    setState(name) {
      const next = this.byState[name];
      if (!next || next === this.row) return;
      this.row = next;
      this.frame = 0;
    }
    setLayout(layout, byState) {
      this.layout = Array.isArray(layout) && layout.length > 0 ? layout : BUILTIN_LAYOUT;
      this.byState = byState && typeof byState === "object"
        ? byState
        : Object.fromEntries(this.layout.map((entry) => [entry.state, entry]));
      const next = this.byState[this.row.state] || this.layout[0] || BUILTIN_LAYOUT[0];
      if (next !== this.row) {
        this.row = next;
        this.frame = 0;
      }
    }
    tick() {
      const w = Math.round(CELL_W * this.scale);
      const h = Math.round(CELL_H * this.scale);
      const x = -this.frame * w;
      const y = -this.row.row * h;
      this.el.style.backgroundPosition = `${x}px ${y}px`;
      const dur = this.row.durations[this.frame] ?? 150;
      this.timer = setTimeout(() => {
        this.frame = (this.frame + 1) % this.row.frames;
        this.tick();
      }, dur);
    }
  }

  function loadAtlasSize(url) {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => {
        const width = Number(img.naturalWidth) || 0;
        const height = Number(img.naturalHeight) || 0;
        if (width <= 0 || height <= 0) {
          reject(new Error("atlas image reported empty size"));
          return;
        }
        resolve({ width, height });
      };
      img.onerror = () => reject(new Error("atlas image failed to load"));
      img.src = url;
    });
  }

  function readDeviceId() {
    const raw = window.location.hash.replace(/^#/, "");
    try {
      return decodeURIComponent(raw);
    } catch {
      return raw;
    }
  }

  async function main() {
    const deviceId = readDeviceId();
    if (!deviceId) {
      console.error("peer.html opened without a device_id fragment");
      return;
    }

    const el = document.getElementById("pet");
    const pet = new PetView(el, 1.0);
    let currentHatchOrder = [];
    let hatchRunToken = 0;

    let init;
    try {
      init = await invoke("get_peer_init", { deviceId });
    } catch (e) {
      console.error(`get_peer_init(${deviceId}) failed:`, e);
      return;
    }
    if (typeof init.scale_millis === "number") {
      pet.setScale(init.scale_millis / 1000);
    }
    if (init.sprite_path_abs) {
      await pet.setSheet(convertFileSrc(init.sprite_path_abs));
    }
    if (init.json_path_abs) {
      try {
        const manifestUrl = convertFileSrc(init.json_path_abs);
        const resp = await fetch(manifestUrl);
        if (resp.ok) {
          const manifest = await resp.json();
          const { layout, byState, hatchOrder } = buildLayout(manifest);
          pet.setLayout(layout, byState);
          currentHatchOrder = hatchOrder;
        }
      } catch (e) {
        console.warn("peer manifest parse failed; using builtin layout", e);
      }
    }
    if (init.state) {
      pet.setState(init.state);
    }

    // Local pet's scale changes (via context menu → state.json) are pushed
    // here by the main-window state poller; keep peers visually consistent.
    await listen("peer-scale", (evt) => {
      const ms = typeof evt.payload === "number"
        ? evt.payload
        : (evt.payload && evt.payload.scale_millis) || 1000;
      pet.setScale(ms / 1000);
    });

    // The remote actor emits remote-peers globally on every change. Filter to
    // our own peer and update animation state.
    await listen("remote-peers", (evt) => {
      const list = evt.payload || [];
      const me = list.find((p) => p && p.device_id === deviceId);
      if (!me) return;
      if (me.state) pet.setState(me.state);
    });

    // Targeted state-only event from Rust (sent only to this window). Cheaper
    // than re-broadcasting the whole peer list when only a state changed.
    await listen("peer-state", (evt) => {
      const payload = evt.payload || {};
      if (payload.device_id !== deviceId) return;
      if (payload.state) pet.setState(payload.state);
    });

    await listen("remote-peer-hatch", async (evt) => {
      const payload = evt && evt.payload ? evt.payload : {};
      if (payload.device_id !== deviceId) return;
      if (!Array.isArray(currentHatchOrder) || currentHatchOrder.length === 0) return;
      hatchRunToken += 1;
      const token = hatchRunToken;
      for (const stateName of currentHatchOrder) {
        if (token !== hatchRunToken) return;
        const row = pet.byState[stateName];
        if (!row) continue;
        pet.setState(stateName);
        const total = hatchTotalDuration(row);
        if (total > 0) {
          await sleep(total);
        } else {
          await sleep(120);
        }
      }
      if (token !== hatchRunToken) return;
      const fallbackState = typeof payload.fallback_state === "string" && payload.fallback_state.trim()
        ? payload.fallback_state.trim()
        : "idle";
      if (pet.byState[fallbackState]) {
        pet.setState(fallbackState);
      } else if (pet.byState.idle) {
        pet.setState("idle");
      }
    });
  }

  main();
})();
