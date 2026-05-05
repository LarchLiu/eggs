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

  // Hard-coded Codex pet contract (references/animation-rows.md in hatch-pet).
  // Frames count from 0; durations are in milliseconds, one per used column.
  const LAYOUT = [
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
  const BY_STATE = Object.fromEntries(LAYOUT.map((r) => [r.state, r]));

  class PetView {
    constructor(el) {
      this.el = el;
      this.row = LAYOUT[0];
      this.frame = 0;
      this.timer = null;
      el.style.width = `${CELL_W}px`;
      el.style.height = `${CELL_H}px`;
      this.tick();
    }

    setSheet(url) {
      this.el.style.backgroundImage = `url("${url}")`;
    }

    setState(name) {
      const next = BY_STATE[name];
      if (!next || next === this.row) return;
      this.row = next;
      this.frame = 0;
    }

    tick() {
      const x = -this.frame * CELL_W;
      const y = -this.row.row * CELL_H;
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

    let current = { pet: "", state: "idle" };

    async function loadPet(id) {
      if (!id) return;
      try {
        const manifest = await invoke("load_pet", { id });
        // `spritesheetAbs` is the absolute path Rust resolved; convertFileSrc
        // turns it into a webview-loadable asset:// URL.
        if (manifest && manifest.spritesheetAbs) {
          pet.setSheet(convertFileSrc(manifest.spritesheetAbs));
        }
      } catch (e) {
        console.error(`load_pet(${id}) failed:`, e);
      }
    }

    // Initial state.json read
    try {
      current = await invoke("read_state");
    } catch (e) {
      console.warn("read_state failed, using defaults", e);
    }
    await loadPet(current.pet);
    pet.setState(current.state);

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
      current = next;
    });

    // Drag-with-modifier: by default the window is click-through so the
    // desktop and apps below stay interactive. Holding Cmd (macOS) or Ctrl
    // (Win/Linux) toggles click-through off so the user can grab the pet.
    const isMac = navigator.platform.toLowerCase().includes("mac");
    const win = getCurrentWindow();
    let modifierActive = false;

    async function setClickThrough(on) {
      try {
        await invoke("set_click_through", { on });
        el.dataset.grab = on ? "false" : "true";
      } catch (e) {
        console.warn("set_click_through failed", e);
      }
    }

    function modifierFromEvent(e) {
      return isMac ? !!e.metaKey : !!e.ctrlKey;
    }

    window.addEventListener("keydown", async (e) => {
      const mod = modifierFromEvent(e);
      if (mod && !modifierActive) {
        modifierActive = true;
        await setClickThrough(false);
      }
    });
    window.addEventListener("keyup", async (e) => {
      const mod = modifierFromEvent(e);
      if (!mod && modifierActive) {
        modifierActive = false;
        await setClickThrough(true);
      }
    });

    window.addEventListener("mousedown", async (e) => {
      if (modifierFromEvent(e)) {
        try {
          await win.startDragging();
        } catch (err) {
          console.warn("startDragging failed", err);
        }
      }
    });

    // Right-click cycles through states for sanity testing during dev.
    window.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const idx = LAYOUT.indexOf(pet.row);
      const next = LAYOUT[(idx + 1) % LAYOUT.length];
      pet.setState(next.state);
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
