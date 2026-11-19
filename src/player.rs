//! Высокоуровневый API поверх ядра эмулятора: окно игры (canvas), клавиатура и геймпад
//! с переназначением клавиш, звук и главный цикл. Всё, что раньше делал `main.js`.
//!
//! Браузерные API звука и геймпада вызываются через `Reflect`: так код не зависит от
//! набора web-sys-биндингов и работает в любом браузере, где эти API есть.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use js_sys::{
    Array, Float32Array, Function, Object, Promise, Reflect, Uint8Array, Uint8ClampedArray, JSON,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local, JsFuture};
use web_sys::{Element, Event, EventTarget, HtmlCanvasElement, KeyboardEvent, Performance, Response, Window};

use crate::state::{base64_decode, base64_encode};
use crate::Emulator;

const W: u32 = 256;
const H: u32 = 240;
const FRAME_MS: f64 = 1000.0 / 60.0988;
const RING: usize = 16384;

/// Порядок = номер бита в контроллере NES.
const BUTTONS: [&str; 8] = ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"];

const DEFAULTS: [[&str; 8]; 2] = [
    ["KeyX", "KeyZ", "ShiftRight", "Enter", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"],
    ["KeyG", "KeyF", "KeyR", "KeyT", "KeyW", "KeyS", "KeyA", "KeyD"],
];

type Bindings = [[String; 8]; 2];

fn default_bindings() -> Bindings {
    std::array::from_fn(|p| std::array::from_fn(|b| DEFAULTS[p][b].to_string()))
}

// ---------- мелкие помощники для работы с JS ----------

fn get(obj: &JsValue, key: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn call(obj: &JsValue, name: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
    let f: Function = Reflect::get(obj, &JsValue::from_str(name))?
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("{} не является функцией", name)))?;
    let list = Array::new();
    for a in args {
        list.push(a);
    }
    Reflect::apply(&f, obj, &list)
}

fn js_err(e: &JsValue) -> String {
    if let Some(s) = e.as_string() {
        return s;
    }
    let name = get(e, "name").as_string().unwrap_or_default();
    let msg = get(e, "message").as_string().unwrap_or_default();
    if name.is_empty() && msg.is_empty() {
        "неизвестная ошибка".to_string()
    } else {
        format!("{}: {}", name, msg)
    }
}

fn err(text: &str) -> JsValue {
    JsValue::from_str(text)
}

fn opt(o: &JsValue, key: &str) -> Option<JsValue> {
    if !o.is_object() {
        return None;
    }
    let v = get(o, key);
    if v.is_undefined() || v.is_null() {
        None
    } else {
        Some(v)
    }
}

fn opt_string(o: &JsValue, key: &str) -> Option<String> {
    opt(o, key).and_then(|v| v.as_string())
}

fn opt_bool(o: &JsValue, key: &str) -> Option<bool> {
    opt(o, key).and_then(|v| v.as_bool())
}

fn opt_f64(o: &JsValue, key: &str) -> Option<f64> {
    opt(o, key).and_then(|v| v.as_f64())
}

fn parse_target(player: u8, button: &str) -> Result<(usize, usize), JsValue> {
    if player > 1 {
        return Err(err("player должен быть 0 или 1"));
    }
    match BUTTONS.iter().position(|b| b.eq_ignore_ascii_case(button)) {
        Some(b) => Ok((player as usize, b)),
        None => Err(err("неизвестная кнопка: ожидается A, B, Select, Start, Up, Down, Left или Right")),
    }
}

// ---------- обработчики событий ----------

struct Listener {
    target: EventTarget,
    name: &'static str,
    cb: Closure<dyn FnMut(Event)>,
}

impl Listener {
    fn new(target: &EventTarget, name: &'static str, f: impl FnMut(Event) + 'static) -> Listener {
        let cb = Closure::wrap(Box::new(f) as Box<dyn FnMut(Event)>);
        let _ = target.add_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
        Listener { target: target.clone(), name, cb }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self
            .target
            .remove_event_listener_with_callback(self.name, self.cb.as_ref().unchecked_ref());
    }
}

// ---------- звук ----------

/// Данные, которые трогают колбэки звука. Лежат отдельно от остального состояния,
/// чтобы колбэки никогда не конфликтовали по заимствованию с главным циклом.
struct AudioShared {
    level: f64,    // сколько сэмплов сейчас в буфере вывода
    last_msg: f64, // когда вывод в последний раз о себе сообщил
    ring: Vec<f32>,
    r: usize,
    w: usize,
    fill: usize,
}

impl AudioShared {
    fn new() -> AudioShared {
        AudioShared { level: 0.0, last_msg: 0.0, ring: vec![0.0; RING], r: 0, w: 0, fill: 0 }
    }

    fn clear(&mut self) {
        self.r = 0;
        self.w = 0;
        self.fill = 0;
        self.level = 0.0;
    }

    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            if self.fill >= RING {
                break;
            }
            self.ring[self.w] = s;
            self.w = (self.w + 1) % RING;
            self.fill += 1;
        }
        self.level = self.fill as f64;
    }

    fn pull(&mut self, out: &mut [f32]) {
        for o in out.iter_mut() {
            if self.fill > 0 {
                *o = self.ring[self.r];
                self.r = (self.r + 1) % RING;
                self.fill -= 1;
            } else {
                *o = 0.0;
            }
        }
        self.level = self.fill as f64;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    None,
    Worklet,
    Compat,
}

struct Audio {
    ctx: Option<JsValue>,
    gain: Option<JsValue>,
    node: Option<JsValue>,
    port: Option<JsValue>,
    post: Option<Function>,
    mode: Mode,
    error: String,
    shared: Rc<RefCell<AudioShared>>,
    muted: bool,
    volume: f32,
    buffer_seconds: f64,
    cb: Option<Closure<dyn FnMut(JsValue)>>,
}

fn ctx_state(ctx: &JsValue) -> String {
    get(ctx, "state").as_string().unwrap_or_default()
}

fn ctx_rate(ctx: &JsValue) -> f64 {
    get(ctx, "sampleRate").as_f64().unwrap_or(48000.0)
}

fn new_audio_context() -> Result<JsValue, JsValue> {
    let global: JsValue = js_sys::global().into();
    let mut ctor = get(&global, "AudioContext");
    if !ctor.is_function() {
        ctor = get(&global, "webkitAudioContext");
    }
    let ctor: Function = ctor.dyn_into().map_err(|_| err("Web Audio не поддерживается этим браузером"))?;
    Reflect::construct(&ctor, &Array::new())
}

/// Основной вариант: AudioWorklet (нужен HTTPS или localhost).
async fn make_worklet(
    ctx: &JsValue,
    url: &str,
    shared: &Rc<RefCell<AudioShared>>,
    perf: &Performance,
) -> Result<(JsValue, JsValue, Function, Closure<dyn FnMut(JsValue)>), JsValue> {
    let worklet = get(ctx, "audioWorklet");
    if worklet.is_undefined() || worklet.is_null() {
        return Err(err("AudioWorklet недоступен (нужен HTTPS или localhost)"));
    }
    let promise = call(&worklet, "addModule", &[&JsValue::from_str(url)])?;
    let promise: Promise = promise.dyn_into().map_err(|_| err("addModule не вернул Promise"))?;
    JsFuture::from(promise).await?;

    let opts = Object::new();
    Reflect::set(&opts, &JsValue::from_str("numberOfInputs"), &JsValue::from_f64(0.0))?;
    Reflect::set(&opts, &JsValue::from_str("numberOfOutputs"), &JsValue::from_f64(1.0))?;
    let global: JsValue = js_sys::global().into();
    let ctor: Function = get(&global, "AudioWorkletNode")
        .dyn_into()
        .map_err(|_| err("AudioWorkletNode недоступен"))?;
    let args = Array::of3(ctx, &JsValue::from_str("nes-audio"), &opts);
    let node = Reflect::construct(&ctor, &args)?;

    let port = get(&node, "port");
    let post: Function = get(&port, "postMessage")
        .dyn_into()
        .map_err(|_| err("port.postMessage недоступен"))?;

    let sh = shared.clone();
    let perf = perf.clone();
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        if let Some(level) = get(&e, "data").as_f64() {
            if let Ok(mut s) = sh.try_borrow_mut() {
                s.level = level;
                s.last_msg = perf.now();
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    Reflect::set(&port, &JsValue::from_str("onmessage"), cb.as_ref())?;
    Ok((node, port, post, cb))
}

/// Запасной вариант для обычного HTTP: ScriptProcessorNode (устаревший, но работает везде).
fn make_compat(
    ctx: &JsValue,
    shared: &Rc<RefCell<AudioShared>>,
    perf: &Performance,
) -> Result<(JsValue, Closure<dyn FnMut(JsValue)>), JsValue> {
    let node = call(
        ctx,
        "createScriptProcessor",
        &[&JsValue::from_f64(1024.0), &JsValue::from_f64(1.0), &JsValue::from_f64(1.0)],
    )?;
    let sh = shared.clone();
    let perf = perf.clone();
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        let out_buf = get(&e, "outputBuffer");
        let ch = match call(&out_buf, "getChannelData", &[&JsValue::from_f64(0.0)]) {
            Ok(c) => c,
            Err(_) => return,
        };
        let ch: Float32Array = match ch.dyn_into() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut tmp = vec![0f32; ch.length() as usize];
        if let Ok(mut s) = sh.try_borrow_mut() {
            s.pull(&mut tmp);
            s.last_msg = perf.now();
        }
        ch.copy_from(&tmp);
    }) as Box<dyn FnMut(JsValue)>);
    Reflect::set(&node, &JsValue::from_str("onaudioprocess"), cb.as_ref())?;
    Ok((node, cb))
}

async fn setup_audio(rc: Rc<RefCell<Inner>>) {
    let (ctx, url, shared, perf) = {
        let i = rc.borrow();
        match &i.audio.ctx {
            Some(c) => (c.clone(), i.worklet_url.clone(), i.audio.shared.clone(), i.perf.clone()),
            None => return,
        }
    };

    let mut mode = Mode::None;
    let mut node: Option<JsValue> = None;
    let mut port: Option<JsValue> = None;
    let mut post: Option<Function> = None;
    let mut cb: Option<Closure<dyn FnMut(JsValue)>> = None;
    let mut error = String::new();

    match make_worklet(&ctx, &url, &shared, &perf).await {
        Ok((n, p, f, c)) => {
            node = Some(n);
            port = Some(p);
            post = Some(f);
            cb = Some(c);
            mode = Mode::Worklet;
        }
        Err(e) => {
            web_sys::console::warn_2(&JsValue::from_str("AudioWorklet не заработал, режим совместимости:"), &e);
            error = js_err(&e);
        }
    }
    if mode == Mode::None {
        match make_compat(&ctx, &shared, &perf) {
            Ok((n, c)) => {
                node = Some(n);
                cb = Some(c);
                mode = Mode::Compat;
            }
            Err(e) => error = js_err(&e),
        }
    }

    let mut i = rc.borrow_mut();
    if i.destroyed {
        return;
    }
    let node = match node {
        Some(n) => n,
        None => {
            i.audio.error = error;
            return;
        }
    };

    let gain = match call(&ctx, "createGain", &[]) {
        Ok(g) => g,
        Err(e) => {
            i.audio.error = js_err(&e);
            return;
        }
    };
    let dest = get(&ctx, "destination");
    if let Err(e) = call(&node, "connect", &[&gain]).and_then(|_| call(&gain, "connect", &[&dest])) {
        i.audio.error = js_err(&e);
        return;
    }

    shared.borrow_mut().last_msg = perf.now();
    i.audio.buffer_seconds = if mode == Mode::Compat { 0.08 } else { 0.05 };
    i.audio.gain = Some(gain);
    i.audio.node = Some(node);
    i.audio.port = port;
    i.audio.post = post;
    i.audio.cb = cb;
    i.audio.error = String::new();
    i.audio.mode = mode;
    i.apply_gain();
    let rate = ctx_rate(&ctx) as f32;
    if let Some(emu) = i.emu.as_mut() {
        emu.set_sample_rate(rate);
    }
}

/// Браузер разрешает звук только после жеста пользователя. Контекст создаём на первом жесте,
/// а resume() вызываем на каждом следующем, пока он не заработает.
fn start_audio(rc: &Rc<RefCell<Inner>>) {
    let need_setup;
    {
        let mut i = match rc.try_borrow_mut() {
            Ok(i) => i,
            Err(_) => return,
        };
        if i.destroyed {
            return;
        }
        if i.audio.ctx.is_none() {
            // Safari: играть звук даже при включённом беззвучном режиме iPhone
            let nav: JsValue = i.window.navigator().into();
            let session = get(&nav, "audioSession");
            if session.is_object() {
                let _ = Reflect::set(&session, &JsValue::from_str("type"), &JsValue::from_str("playback"));
            }
            match new_audio_context() {
                Ok(c) => {
                    i.audio.ctx = Some(c);
                    need_setup = true;
                }
                Err(e) => {
                    i.audio.error = js_err(&e);
                    return;
                }
            }
        } else {
            need_setup = false;
        }
        if let Some(ctx) = &i.audio.ctx {
            if ctx_state(ctx) != "running" {
                let _ = call(ctx, "resume", &[]);
            }
        }
    }
    if need_setup {
        spawn_local(setup_audio(rc.clone()));
    }
}

// ---------- состояние ----------

struct Capture {
    player: usize,
    button: usize,
    resolve: Function,
}

struct Inner {
    window: Window,
    perf: Performance,
    canvas: HtmlCanvasElement,
    owns_canvas: bool,
    ctx2d: JsValue,
    image_data: Function,
    emu: Option<Emulator>,
    paused: bool,
    destroyed: bool,
    keyboard: bool,
    gamepad: bool,
    storage_key: String,
    worklet_url: String,
    bindings: Bindings,
    keys: [u8; 2],
    pads: [u8; 2],
    capture: Option<Capture>,
    last: f64,
    acc: f64,
    audio: Audio,
    listeners: Vec<Listener>,
    raf_closure: Option<Closure<dyn FnMut(f64)>>,
    raf_fn: Option<Function>,
}

impl Inner {
    // ----- привязки клавиш -----

    fn lookup(&self, code: &str) -> Option<(usize, usize)> {
        if code.is_empty() {
            return None;
        }
        for p in 0..2 {
            for b in 0..8 {
                if self.bindings[p][b] == code {
                    return Some((p, b));
                }
            }
        }
        None
    }

    fn assign(&mut self, player: usize, button: usize, code: String) {
        if !code.is_empty() {
            // одна клавиша — одно действие: снимаем её с прежнего места
            for p in 0..2 {
                for b in 0..8 {
                    if self.bindings[p][b] == code {
                        self.bindings[p][b].clear();
                    }
                }
            }
        }
        self.bindings[player][button] = code;
        self.keys = [0, 0];
        self.save_bindings();
    }

    fn bindings_js(&self) -> Array {
        let all = Array::new();
        for p in 0..2 {
            let o = Object::new();
            for (b, name) in BUTTONS.iter().enumerate() {
                let _ = Reflect::set(&o, &JsValue::from_str(name), &JsValue::from_str(&self.bindings[p][b]));
            }
            all.push(&o);
        }
        all
    }

    fn save_bindings(&self) {
        let json = match JSON::stringify(&self.bindings_js()) {
            Ok(s) => String::from(s),
            Err(_) => return,
        };
        if let Ok(Some(storage)) = self.window.local_storage() {
            let _ = storage.set_item(&self.storage_key, &json);
        }
    }

    // ----- кадры -----

    fn tick(&mut self, t: f64) {
        if self.emu.is_none() || self.paused {
            self.last = t;
            return;
        }
        let mut ran = 0;
        if self.audio_ready(t) {
            // темп задаёт звук: считаем кадры, пока в буфере меньше целевого запаса
            let target = self.audio_rate() * self.audio.buffer_seconds;
            loop {
                let level = self.audio.shared.borrow().level;
                if level >= target || ran >= 4 {
                    break;
                }
                self.run_frame(true);
                ran += 1;
            }
            self.acc = 0.0;
        } else {
            // звука нет (ещё не было жеста или он недоступен) — темп по таймеру
            self.acc += (t - self.last).min(100.0);
            while self.acc >= FRAME_MS {
                self.run_frame(false);
                self.acc -= FRAME_MS;
                ran += 1;
            }
        }
        self.last = t;
        if ran > 0 {
            self.draw();
        }
    }

    fn run_frame(&mut self, audio_on: bool) {
        self.poll_gamepads();
        let p0 = self.keys[0] | self.pads[0];
        let p1 = self.keys[1] | self.pads[1];
        let samples: Vec<f32> = match self.emu.as_mut() {
            Some(emu) => {
                emu.set_pad(0, p0);
                emu.set_pad(1, p1);
                emu.frame();
                if audio_on {
                    emu.audio_samples().to_vec()
                } else {
                    Vec::new()
                }
            }
            None => return,
        };
        if !samples.is_empty() {
            self.push_audio(&samples);
        }
    }

    fn push_audio(&mut self, samples: &[f32]) {
        match self.audio.mode {
            Mode::Worklet => {
                if let (Some(post), Some(port)) = (&self.audio.post, &self.audio.port) {
                    let arr = Float32Array::from(samples);
                    let transfer = Array::of1(&arr.buffer());
                    if post.call2(port, &arr, &transfer).is_ok() {
                        self.audio.shared.borrow_mut().level += samples.len() as f64;
                    }
                }
            }
            Mode::Compat => self.audio.shared.borrow_mut().push(samples),
            Mode::None => {}
        }
    }

    fn draw(&self) {
        let emu = match &self.emu {
            Some(e) => e,
            None => return,
        };
        let bytes = Uint8Array::from(emu.frame_buffer());
        let clamped = Uint8ClampedArray::new(&bytes.buffer());
        let args = Array::of3(&clamped, &JsValue::from_f64(W as f64), &JsValue::from_f64(H as f64));
        if let Ok(img) = Reflect::construct(&self.image_data, &args) {
            let _ = call(&self.ctx2d, "putImageData", &[&img, &JsValue::from_f64(0.0), &JsValue::from_f64(0.0)]);
        }
    }

    fn poll_gamepads(&mut self) {
        self.pads = [0, 0];
        if !self.gamepad {
            return;
        }
        let nav: JsValue = self.window.navigator().into();
        let list = match call(&nav, "getGamepads", &[]) {
            Ok(l) => l,
            Err(_) => return,
        };
        let list: Array = match list.dyn_into() {
            Ok(a) => a,
            Err(_) => return,
        };
        let mut slot = 0usize;
        for idx in 0..list.length() {
            if slot > 1 {
                break;
            }
            let gp = list.get(idx);
            if !gp.is_object() || !get(&gp, "connected").as_bool().unwrap_or(false) {
                continue;
            }
            let buttons = get(&gp, "buttons");
            let axes = get(&gp, "axes");
            let btn = |i: u32| -> bool {
                let b = Reflect::get_u32(&buttons, i).unwrap_or(JsValue::UNDEFINED);
                get(&b, "pressed").as_bool().unwrap_or(false)
            };
            let axis = |i: u32| -> f64 {
                Reflect::get_u32(&axes, i).ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
            };
            let (ax, ay) = (axis(0), axis(1));
            let mut s = 0u8;
            if btn(1) || btn(3) {
                s |= 1; // A
            }
            if btn(0) || btn(2) {
                s |= 2; // B
            }
            if btn(8) {
                s |= 4; // Select
            }
            if btn(9) {
                s |= 8; // Start
            }
            if btn(12) || ay < -0.5 {
                s |= 16; // Up
            }
            if btn(13) || ay > 0.5 {
                s |= 32; // Down
            }
            if btn(14) || ax < -0.5 {
                s |= 64; // Left
            }
            if btn(15) || ax > 0.5 {
                s |= 128; // Right
            }
            self.pads[slot] = s;
            slot += 1;
        }
    }

    fn load_rom(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        let mut emu = Emulator::new(bytes)?;
        if let Some(ctx) = &self.audio.ctx {
            emu.set_sample_rate(ctx_rate(ctx) as f32);
        }
        self.emu = Some(emu);
        self.paused = false;
        self.acc = 0.0;
        self.audio.shared.borrow_mut().clear();
        Ok(())
    }

    // ----- сохранения -----

    fn load_state(&mut self, data: &[u8]) -> Result<(), JsValue> {
        match self.emu.as_mut() {
            Some(emu) => emu.load_state(data)?,
            None => return Err(err("Сначала загрузи игру")),
        }
        self.audio.shared.borrow_mut().clear();
        self.acc = 0.0;
        self.draw(); // на паузе сразу показываем загруженный кадр
        Ok(())
    }

    fn storage(&self) -> Result<web_sys::Storage, JsValue> {
        self.window
            .local_storage()?
            .ok_or_else(|| err("localStorage недоступен"))
    }

    /// Слоты хранятся отдельно для каждой игры: ключ включает отпечаток ROM.
    fn slot_key(&self, slot: u8) -> Option<String> {
        let id = self.emu.as_ref()?.rom_id();
        Some(format!("{}:state:{:08x}:{}", self.storage_key, id, slot))
    }

    fn slot_raw(&self, slot: u8) -> Option<String> {
        let key = self.slot_key(slot)?;
        let storage = self.storage().ok()?;
        storage.get_item(&key).ok().flatten()
    }

    // ----- звук -----

    fn audio_rate(&self) -> f64 {
        match &self.audio.ctx {
            Some(ctx) => ctx_rate(ctx),
            None => 48000.0,
        }
    }

    /// Вывод молчит (контекст приостановлен, вкладка в фоне) — считаем, что звука нет,
    /// и держим темп по таймеру, чтобы игра не зависла.
    fn audio_ready(&self, now: f64) -> bool {
        if self.audio.mode == Mode::None {
            return false;
        }
        match &self.audio.ctx {
            Some(ctx) => ctx_state(ctx) == "running" && now - self.audio.shared.borrow().last_msg < 500.0,
            None => false,
        }
    }

    fn apply_gain(&self) {
        if let Some(gain) = &self.audio.gain {
            let v = if self.audio.muted { 0.0 } else { self.audio.volume as f64 };
            let _ = Reflect::set(&get(gain, "gain"), &JsValue::from_str("value"), &JsValue::from_f64(v));
        }
    }

    fn audio_status(&self) -> String {
        let a = &self.audio;
        if a.mode == Mode::None && !a.error.is_empty() {
            return format!("Звук: ошибка ({})", a.error);
        }
        match &a.ctx {
            None => "Звук: включится после первого нажатия".to_string(),
            Some(ctx) => {
                if a.mode == Mode::None {
                    return "Звук: загрузка…".to_string();
                }
                let state = ctx_state(ctx);
                if state != "running" {
                    return format!("Звук: {}, нажми любую кнопку", state);
                }
                let note = if a.mode == Mode::Compat {
                    " (режим совместимости: открой страницу по HTTPS или localhost)"
                } else {
                    ""
                };
                format!("Звук: работает, {} Гц{}", ctx_rate(ctx), note)
            }
        }
    }
}

// ---------- события клавиатуры ----------

fn is_typing_target(e: &KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<Element>().ok())
        .map(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"))
        .unwrap_or(false)
}

fn on_keydown(rc: &Rc<RefCell<Inner>>, e: KeyboardEvent) {
    let mut i = match rc.try_borrow_mut() {
        Ok(i) => i,
        Err(_) => return,
    };
    if i.destroyed {
        return;
    }
    let code = e.code();

    // режим «ждём клавишу для привязки»
    if let Some(cap) = i.capture.take() {
        e.prevent_default();
        e.stop_propagation();
        let value = if code == "Escape" {
            String::new()
        } else if code == "Backspace" {
            i.assign(cap.player, cap.button, String::new());
            String::new()
        } else {
            i.assign(cap.player, cap.button, code.clone());
            code
        };
        let _ = cap.resolve.call1(&JsValue::UNDEFINED, &JsValue::from_str(&value));
        return;
    }

    if !i.keyboard || is_typing_target(&e) {
        return;
    }
    if let Some((p, b)) = i.lookup(&code) {
        e.prevent_default();
        i.keys[p] |= 1u8 << b;
    }
}

fn on_keyup(rc: &Rc<RefCell<Inner>>, e: KeyboardEvent) {
    let mut i = match rc.try_borrow_mut() {
        Ok(i) => i,
        Err(_) => return,
    };
    let code = e.code();
    if let Some((p, b)) = i.lookup(&code) {
        i.keys[p] &= !(1u8 << b);
    }
}

// ---------- главный цикл ----------

fn start_loop(inner: &Rc<RefCell<Inner>>) {
    let weak: Weak<RefCell<Inner>> = Rc::downgrade(inner);
    let cb = Closure::wrap(Box::new(move |t: f64| frame(&weak, t)) as Box<dyn FnMut(f64)>);
    let func: Function = cb.as_ref().unchecked_ref::<Function>().clone();
    let window = {
        let mut i = inner.borrow_mut();
        i.raf_closure = Some(cb);
        i.raf_fn = Some(func.clone());
        i.window.clone()
    };
    let _ = window.request_animation_frame(&func);
}

fn frame(weak: &Weak<RefCell<Inner>>, t: f64) {
    let rc = match weak.upgrade() {
        Some(rc) => rc,
        None => return,
    };
    let mut i = rc.borrow_mut();
    if i.destroyed {
        return;
    }
    i.tick(t);
    if let Some(f) = &i.raf_fn {
        let _ = i.window.request_animation_frame(f);
    }
}

// ---------- публичный API ----------

/// Эмулятор NES с окном игры, вводом, звуком и главным циклом.
///
/// ```js
/// import init, { Nes } from './nes_wasm.js';
/// await init();
/// const nes = new Nes(document.getElementById('game'));
/// await nes.loadRomFromUrl('game.nes');
/// ```
#[wasm_bindgen]
pub struct Nes {
    inner: Rc<RefCell<Inner>>,
}

#[wasm_bindgen]
impl Nes {
    /// `target` — canvas (используется как есть) или любой элемент-контейнер (canvas создаётся
    /// внутри него); можно передать `null` и забрать canvas через `nes.canvas`.
    ///
    /// Опции (все необязательные): `scale` (размер canvas в целых пикселях ×256),
    /// `keyboard` (true), `gamepad` (true), `autoAudio` (true: звук включается на первом жесте),
    /// `muted` (false), `volume` (1.5), `storageKey` ("nes-bindings"),
    /// `workletUrl` ("audio-worklet.js").
    #[wasm_bindgen(constructor)]
    pub fn new(target: &JsValue, options: &JsValue) -> Result<Nes, JsValue> {
        console_error_panic_hook::set_once();
        let window = web_sys::window().ok_or_else(|| err("нет window"))?;
        let document = window.document().ok_or_else(|| err("нет document"))?;
        let perf = window.performance().ok_or_else(|| err("нет performance"))?;

        // окно игры
        let (canvas, owns_canvas) = match target.dyn_ref::<HtmlCanvasElement>() {
            Some(c) => (c.clone(), false),
            None => {
                let c: HtmlCanvasElement = document
                    .create_element("canvas")?
                    .dyn_into()
                    .map_err(|_| err("не удалось создать canvas"))?;
                if let Some(parent) = target.dyn_ref::<Element>() {
                    parent.append_child(&c)?;
                }
                (c, true)
            }
        };
        canvas.set_width(W);
        canvas.set_height(H);
        if owns_canvas {
            let style = canvas.style();
            style.set_property("image-rendering", "pixelated")?;
            style.set_property("background", "#000")?;
            style.set_property("display", "block")?;
            style.set_property("aspect-ratio", "256 / 240")?;
            match opt_f64(options, "scale") {
                Some(s) if s > 0.0 => style.set_property("width", &format!("{}px", W as f64 * s))?,
                _ => style.set_property("width", "100%")?,
            }
        }
        let ctx2d: JsValue = canvas
            .get_context("2d")?
            .ok_or_else(|| err("2d-контекст недоступен"))?
            .into();
        let window_js: JsValue = window.clone().into();
        let image_data: Function = get(&window_js, "ImageData")
            .dyn_into()
            .map_err(|_| err("ImageData недоступен"))?;

        let storage_key = opt_string(options, "storageKey").unwrap_or_else(|| "nes-bindings".to_string());
        let mut bindings = default_bindings();
        if let Ok(Some(storage)) = window.local_storage() {
            if let Ok(Some(saved)) = storage.get_item(&storage_key) {
                if let Ok(parsed) = JSON::parse(&saved) {
                    for p in 0..2 {
                        let player = Reflect::get_u32(&parsed, p as u32).unwrap_or(JsValue::UNDEFINED);
                        if !player.is_object() {
                            continue;
                        }
                        for (b, name) in BUTTONS.iter().enumerate() {
                            if let Some(code) = get(&player, name).as_string() {
                                bindings[p][b] = code;
                            }
                        }
                    }
                }
            }
        }

        let auto_audio = opt_bool(options, "autoAudio").unwrap_or(true);
        let inner = Rc::new(RefCell::new(Inner {
            window: window.clone(),
            perf,
            canvas,
            owns_canvas,
            ctx2d,
            image_data,
            emu: None,
            paused: false,
            destroyed: false,
            keyboard: opt_bool(options, "keyboard").unwrap_or(true),
            gamepad: opt_bool(options, "gamepad").unwrap_or(true),
            storage_key,
            worklet_url: opt_string(options, "workletUrl").unwrap_or_else(|| "audio-worklet.js".to_string()),
            bindings,
            keys: [0, 0],
            pads: [0, 0],
            capture: None,
            last: 0.0,
            acc: 0.0,
            audio: Audio {
                ctx: None,
                gain: None,
                node: None,
                port: None,
                post: None,
                mode: Mode::None,
                error: String::new(),
                shared: Rc::new(RefCell::new(AudioShared::new())),
                muted: opt_bool(options, "muted").unwrap_or(false),
                volume: opt_f64(options, "volume").unwrap_or(1.5) as f32,
                buffer_seconds: 0.05,
                cb: None,
            },
            listeners: Vec::new(),
            raf_closure: None,
            raf_fn: None,
        }));

        // обработчики на window; держат только Weak, поэтому не мешают освобождению
        let weak = Rc::downgrade(&inner);
        let win_target: EventTarget = window.unchecked_into();
        let mut listeners: Vec<Listener> = Vec::new();
        {
            let w = weak.clone();
            listeners.push(Listener::new(&win_target, "keydown", move |e| {
                if let Some(rc) = w.upgrade() {
                    on_keydown(&rc, e.unchecked_into());
                }
            }));
        }
        {
            let w = weak.clone();
            listeners.push(Listener::new(&win_target, "keyup", move |e| {
                if let Some(rc) = w.upgrade() {
                    on_keyup(&rc, e.unchecked_into());
                }
            }));
        }
        {
            let w = weak.clone();
            listeners.push(Listener::new(&win_target, "blur", move |_| {
                if let Some(rc) = w.upgrade() {
                    if let Ok(mut i) = rc.try_borrow_mut() {
                        i.keys = [0, 0];
                    }
                }
            }));
        }
        if auto_audio {
            // iOS/Safari считают жестом не все события, поэтому слушаем несколько
            for name in ["pointerup", "touchend", "click", "keydown"] {
                let w = weak.clone();
                listeners.push(Listener::new(&win_target, name, move |_| {
                    if let Some(rc) = w.upgrade() {
                        start_audio(&rc);
                    }
                }));
            }
        }
        inner.borrow_mut().listeners = listeners;

        start_loop(&inner);
        Ok(Nes { inner })
    }

    /// canvas, в котором рисуется игра.
    #[wasm_bindgen(getter)]
    pub fn canvas(&self) -> HtmlCanvasElement {
        self.inner.borrow().canvas.clone()
    }

    /// Загружает ROM (`Uint8Array` с содержимым .nes). Бросает строку с причиной, если не вышло.
    #[wasm_bindgen(js_name = loadRom)]
    pub fn load_rom(&self, rom: &[u8]) -> Result<(), JsValue> {
        self.inner.borrow_mut().load_rom(rom)
    }

    /// Скачивает ROM по ссылке и загружает его.
    #[wasm_bindgen(js_name = loadRomFromUrl)]
    pub fn load_rom_from_url(&self, url: String) -> Promise {
        let inner = self.inner.clone();
        future_to_promise(async move {
            let window = inner.borrow().window.clone();
            let resp = JsFuture::from(window.fetch_with_str(&url)).await?;
            let resp: Response = resp.dyn_into().map_err(|_| err("fetch вернул не Response"))?;
            if !resp.ok() {
                return Err(err(&format!("HTTP {} при загрузке {}", resp.status(), url)));
            }
            let buf = JsFuture::from(resp.array_buffer()?).await?;
            let bytes = Uint8Array::new(&buf).to_vec();
            inner.borrow_mut().load_rom(&bytes)?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Есть ли загруженная игра.
    #[wasm_bindgen(getter, js_name = hasRom)]
    pub fn has_rom(&self) -> bool {
        self.inner.borrow().emu.is_some()
    }

    pub fn pause(&self) {
        self.inner.borrow_mut().paused = true;
    }

    pub fn resume(&self) {
        self.inner.borrow_mut().paused = false;
    }

    #[wasm_bindgen(getter)]
    pub fn paused(&self) -> bool {
        self.inner.borrow().paused
    }

    /// Сброс консоли (кнопка Reset).
    pub fn reset(&self) {
        if let Some(emu) = self.inner.borrow_mut().emu.as_mut() {
            emu.reset();
        }
    }

    /// Во весь экран.
    pub fn fullscreen(&self) {
        let _ = call(&self.inner.borrow().canvas, "requestFullscreen", &[]);
    }

    // ----- сохранения -----

    /// Состояние консоли целиком в виде байт (`Uint8Array`): можно положить в файл, IndexedDB и т.п.
    #[wasm_bindgen(js_name = saveState)]
    pub fn save_state(&self) -> Result<Vec<u8>, JsValue> {
        match &self.inner.borrow().emu {
            Some(emu) => Ok(emu.save_state()),
            None => Err(err("Сначала загрузи игру")),
        }
    }

    /// Загружает байты из `saveState()`. Бросает строку, если файл повреждён или от другой игры.
    #[wasm_bindgen(js_name = loadState)]
    pub fn load_state(&self, data: &[u8]) -> Result<(), JsValue> {
        self.inner.borrow_mut().load_state(data)
    }

    /// Сохраняет состояние в слот (в localStorage; слоты у каждой игры свои). Номер слота: 0–255.
    #[wasm_bindgen(js_name = saveSlot)]
    pub fn save_slot(&self, slot: u8) -> Result<(), JsValue> {
        let i = self.inner.borrow();
        let key = i.slot_key(slot).ok_or_else(|| err("Сначала загрузи игру"))?;
        let bytes = match &i.emu {
            Some(emu) => emu.save_state(),
            None => return Err(err("Сначала загрузи игру")),
        };
        let value = format!("{}|{}", js_sys::Date::now() as u64, base64_encode(&bytes));
        i.storage()?
            .set_item(&key, &value)
            .map_err(|_| err("Не удалось сохранить: localStorage переполнен или запрещён"))
    }

    /// Загружает состояние из слота. Бросает строку, если слот пуст или повреждён.
    #[wasm_bindgen(js_name = loadSlot)]
    pub fn load_slot(&self, slot: u8) -> Result<(), JsValue> {
        let mut i = self.inner.borrow_mut();
        if i.emu.is_none() {
            return Err(err("Сначала загрузи игру"));
        }
        let raw = i.slot_raw(slot).ok_or_else(|| err("Слот пуст"))?;
        let payload = raw.splitn(2, '|').nth(1).unwrap_or("");
        let bytes = base64_decode(payload).map_err(|e| err(&e))?;
        i.load_state(&bytes)
    }

    #[wasm_bindgen(js_name = hasSlot)]
    pub fn has_slot(&self, slot: u8) -> bool {
        self.inner.borrow().slot_raw(slot).is_some()
    }

    /// Когда сделано сохранение в слоте (миллисекунды как у `Date.now()`) или `undefined`, если слот пуст.
    #[wasm_bindgen(js_name = slotTime)]
    pub fn slot_time(&self, slot: u8) -> Option<f64> {
        let raw = self.inner.borrow().slot_raw(slot)?;
        raw.split('|').next()?.parse::<f64>().ok()
    }

    #[wasm_bindgen(js_name = deleteSlot)]
    pub fn delete_slot(&self, slot: u8) {
        let i = self.inner.borrow();
        if let (Some(key), Ok(storage)) = (i.slot_key(slot), i.storage()) {
            let _ = storage.remove_item(&key);
        }
    }

    // ----- звук -----

    /// Включает звук. Вызывать из обработчика клика/нажатия, если `autoAudio: false`.
    #[wasm_bindgen(js_name = enableAudio)]
    pub fn enable_audio(&self) {
        start_audio(&self.inner);
    }

    #[wasm_bindgen(js_name = setMuted)]
    pub fn set_muted(&self, muted: bool) {
        let mut i = self.inner.borrow_mut();
        i.audio.muted = muted;
        i.apply_gain();
    }

    #[wasm_bindgen(getter)]
    pub fn muted(&self) -> bool {
        self.inner.borrow().audio.muted
    }

    /// Громкость: 0.0 — тишина, 1.0 — «как есть», по умолчанию 1.5.
    #[wasm_bindgen(js_name = setVolume)]
    pub fn set_volume(&self, volume: f32) {
        let mut i = self.inner.borrow_mut();
        i.audio.volume = volume.max(0.0);
        i.apply_gain();
    }

    #[wasm_bindgen(getter)]
    pub fn volume(&self) -> f32 {
        self.inner.borrow().audio.volume
    }

    /// Текст про состояние звука, например «Звук: работает, 48000 Гц».
    #[wasm_bindgen(js_name = audioStatus)]
    pub fn audio_status(&self) -> String {
        self.inner.borrow().audio_status()
    }

    // ----- управление -----

    /// Привязки клавиш: `[ {A:'KeyX', B:'KeyZ', ...}, {...} ]` (игрок 1, игрок 2).
    /// Значения — `KeyboardEvent.code`; пустая строка — кнопка не назначена.
    #[wasm_bindgen(js_name = getBindings)]
    pub fn get_bindings(&self) -> Array {
        self.inner.borrow().bindings_js()
    }

    /// Назначает клавишу (`KeyboardEvent.code`) на кнопку. `player`: 0 или 1;
    /// `button`: A, B, Select, Start, Up, Down, Left, Right. Сохраняется в localStorage.
    #[wasm_bindgen(js_name = setBinding)]
    pub fn set_binding(&self, player: u8, button: &str, code: &str) -> Result<(), JsValue> {
        let (p, b) = parse_target(player, button)?;
        self.inner.borrow_mut().assign(p, b, code.to_string());
        Ok(())
    }

    #[wasm_bindgen(js_name = resetBindings)]
    pub fn reset_bindings(&self) {
        let mut i = self.inner.borrow_mut();
        i.bindings = default_bindings();
        i.keys = [0, 0];
        i.save_bindings();
    }

    /// Ждёт следующее нажатие клавиши и назначает её на кнопку. Promise возвращает код клавиши;
    /// Esc отменяет (пустая строка), Backspace снимает привязку.
    #[wasm_bindgen(js_name = captureBinding)]
    pub fn capture_binding(&self, player: u8, button: &str) -> Promise {
        let target = parse_target(player, button);
        let inner = self.inner.clone();
        Promise::new(&mut move |resolve: Function, reject: Function| match &target {
            Ok((p, b)) => {
                let mut i = inner.borrow_mut();
                if let Some(old) = i.capture.take() {
                    let _ = old.resolve.call1(&JsValue::UNDEFINED, &JsValue::from_str(""));
                }
                i.capture = Some(Capture { player: *p, button: *b, resolve });
            }
            Err(e) => {
                let _ = reject.call1(&JsValue::UNDEFINED, e);
            }
        })
    }

    /// Отменяет ожидание клавиши из `captureBinding`.
    #[wasm_bindgen(js_name = cancelCapture)]
    pub fn cancel_capture(&self) {
        if let Some(c) = self.inner.borrow_mut().capture.take() {
            let _ = c.resolve.call1(&JsValue::UNDEFINED, &JsValue::from_str(""));
        }
    }

    /// Останавливает цикл, снимает обработчики, закрывает звук и убирает canvas, если его
    /// создала библиотека.
    pub fn destroy(&self) {
        let mut i = self.inner.borrow_mut();
        if i.destroyed {
            return;
        }
        i.destroyed = true;
        i.listeners.clear();
        i.emu = None;
        if let Some(port) = &i.audio.port {
            let _ = Reflect::set(port, &JsValue::from_str("onmessage"), &JsValue::NULL);
        }
        if i.audio.mode == Mode::Compat {
            if let Some(node) = &i.audio.node {
                let _ = Reflect::set(node, &JsValue::from_str("onaudioprocess"), &JsValue::NULL);
            }
        }
        if let Some(node) = &i.audio.node {
            let _ = call(node, "disconnect", &[]);
        }
        if let Some(ctx) = &i.audio.ctx {
            let _ = call(ctx, "close", &[]);
        }
        if i.owns_canvas {
            let _ = call(&i.canvas, "remove", &[]);
        }
    }
}
