// Eggs desktop pet — frontend.
//
// Consumes the Codex pet contract directly:
//   * 8 columns x 9 rows atlas at 192x208 per cell.
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
  const HATCH_FIXED_STATES = ["unborn", "ready", "hatching", "hatched", "walk"];

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
    const hatch = manifest && Array.isArray(manifest.hatch) ? manifest.hatch : [];
    const hasHatch = hatch.length > 0;
    const hatchIsComplete = hasHatch
      && hatch.length === HATCH_FIXED_STATES.length
      && hatch.every((raw, idx) => {
        const state = raw && typeof raw.state === "string" ? raw.state.trim() : "";
        return state === HATCH_FIXED_STATES[idx];
      });
    if (hatchIsComplete) {
      append(hatch);
    } else if (hasHatch) {
      console.warn("Ignoring invalid hatch states: hatch must define exactly unborn,ready,hatching,hatched,walk");
    }
    const layout = [...BUILTIN_LAYOUT, ...extra];
    const byState = Object.fromEntries(layout.map((entry) => [entry.state, entry]));
    return { layout, byState };
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
      this.el.style.backgroundSize = `${CELL_W * 8 * this.scale}px ${CELL_H * 9 * this.scale}px`;
    }

    setScale(scale) {
      this.scale = scale;
      this.applyScale();
    }

    setSheet(url) {
      this.el.style.backgroundImage = `url("${url}")`;
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

  async function main() {
    const el = document.getElementById("pet");
    const pet = new PetView(el);
    el.dataset.grab = "false";

    let current = { pet: "", state: "idle", scale_millis: 1000 };

    function scaleFromMillis(scaleMillis) {
      return scaleMillis / 1000;
    }

    async function loadPet(id) {
      if (!id) return;
      try {
        const manifest = await invoke("load_pet", { id });
        // `spritesheetAbs` is the absolute path Rust resolved; convertFileSrc
        // turns it into a webview-loadable asset:// URL.
        if (manifest && manifest.spritesheetAbs) {
          pet.setSheet(convertFileSrc(manifest.spritesheetAbs));
        }
        const { layout, byState } = buildLayout(manifest);
        pet.setLayout(layout, byState);
      } catch (e) {
        console.error(`load_pet(${id}) failed:`, e);
        pet.setLayout(BUILTIN_LAYOUT, Object.fromEntries(BUILTIN_LAYOUT.map((entry) => [entry.state, entry])));
      }
    }

    // Initial state.json read
    try {
      current = await invoke("read_state");
    } catch (e) {
      console.warn("read_state failed, using defaults", e);
    }
    pet.setScale(scaleFromMillis(current.scale_millis ?? 1000));
    await loadPet(current.pet);
    pet.setState(current.state);

    // list_states() is menu-oriented and does not include hatch states.

    // React to state.json changes (CLI subcommands or external editors).
    await listen("state-changed", async (evt) => {
      const next = evt.payload;
      if (!next) return;
      if (next.pet !== current.pet) {
        await loadPet(next.pet);
      }
      if (next.state !== current.state) {
        pet.setState(next.state);
      }
      if (next.scale_millis !== current.scale_millis) {
        pet.setScale(scaleFromMillis(next.scale_millis));
      }
      current = next;
    });

    const win = getCurrentWindow();

    window.addEventListener("mousedown", async (e) => {
      if (e.button === 0) {
        el.dataset.grab = "true";
        try {
          await win.startDragging();
        } catch (err) {
          console.warn("startDragging failed", err);
          el.dataset.grab = "false";
        }
      }
    });

    window.addEventListener("mouseup", () => {
      el.dataset.grab = "false";
    });
    window.addEventListener("mouseleave", () => {
      el.dataset.grab = "false";
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
  }

  main();
})();
