use crate::cartridge::Cartridge;

const CPU_HZ: f64 = 1_789_773.0;

#[rustfmt::skip]
const LENGTHS: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14,
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

const DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];

const NOISE_PERIODS: [u16; 16] = [4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068];
const DMC_RATES: [u16; 16] = [428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54];

// ---------- огибающая ----------

#[derive(Default)]
struct Envelope {
    start: bool,
    looping: bool,
    constant: bool,
    period: u8,
    divider: u8,
    decay: u8,
}

impl Envelope {
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.period;
        } else if self.divider == 0 {
            self.divider = self.period;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.looping {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn volume(&self) -> u8 {
        if self.constant {
            self.period
        } else {
            self.decay
        }
    }
}

// ---------- прямоугольный канал ----------

#[derive(Default)]
struct Pulse {
    ones_complement: bool, // у первого канала sweep считает через обратный код
    enabled: bool,
    duty: u8,
    seq: u8,
    timer: u16,
    period: u16,
    length: u8,
    halt: bool,
    env: Envelope,
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_divider: u8,
    sweep_reload: bool,
}

impl Pulse {
    fn new(ones_complement: bool) -> Pulse {
        Pulse { ones_complement, ..Default::default() }
    }

    fn write(&mut self, reg: u8, val: u8) {
        match reg {
            0 => {
                self.duty = val >> 6;
                self.halt = val & 0x20 != 0;
                self.env.looping = self.halt;
                self.env.constant = val & 0x10 != 0;
                self.env.period = val & 0x0F;
            }
            1 => {
                self.sweep_enabled = val & 0x80 != 0;
                self.sweep_period = (val >> 4) & 7;
                self.sweep_negate = val & 0x08 != 0;
                self.sweep_shift = val & 7;
                self.sweep_reload = true;
            }
            2 => self.period = (self.period & 0x0700) | val as u16,
            _ => {
                self.period = (self.period & 0x00FF) | (((val & 7) as u16) << 8);
                if self.enabled {
                    self.length = LENGTHS[(val >> 3) as usize];
                }
                self.seq = 0;
                self.env.start = true;
            }
        }
    }

    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    fn target(&self) -> i32 {
        let change = (self.period >> self.sweep_shift) as i32;
        if self.sweep_negate {
            let ones = if self.ones_complement { 1 } else { 0 };
            (self.period as i32 - change - ones).max(0)
        } else {
            self.period as i32 + change
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.seq = (self.seq + 1) & 7;
        } else {
            self.timer -= 1;
        }
    }

    fn clock_length(&mut self) {
        if self.length > 0 && !self.halt {
            self.length -= 1;
        }
    }

    fn clock_sweep(&mut self) {
        let target = self.target();
        if self.sweep_divider == 0
            && self.sweep_enabled
            && self.sweep_shift > 0
            && self.period >= 8
            && target <= 0x7FF
        {
            self.period = target as u16;
        }
        if self.sweep_divider == 0 || self.sweep_reload {
            self.sweep_divider = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_divider -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.period < 8 || self.target() > 0x7FF {
            return 0;
        }
        if DUTY[self.duty as usize][self.seq as usize] == 0 {
            return 0;
        }
        self.env.volume()
    }
}

// ---------- треугольный канал ----------

#[derive(Default)]
struct Triangle {
    enabled: bool,
    control: bool,
    linear_value: u8,
    linear: u8,
    linear_reload: bool,
    period: u16,
    timer: u16,
    seq: u8,
    length: u8,
}

impl Triangle {
    fn write(&mut self, reg: u8, val: u8) {
        match reg {
            0 => {
                self.control = val & 0x80 != 0;
                self.linear_value = val & 0x7F;
            }
            2 => self.period = (self.period & 0x0700) | val as u16,
            3 => {
                self.period = (self.period & 0x00FF) | (((val & 7) as u16) << 8);
                if self.enabled {
                    self.length = LENGTHS[(val >> 3) as usize];
                }
                self.linear_reload = true;
            }
            _ => {}
        }
    }

    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            if self.length > 0 && self.linear > 0 {
                self.seq = (self.seq + 1) & 31;
            }
        } else {
            self.timer -= 1;
        }
    }

    fn clock_linear(&mut self) {
        if self.linear_reload {
            self.linear = self.linear_value;
        } else if self.linear > 0 {
            self.linear -= 1;
        }
        if !self.control {
            self.linear_reload = false;
        }
    }

    fn clock_length(&mut self) {
        if self.length > 0 && !self.control {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.seq < 16 {
            15 - self.seq
        } else {
            self.seq - 16
        }
    }
}

// ---------- шум ----------

struct Noise {
    enabled: bool,
    halt: bool,
    env: Envelope,
    mode: bool,
    period: u16,
    timer: u16,
    shift: u16,
    length: u8,
}

impl Noise {
    fn new() -> Noise {
        Noise {
            enabled: false,
            halt: false,
            env: Envelope::default(),
            mode: false,
            period: NOISE_PERIODS[0],
            timer: NOISE_PERIODS[0],
            shift: 1,
            length: 0,
        }
    }

    fn write(&mut self, reg: u8, val: u8) {
        match reg {
            0 => {
                self.halt = val & 0x20 != 0;
                self.env.looping = self.halt;
                self.env.constant = val & 0x10 != 0;
                self.env.period = val & 0x0F;
            }
            2 => {
                self.mode = val & 0x80 != 0;
                self.period = NOISE_PERIODS[(val & 0x0F) as usize];
            }
            3 => {
                if self.enabled {
                    self.length = LENGTHS[(val >> 3) as usize];
                }
                self.env.start = true;
            }
            _ => {}
        }
    }

    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period - 1;
            let bit = if self.mode { 6 } else { 1 };
            let feedback = (self.shift & 1) ^ ((self.shift >> bit) & 1);
            self.shift = (self.shift >> 1) | (feedback << 14);
        } else {
            self.timer -= 1;
        }
    }

    fn clock_length(&mut self) {
        if self.length > 0 && !self.halt {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.shift & 1 == 1 {
            0
        } else {
            self.env.volume()
        }
    }
}

// ---------- DMC (дельта-модуляция, сэмплы из PRG) ----------

struct Dmc {
    irq_enable: bool,
    irq: bool,
    looping: bool,
    period: u16,
    timer: u16,
    level: u8,
    sample_addr: u16,
    sample_len: u16,
    addr: u16,
    remaining: u16,
    buffer: Option<u8>,
    shift: u8,
    bits: u8,
    silence: bool,
}

impl Dmc {
    fn new() -> Dmc {
        Dmc {
            irq_enable: false,
            irq: false,
            looping: false,
            period: DMC_RATES[0],
            timer: DMC_RATES[0],
            level: 0,
            sample_addr: 0xC000,
            sample_len: 1,
            addr: 0xC000,
            remaining: 0,
            buffer: None,
            shift: 0,
            bits: 8,
            silence: true,
        }
    }

    fn write(&mut self, reg: u8, val: u8) {
        match reg {
            0 => {
                self.irq_enable = val & 0x80 != 0;
                if !self.irq_enable {
                    self.irq = false;
                }
                self.looping = val & 0x40 != 0;
                self.period = DMC_RATES[(val & 0x0F) as usize];
            }
            1 => self.level = val & 0x7F,
            2 => self.sample_addr = 0xC000 | ((val as u16) << 6),
            _ => self.sample_len = ((val as u16) << 4) | 1,
        }
    }

    fn restart(&mut self) {
        self.addr = self.sample_addr;
        self.remaining = self.sample_len;
    }

    fn tick(&mut self, cart: &Cartridge) {
        // читатель памяти: подгружает следующий байт, когда буфер пуст
        if self.buffer.is_none() && self.remaining > 0 {
            self.buffer = Some(cart.cpu_read(self.addr));
            self.addr = if self.addr == 0xFFFF { 0x8000 } else { self.addr + 1 };
            self.remaining -= 1;
            if self.remaining == 0 {
                if self.looping {
                    self.restart();
                } else if self.irq_enable {
                    self.irq = true;
                }
            }
        }

        if self.timer == 0 {
            self.timer = self.period - 1;
            if !self.silence {
                if self.shift & 1 != 0 {
                    if self.level <= 125 {
                        self.level += 2;
                    }
                } else if self.level >= 2 {
                    self.level -= 2;
                }
            }
            self.shift >>= 1;
            self.bits -= 1;
            if self.bits == 0 {
                self.bits = 8;
                match self.buffer.take() {
                    Some(b) => {
                        self.shift = b;
                        self.silence = false;
                    }
                    None => self.silence = true,
                }
            }
        } else {
            self.timer -= 1;
        }
    }
}

// ---------- APU целиком ----------

pub struct Apu {
    pulse: [Pulse; 2],
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,

    mode5: bool,
    irq_inhibit: bool,
    frame_irq: bool,
    frame_cycle: u32,
    odd: bool,

    pulse_table: [f32; 31],
    tnd_table: [f32; 203],

    rate: f64,
    sample_period: f64, // тактов CPU на один выходной сэмпл
    sample_timer: f64,
    sum: f32,
    count: u32,
    hp_r: f32,
    hp_in: f32,
    hp_out: f32,

    /// Сэмплы, накопленные с последнего `samples.clear()`. Моно, f32.
    pub samples: Vec<f32>,
}

impl Apu {
    pub fn new() -> Apu {
        let mut pulse_table = [0f32; 31];
        for i in 1..31 {
            pulse_table[i] = 95.88 / (8128.0 / i as f32 + 100.0);
        }
        let mut tnd_table = [0f32; 203];
        for i in 1..203 {
            tnd_table[i] = 163.67 / (24329.0 / i as f32 + 100.0);
        }
        let mut apu = Apu {
            pulse: [Pulse::new(true), Pulse::new(false)],
            triangle: Triangle::default(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            mode5: false,
            irq_inhibit: false,
            frame_irq: false,
            frame_cycle: 0,
            odd: false,
            pulse_table,
            tnd_table,
            rate: 48000.0,
            sample_period: 0.0,
            sample_timer: 0.0,
            sum: 0.0,
            count: 0,
            hp_r: 0.0,
            hp_in: 0.0,
            hp_out: 0.0,
            samples: Vec::with_capacity(2048),
        };
        apu.set_sample_rate(48000.0);
        apu
    }

    pub fn reset(&mut self) {
        let rate = self.rate;
        *self = Apu::new();
        self.set_sample_rate(rate);
    }

    pub fn set_sample_rate(&mut self, rate: f64) {
        if rate < 8000.0 || rate > 192000.0 {
            return;
        }
        self.rate = rate;
        self.sample_period = CPU_HZ / rate;
        // однополюсный high-pass ~40 Гц убирает постоянную составляющую
        self.hp_r = 1.0 - (2.0 * std::f32::consts::PI * 40.0 / rate as f32);
    }

    pub fn irq_pending(&self) -> bool {
        self.frame_irq || self.dmc.irq
    }

    pub fn read_status(&mut self) -> u8 {
        let mut s = 0;
        if self.pulse[0].length > 0 {
            s |= 0x01;
        }
        if self.pulse[1].length > 0 {
            s |= 0x02;
        }
        if self.triangle.length > 0 {
            s |= 0x04;
        }
        if self.noise.length > 0 {
            s |= 0x08;
        }
        if self.dmc.remaining > 0 {
            s |= 0x10;
        }
        if self.frame_irq {
            s |= 0x40;
        }
        if self.dmc.irq {
            s |= 0x80;
        }
        self.frame_irq = false;
        s
    }

    pub fn write_reg(&mut self, addr: u16, val: u8) {
        match addr {
            0x4000..=0x4003 => self.pulse[0].write((addr - 0x4000) as u8, val),
            0x4004..=0x4007 => self.pulse[1].write((addr - 0x4004) as u8, val),
            0x4008..=0x400B => self.triangle.write((addr - 0x4008) as u8, val),
            0x400C..=0x400F => self.noise.write((addr - 0x400C) as u8, val),
            0x4010..=0x4013 => self.dmc.write((addr - 0x4010) as u8, val),
            0x4015 => {
                self.pulse[0].set_enabled(val & 0x01 != 0);
                self.pulse[1].set_enabled(val & 0x02 != 0);
                self.triangle.set_enabled(val & 0x04 != 0);
                self.noise.set_enabled(val & 0x08 != 0);
                self.dmc.irq = false;
                if val & 0x10 != 0 {
                    if self.dmc.remaining == 0 {
                        self.dmc.restart();
                    }
                } else {
                    self.dmc.remaining = 0;
                }
            }
            0x4017 => {
                self.mode5 = val & 0x80 != 0;
                self.irq_inhibit = val & 0x40 != 0;
                if self.irq_inhibit {
                    self.frame_irq = false;
                }
                self.frame_cycle = 0;
                if self.mode5 {
                    self.quarter();
                    self.half();
                }
            }
            _ => {}
        }
    }

    fn quarter(&mut self) {
        self.pulse[0].env.clock();
        self.pulse[1].env.clock();
        self.noise.env.clock();
        self.triangle.clock_linear();
    }

    fn half(&mut self) {
        for p in self.pulse.iter_mut() {
            p.clock_length();
            p.clock_sweep();
        }
        self.triangle.clock_length();
        self.noise.clock_length();
    }

    /// Один такт CPU.
    pub fn tick(&mut self, cart: &Cartridge) {
        self.frame_cycle += 1;
        match (self.mode5, self.frame_cycle) {
            (_, 7457) | (_, 22371) => self.quarter(),
            (_, 14913) => {
                self.quarter();
                self.half();
            }
            (false, 29829) => {
                self.quarter();
                self.half();
                if !self.irq_inhibit {
                    self.frame_irq = true;
                }
            }
            (false, 29830) => self.frame_cycle = 0,
            (true, 37281) => {
                self.quarter();
                self.half();
            }
            (true, 37282) => self.frame_cycle = 0,
            _ => {}
        }

        self.triangle.clock_timer();
        self.noise.clock_timer();
        self.dmc.tick(cart);
        if self.odd {
            self.pulse[0].clock_timer();
            self.pulse[1].clock_timer();
        }
        self.odd = !self.odd;

        // нелинейный микшер NES
        let p = (self.pulse[0].output() + self.pulse[1].output()) as usize;
        let tnd = 3 * self.triangle.output() as usize
            + 2 * self.noise.output() as usize
            + self.dmc.level as usize;
        self.sum += self.pulse_table[p] + self.tnd_table[tnd];
        self.count += 1;

        // усредняем такты CPU внутри одного выходного сэмпла (box-фильтр)
        self.sample_timer += 1.0;
        if self.sample_timer >= self.sample_period {
            self.sample_timer -= self.sample_period;
            let avg = self.sum / self.count as f32;
            self.sum = 0.0;
            self.count = 0;
            let out = avg - self.hp_in + self.hp_r * self.hp_out;
            self.hp_in = avg;
            self.hp_out = out;
            self.samples.push(out.clamp(-1.0, 1.0));
        }
    }
}
