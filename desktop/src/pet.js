// Eggs desktop pet — frontend.
//
// Consumes the Codex pet contract directly:
//   * Each cell is 192x208.
//   * Atlas pixel dimensions come from the loaded image itself, so pets can
//     ship extra animation rows without being vertically squashed.
//   * Each row is one animation state with a fixed frame count and per-frame
//     duration (LAYOUT below). Unused cells are transparent.
//
// Tauri 2 globals (enabled via `withGlobalTauri: true` in tauri.conf.json):
//   window.__TAURI__.core.invoke / convertFileSrc
//   window.__TAURI__.event.listen
//   window.__TAURI__.window.getCurrentWindow

(() => {
  const tauri = (typeof window !== "undefined" && window.__TAURI__) || null;
  if (!tauri) {
    console.error("Tauri globals missing; this page must run inside the Tauri webview");
    return;
  }
  const { invoke, convertFileSrc } = tauri.core;
  const { listen } = tauri.event;
  const { getCurrentWindow } = tauri.window;

  const CELL_W = 192;
  const CELL_H = 208;
  // Hold-to-jump should feel stable: require a clear move before switching
  // from jumping to running/dragging.
  const DRAG_THRESHOLD_PX = 16;
  const RUN_DIRECTION_SWITCH_PX = 14;

  // Built-in states (Codex pet baseline). A pet manifest can append more
  // states via { custom: [...], hatch: [...] }.
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
  const BUILTIN_STATE_SET = new Set(BUILTIN_LAYOUT.map((row) => row.state));
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
    constructor(el) {
      this.el = el;
      this.layout = BUILTIN_LAYOUT;
      this.byState = Object.fromEntries(BUILTIN_LAYOUT.map((entry) => [entry.state, entry]));
      this.row = BUILTIN_LAYOUT[0];
      this.frame = 0;
      this.timer = null;
      this.scale = 1.0;
      this.atlasWidth = null;
      this.atlasHeight = null;
      this.sheetLoadToken = 0;
      this.applyScale();
      this.tick();
    }

    applyScale() {
      const displayW = Math.round(CELL_W * this.scale);
      const displayH = Math.round(CELL_H * this.scale);
      document.documentElement.style.setProperty("--cell-w", `${displayW}px`);
      document.documentElement.style.setProperty("--cell-h", `${displayH}px`);
      this.el.style.width = `${displayW}px`;
      this.el.style.height = `${displayH}px`;
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
      if (this.timer) {
        clearTimeout(this.timer);
        this.timer = null;
      }
      this.tick();
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
      const displayW = Math.round(CELL_W * this.scale);
      const displayH = Math.round(CELL_H * this.scale);
      const x = -this.frame * displayW;
      const y = -this.row.row * displayH;
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

  async function main() {
    const el = document.getElementById("pet");
    const pet = new PetView(el);
    el.dataset.grab = "false";

    let current = { pet: "", state: "idle", scale_millis: 1000 };
    let hatchedPets = new Set();
    let hatchRunToken = 0;
    let currentHatchOrder = [];
    let currentPetSource = "local";

    function scaleFromMillis(scaleMillis) {
      return scaleMillis / 1000;
    }

    function hatchKeyFor(petId, sourceKind = "local") {
      if (!petId) return "";
      const source = typeof sourceKind === "string" && sourceKind.trim() ? sourceKind.trim() : "local";
      return `${source}:${petId}`;
    }

    async function loadPet(id) {
      if (!id) return;
      try {
        const manifest = await invoke("load_pet", { id });
        // `spritesheetAbs` is the absolute path Rust resolved; convertFileSrc
        // turns it into a webview-loadable asset:// URL.
        if (manifest && manifest.spritesheetAbs) {
          await pet.setSheet(convertFileSrc(manifest.spritesheetAbs));
        }
        const { layout, byState, hatchOrder } = buildLayout(manifest);
        pet.setLayout(layout, byState);
        currentHatchOrder = hatchOrder;
        currentPetSource = (manifest && manifest.sourceKind) || "local";
      } catch (e) {
        console.error(`load_pet(${id}) failed:`, e);
        pet.setLayout(BUILTIN_LAYOUT, Object.fromEntries(BUILTIN_LAYOUT.map((entry) => [entry.state, entry])));
        currentHatchOrder = [];
        currentPetSource = "local";
      }
    }

    async function runHatchSequence({ petId, hatchKey, fallbackState = "idle", markCompleted = false, syncRemoteFallback = false }) {
      if (!petId || !Array.isArray(currentHatchOrder) || currentHatchOrder.length === 0) return false;
      hatchRunToken += 1;
      const token = hatchRunToken;
      for (const stateName of currentHatchOrder) {
        if (token !== hatchRunToken) return false;
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
      if (token !== hatchRunToken) return false;
      if (markCompleted) {
        try {
          await invoke("mark_pet_hatched", { hatchKey });
          hatchedPets.add(hatchKey);
        } catch (e) {
          console.warn("mark_pet_hatched failed", e);
        }
      }
      if (fallbackState && pet.byState[fallbackState]) {
        pet.setState(fallbackState);
      } else if (pet.byState.idle) {
        pet.setState("idle");
      }
      if (syncRemoteFallback) {
        try {
          await invoke("queue_hatch_finish_state", { state: fallbackState || "idle" });
        } catch (e) {
          console.warn("queue_hatch_finish_state failed", e);
        }
      }
      return true;
    }

    async function playHatchIfNeeded(petId, sourceKind, fallbackState) {
      if (!petId || !Array.isArray(currentHatchOrder) || currentHatchOrder.length === 0) return;
      const hatchKey = hatchKeyFor(petId, sourceKind);
      if (hatchedPets.has(hatchKey)) return;
      await runHatchSequence({ petId, hatchKey, fallbackState, markCompleted: true });
    }

    // Initial state.json read
    try {
      current = await invoke("read_state");
    } catch (e) {
      console.warn("read_state failed, using defaults", e);
    }
    try {
      const completed = await invoke("read_hatched_pets");
      if (Array.isArray(completed)) {
        hatchedPets = new Set(completed.filter((id) => typeof id === "string" && id.trim()));
      }
    } catch (e) {
      console.warn("read_hatched_pets failed", e);
    }
    pet.setScale(scaleFromMillis(current.scale_millis ?? 1000));
    await loadPet(current.pet);
    pet.setState(current.state);
    await playHatchIfNeeded(current.pet, currentPetSource, current.state);

    // list_states() is menu-oriented and does not include hatch states.

    const win = getCurrentWindow();
    let pointerDown = false;
    let dragStarted = false;
    let downPoint = null;
    let dragLastWindowX = null;
    let dragDirection = null;
    let dragPendingDeltaX = 0;
    let transientState = null;
    let restoreState = null;

    function hasState(name) {
      return !!(name && pet.byState && pet.byState[name]);
    }

    function fallbackState() {
      if (hasState(current.state)) return current.state;
      if (hasState("idle")) return "idle";
      return (pet.layout && pet.layout[0] && pet.layout[0].state) || null;
    }

    function applyStableState() {
      const name = fallbackState();
      if (name) pet.setState(name);
    }

    function setTransientState(name, { captureRestore = false } = {}) {
      if (!hasState(name)) return false;
      if (captureRestore && !restoreState) {
        restoreState = fallbackState();
      }
      transientState = name;
      pet.setState(name);
      return true;
    }

    function clearTransientState() {
      transientState = null;
      const next = hasState(restoreState) ? restoreState : fallbackState();
      restoreState = null;
      if (next) {
        pet.setState(next);
      }
    }

    function releasePointer({ resetVisual = true } = {}) {
      const wasDragging = dragStarted;
      const startedWithPointer = pointerDown;
      pointerDown = false;
      dragStarted = false;
      downPoint = null;
      dragLastWindowX = null;
      dragDirection = null;
      dragPendingDeltaX = 0;
      el.dataset.grab = "false";
      if (resetVisual && transientState && (startedWithPointer || wasDragging)) {
        clearTransientState();
      }
    }

    // React to state.json changes (CLI subcommands or external editors).
    await listen("state-changed", async (evt) => {
      const next = evt.payload;
      if (!next) return;
      if (next.pet !== current.pet) {
        hatchRunToken += 1;
        await loadPet(next.pet);
      }
      if (next.state !== current.state && !transientState) {
        pet.setState(next.state);
      }
      if (next.scale_millis !== current.scale_millis) {
        pet.setScale(scaleFromMillis(next.scale_millis));
      }
      if (next.pet !== current.pet) {
        await playHatchIfNeeded(next.pet, currentPetSource, next.state);
      }
      current = next;
      if (!transientState) {
        applyStableState();
      } else if (!hasState(transientState)) {
        clearTransientState();
      }
    });

    window.addEventListener("mousedown", async (e) => {
      if (e.button === 0) {
        pointerDown = true;
        dragStarted = false;
        downPoint = { x: e.clientX, y: e.clientY };
        dragLastWindowX = null;
        dragDirection = null;
        dragPendingDeltaX = 0;
        el.dataset.grab = "true";
        restoreState = fallbackState();
        setTransientState("jumping", { captureRestore: true });
      }
    });

    window.addEventListener("mousemove", async (e) => {
      if (!pointerDown || dragStarted || !downPoint) return;
      const dx = e.clientX - downPoint.x;
      const dy = e.clientY - downPoint.y;
      if (Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      const runState = dx >= 0 ? "running-right" : "running-left";
      setTransientState(runState, { captureRestore: true });
      dragDirection = dx >= 0 ? "right" : "left";
      dragPendingDeltaX = 0;
      dragStarted = true;
      try {
        await win.startDragging();
      } catch (err) {
        console.warn("startDragging failed", err);
        releasePointer({ resetVisual: true });
      }
    });

    window.addEventListener("mouseup", () => {
      releasePointer({ resetVisual: true });
    });
    window.addEventListener("mouseleave", () => {
      releasePointer({ resetVisual: true });
    });
    window.addEventListener("blur", () => {
      releasePointer({ resetVisual: true });
    });

    await listen("pet-window-moved", (evt) => {
      if (!pointerDown || !dragStarted) return;
      const payload = evt && evt.payload;
      const x = Array.isArray(payload) ? Number(payload[0]) : Number(payload && payload.x);
      if (!Number.isFinite(x)) return;
      if (dragLastWindowX == null) {
        dragLastWindowX = x;
        return;
      }
      const dx = x - dragLastWindowX;
      dragLastWindowX = x;
      if (dx === 0) return;
      const nextDirection = dx > 0 ? "right" : "left";
      if (!dragDirection) {
        dragDirection = nextDirection;
        dragPendingDeltaX = 0;
        setTransientState(
          nextDirection === "right" ? "running-right" : "running-left",
          { captureRestore: false },
        );
        return;
      }
      if (nextDirection === dragDirection) {
        dragPendingDeltaX = 0;
        return;
      }
      dragPendingDeltaX += dx;
      if (dragPendingDeltaX >= RUN_DIRECTION_SWITCH_PX) {
        dragDirection = "right";
        dragPendingDeltaX = 0;
        setTransientState("running-right", { captureRestore: false });
      } else if (dragPendingDeltaX <= -RUN_DIRECTION_SWITCH_PX) {
        dragDirection = "left";
        dragPendingDeltaX = 0;
        setTransientState("running-left", { captureRestore: false });
      }
    });

    window.addEventListener("contextmenu", async (e) => {
      e.preventDefault();
      el.dataset.grab = "false";
      try {
        await invoke("show_context_menu");
      } catch (err) {
        console.warn("show_context_menu failed", err);
      }
    });

    window.addEventListener("dblclick", async (e) => {
      e.preventDefault();
      el.dataset.grab = "false";
      try {
        await invoke("open_local_input");
      } catch (err) {
        console.warn("open_local_input failed", err);
      }
    });

    // Remote multiplayer events. Visual rendering of peers is a follow-up;
    // for now we surface them in the devtools console so the protocol can
    // be verified end-to-end.
    await listen("remote-status", (evt) => {
      console.log("[remote-status]", evt.payload);
    });
    await listen("remote-peers", (evt) => {
      console.log("[remote-peers]", evt.payload);
    });

    await listen("play-hatch", async (evt) => {
      const payload = evt && evt.payload ? evt.payload : {};
      const fallbackState = typeof payload.fallback_state === "string" && payload.fallback_state.trim()
        ? payload.fallback_state.trim()
        : current.state;
      const syncRemote = !!payload.sync_remote;
      await runHatchSequence({
        petId: current.pet,
        fallbackState,
        markCompleted: false,
        syncRemoteFallback: syncRemote,
      });
    });
  }

  main();
})();
