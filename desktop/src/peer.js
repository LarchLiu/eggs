// Eggs peer overlay — frontend for one remote peer.
//
// One transparent always-on-top window per remote peer; this script runs in
// each. It identifies itself by the URL fragment (#<peer_id>) that the Rust
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

  // Identical to the local-pet contract; duplicated so peer.js stays
  // self-contained (no shared module loader).
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

  function readPeerId() {
    const raw = window.location.hash.replace(/^#/, "");
    try {
      return decodeURIComponent(raw);
    } catch {
      return raw;
    }
  }

  async function main() {
    const peerId = readPeerId();
    if (!peerId) {
      console.error("peer.html opened without a peer_id fragment");
      return;
    }

    const el = document.getElementById("pet");
    const pet = new PetView(el);

    let init;
    try {
      init = await invoke("get_peer_init", { peerId });
    } catch (e) {
      console.error(`get_peer_init(${peerId}) failed:`, e);
      return;
    }
    if (init.sprite_path_abs) {
      pet.setSheet(convertFileSrc(init.sprite_path_abs));
    }
    if (init.state) {
      pet.setState(init.state);
    }

    // The remote actor emits remote-peers globally on every change. Filter to
    // our own peer and update animation state.
    await listen("remote-peers", (evt) => {
      const list = evt.payload || [];
      const me = list.find((p) => p && p.peer_id === peerId);
      if (!me) return;
      if (me.state) pet.setState(me.state);
    });

    // Targeted state-only event from Rust (sent only to this window). Cheaper
    // than re-broadcasting the whole peer list when only a state changed.
    await listen("peer-state", (evt) => {
      const payload = evt.payload || {};
      if (payload.peer_id !== peerId) return;
      if (payload.state) pet.setState(payload.state);
    });
  }

  main();
})();
