mod apu;
mod bus;
mod cartridge;
mod controller;
mod cpu;
mod player;
mod ppu;

use bus::Bus;
use cartridge::Cartridge;
use cpu::Cpu;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    cpu: Cpu,
    bus: Bus,
}

#[wasm_bindgen]
impl Emulator {
    /// Создаёт эмулятор из байтов .nes файла. Бросает строку-ошибку в JS.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsValue> {
        console_error_panic_hook::set_once();
        let cart = Cartridge::from_ines(rom).map_err(|e| JsValue::from_str(&e))?;
        let mut bus = Bus::new(cart);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        Ok(Emulator { cpu, bus })
    }

    /// Сброс консоли (кнопка Reset).
    pub fn reset(&mut self) {
        self.bus.apu.reset();
        self.cpu.reset(&mut self.bus);
    }

    /// Прогоняет один полный кадр (~29780 тактов CPU).
    pub fn frame(&mut self) {
        self.bus.ppu.frame_complete = false;
        self.bus.apu.samples.clear();
        while !self.bus.ppu.frame_complete {
            self.step();
        }
    }

    /// Указатель на RGBA-буфер 256x240 в памяти wasm.
    pub fn frame_ptr(&self) -> *const u8 {
        self.bus.ppu.frame.as_ptr()
    }

    /// Частота дискретизации аудио (AudioContext.sampleRate). Вызывать после создания.
    pub fn set_sample_rate(&mut self, rate: f32) {
        self.bus.apu.set_sample_rate(rate as f64);
    }

    /// Указатель на f32-сэмплы (моно), накопленные за последний `frame()`.
    pub fn audio_ptr(&self) -> *const f32 {
        self.bus.apu.samples.as_ptr()
    }

    pub fn audio_len(&self) -> usize {
        self.bus.apu.samples.len()
    }

    /// Состояние кнопок игрока (0 или 1) целиком.
    /// Биты: 0=A, 1=B, 2=Select, 3=Start, 4=Up, 5=Down, 6=Left, 7=Right.
    pub fn set_pad(&mut self, player: u8, state: u8) {
        if let Some(pad) = self.bus.pads.get_mut(player as usize) {
            pad.state = state;
        }
    }
}

impl Emulator {
    /// RGBA-буфер кадра 256x240 (для высокоуровневого API).
    pub(crate) fn frame_buffer(&self) -> &[u8] {
        &self.bus.ppu.frame
    }

    /// Сэмплы звука, накопленные за последний кадр (моно, f32).
    pub(crate) fn audio_samples(&self) -> &[f32] {
        &self.bus.apu.samples
    }

    fn step(&mut self) {
        self.cpu.irq = self.bus.apu.irq_pending() || self.bus.cart.irq_pending();
        let mut cycles = self.cpu.step(&mut self.bus);
        if self.bus.dma_pending {
            self.bus.dma_pending = false;
            cycles += 513 + (self.cpu.cycles & 1) as u32;
        }
        for _ in 0..cycles {
            self.bus.apu.tick(&self.bus.cart);
        }
        // на 1 такт CPU приходится 3 такта PPU
        for _ in 0..cycles * 3 {
            self.bus.ppu.tick(&mut self.bus.cart);
            if self.bus.ppu.nmi {
                self.bus.ppu.nmi = false;
                self.cpu.nmi = true;
            }
        }
    }
}
