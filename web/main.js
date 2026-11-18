import init, { Emulator } from './pkg/nes_wasm.js';

const W = 256;
const H = 240;
const FRAME_MS = 1000 / 60.0988;

// Порядок = номер бита в контроллере NES.
const NAMES = ['A', 'B', 'Select', 'Start', 'Up', 'Down', 'Left', 'Right'];
const SHOWN = ['A', 'B', 'Select', 'Start', '↑', '↓', '←', '→'];

const DEFAULTS = [
  { A: 'KeyX', B: 'KeyZ', Select: 'ShiftRight', Start: 'Enter',
    Up: 'ArrowUp', Down: 'ArrowDown', Left: 'ArrowLeft', Right: 'ArrowRight' },
  { A: 'KeyG', B: 'KeyF', Select: 'KeyR', Start: 'KeyT',
    Up: 'KeyW', Down: 'KeyS', Left: 'KeyA', Right: 'KeyD' },
];

// ---------- привязки клавиш ----------

const STORE_KEY = 'nes-bindings-v1';
let bindings = loadBindings();
const keymap = new Map(); // code -> [игрок, бит]
rebuildKeymap();

function loadBindings() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE_KEY));
    if (Array.isArray(saved) && saved.length === 2) {
      // подмешиваем дефолты, чтобы не сломаться, если формат поменяется
      return saved.map((b, p) => ({ ...DEFAULTS[p], ...b }));
    }
  } catch { /* нет доступа к storage или битый JSON */ }
  return structuredClone(DEFAULTS);
}

function saveBindings() {
  try { localStorage.setItem(STORE_KEY, JSON.stringify(bindings)); } catch { /* ignore */ }
}

function rebuildKeymap() {
  keymap.clear();
  bindings.forEach((b, p) => {
    NAMES.forEach((name, bit) => {
      if (b[name]) keymap.set(b[name], [p, bit]);
    });
  });
}

function keyLabel(code) {
  if (!code) return '—';
  const special = {
    ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→',
    ShiftLeft: 'L Shift', ShiftRight: 'R Shift', ControlLeft: 'L Ctrl', ControlRight: 'R Ctrl',
    AltLeft: 'L Alt', AltRight: 'R Alt', Space: 'Space', Enter: 'Enter', Tab: 'Tab',
  };
  return special[code] ?? code.replace(/^Key|^Digit|^Numpad/, (m) => (m === 'Numpad' ? 'Num ' : ''));
}

// ---------- диалог «Управление» ----------

const dlg = document.getElementById('dlg');
const cols = document.getElementById('cols');
let listening = null; // { p, name }

function renderBinds() {
  cols.replaceChildren();
  bindings.forEach((b, p) => {
    const col = document.createElement('section');
    const title = document.createElement('h3');
    title.textContent = `Игрок ${p + 1}`;
    col.append(title);
    NAMES.forEach((name, i) => {
      const row = document.createElement('div');
      row.className = 'row';
      const label = document.createElement('span');
      label.textContent = SHOWN[i];
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'key';
      const active = listening && listening.p === p && listening.name === name;
      if (active) btn.classList.add('listening');
      btn.textContent = active ? 'Нажми клавишу…' : keyLabel(b[name]);
      btn.addEventListener('click', () => { listening = { p, name }; renderBinds(); });
      row.append(label, btn);
      col.append(row);
    });
    cols.append(col);
  });
}

document.getElementById('binds').addEventListener('click', () => {
  keys[0] = keys[1] = 0;
  listening = null;
  renderBinds();
  dlg.showModal();
});
document.getElementById('close').addEventListener('click', () => dlg.close());
document.getElementById('defaults').addEventListener('click', () => {
  bindings = structuredClone(DEFAULTS);
  listening = null;
  saveBindings();
  rebuildKeymap();
  renderBinds();
});
dlg.addEventListener('close', () => { listening = null; });

// ---------- ввод ----------

const keys = [0, 0]; // с клавиатуры
let pads = [0, 0];   // с геймпадов

window.addEventListener('keydown', (e) => {
  if (listening) {
    e.preventDefault();
    e.stopPropagation();
    if (e.code !== 'Escape') {
      const { p, name } = listening;
      if (e.code === 'Backspace') {
        bindings[p][name] = '';
      } else {
        // одна клавиша — одно действие: снимаем её с прежнего места
        for (const b of bindings) for (const n of NAMES) if (b[n] === e.code) b[n] = '';
        bindings[p][name] = e.code;
      }
      saveBindings();
      rebuildKeymap();
    }
    listening = null;
    renderBinds();
    return;
  }
  if (dlg.open) return;
  const hit = keymap.get(e.code);
  if (!hit) return;
  e.preventDefault();
  keys[hit[0]] |= 1 << hit[1];
});

window.addEventListener('keyup', (e) => {
  const hit = keymap.get(e.code);
  if (!hit) return;
  keys[hit[0]] &= ~(1 << hit[1]);
});

window.addEventListener('blur', () => { keys[0] = keys[1] = 0; });

function pollGamepads() {
  pads = [0, 0];
  if (!navigator.getGamepads) return;
  let p = 0;
  for (const g of navigator.getGamepads()) {
    if (!g || !g.connected) continue;
    if (p > 1) break;
    const b = (i) => !!(g.buttons[i] && g.buttons[i].pressed);
    const ax = g.axes[0] || 0;
    const ay = g.axes[1] || 0;
    let s = 0;
    if (b(1) || b(3)) s |= 1 << 0;           // A
    if (b(0) || b(2)) s |= 1 << 1;           // B
    if (b(8)) s |= 1 << 2;                   // Select
    if (b(9)) s |= 1 << 3;                   // Start
    if (b(12) || ay < -0.5) s |= 1 << 4;     // Up
    if (b(13) || ay > 0.5) s |= 1 << 5;      // Down
    if (b(14) || ax < -0.5) s |= 1 << 6;     // Left
    if (b(15) || ax > 0.5) s |= 1 << 7;      // Right
    pads[p++] = s;
  }
}

// ---------- эмулятор ----------

const canvas = document.getElementById('screen');
const ctx = canvas.getContext('2d');
const statusEl = document.getElementById('status');
const pauseBtn = document.getElementById('pause');
const setStatus = (text) => { statusEl.textContent = text; };

let wasmMemory = null;
let emu = null;
let paused = false;
let last = 0;
let acc = 0;

async function loadRom(file) {
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const next = new Emulator(bytes);
    if (emu) emu.free();
    emu = next;
    paused = false;
    pauseBtn.textContent = 'Пауза';
    setStatus(file.name);
  } catch (err) {
    setStatus(`Ошибка: ${err}`);
  }
}

document.getElementById('rom').addEventListener('change', (e) => {
  const file = e.target.files[0];
  if (file) loadRom(file);
  e.target.value = '';
});
window.addEventListener('dragover', (e) => e.preventDefault());
window.addEventListener('drop', (e) => {
  e.preventDefault();
  const file = e.dataTransfer.files[0];
  if (file) loadRom(file);
});
pauseBtn.addEventListener('click', () => {
  paused = !paused;
  pauseBtn.textContent = paused ? 'Продолжить' : 'Пауза';
});
document.getElementById('reset').addEventListener('click', () => { if (emu) emu.reset(); });

function loop(t) {
  requestAnimationFrame(loop);
  if (!emu || paused) { last = t; return; }
  acc += Math.min(t - last, 100);
  last = t;
  let ran = false;
  while (acc >= FRAME_MS) {
    pollGamepads();
    emu.set_pad(0, keys[0] | pads[0]);
    emu.set_pad(1, keys[1] | pads[1]);
    emu.frame();
    acc -= FRAME_MS;
    ran = true;
  }
  if (ran) {
    // память wasm может вырасти, поэтому view создаём заново каждый кадр
    const pixels = new Uint8ClampedArray(wasmMemory.buffer, emu.frame_ptr(), W * H * 4);
    ctx.putImageData(new ImageData(pixels, W, H), 0, 0);
  }
}

const wasm = await init();
wasmMemory = wasm.memory;
requestAnimationFrame(loop);
