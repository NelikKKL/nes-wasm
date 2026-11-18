use crate::cartridge::{Cartridge, Mirroring};

pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 240;

#[rustfmt::skip]
const PALETTE: [u8; 192] = [
    0x54,0x54,0x54, 0x00,0x1E,0x74, 0x08,0x10,0x90, 0x30,0x00,0x88, 0x44,0x00,0x64, 0x5C,0x00,0x30, 0x54,0x04,0x00, 0x3C,0x18,0x00,
    0x20,0x2A,0x00, 0x08,0x3A,0x00, 0x00,0x40,0x00, 0x00,0x3C,0x00, 0x00,0x32,0x3C, 0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00,
    0x98,0x96,0x98, 0x08,0x4C,0xC4, 0x30,0x32,0xEC, 0x5C,0x1E,0xE4, 0x88,0x14,0xB0, 0xA0,0x14,0x64, 0x98,0x22,0x20, 0x78,0x3C,0x00,
    0x54,0x5A,0x00, 0x28,0x72,0x00, 0x08,0x7C,0x00, 0x00,0x76,0x28, 0x00,0x66,0x78, 0x00,0x00,0x00, 0x00,0x00,0x00, 0x00,0x00,0x00,
    0xEC,0xEE,0xEC, 0x4C,0x9A,0xEC, 0x78,0x7C,0xEC, 0xB0,0x62,0xEC, 0xE4,0x54,0xEC, 0xEC,0x58,0xB4, 0xEC,0x6A,0x64, 0xD4,0x88,0x20,
    0xA0,0xAA,0x00, 0x74,0xC4,0x00, 0x4C,0xD0,0x20, 0x38,0xCC,0x6C, 0x38,0xB4,0xCC, 0x3C,0x3C,0x3C, 0x00,0x00,0x00, 0x00,0x00,0x00,
    0xEC,0xEE,0xEC, 0xA8,0xCC,0xEC, 0xBC,0xBC,0xEC, 0xD4,0xB2,0xEC, 0xEC,0xAE,0xEC, 0xEC,0xAE,0xD4, 0xEC,0xB4,0xB0, 0xE4,0xC4,0x90,
    0xCC,0xD2,0x78, 0xB4,0xDE,0x78, 0xA8,0xE2,0x90, 0x98,0xE2,0xB4, 0xA0,0xD6,0xE4, 0xA0,0xA2,0xA0, 0x00,0x00,0x00, 0x00,0x00,0x00,
];

pub struct Ppu {
    /// RGBA, 256x240
    pub frame: Vec<u8>,
    pub frame_complete: bool,
    pub nmi: bool,

    ctrl: u8,
    mask: u8,
    status: u8,
    oam: [u8; 256],
    oam_addr: u8,
    vram: [u8; 4096],
    palette: [u8; 32],

    // внутренние регистры скролла (loopy)
    v: u16,
    t: u16,
    fine_x: u8,
    w: bool,
    buffer: u8,

    scanline: i16, // -1 (pre-render) ..= 260
    cycle: u16,    // 0..=340
    odd_frame: bool,

    // фон: данные следующего тайла и сдвиговые регистры
    nt_id: u8,
    nt_attr: u8,
    nt_lo: u8,
    nt_hi: u8,
    bg_lo: u16,
    bg_hi: u16,
    at_lo: u16,
    at_hi: u16,

    // спрайты текущей строки
    spr_count: usize,
    spr_zero: bool,
    spr_x: [u8; 8],
    spr_attr: [u8; 8],
    spr_lo: [u8; 8],
    spr_hi: [u8; 8],
}

fn nt_index(addr: u16, m: Mirroring) -> usize {
    let a = ((addr - 0x2000) & 0x0FFF) as usize;
    let table = a >> 10;
    let off = a & 0x03FF;
    let t = match m {
        Mirroring::Vertical => table & 1,
        Mirroring::Horizontal => table >> 1,
        Mirroring::SingleLow => 0,
        Mirroring::SingleHigh => 1,
        Mirroring::FourScreen => table,
    };
    t * 0x400 + off
}

fn pal_index(addr: u16) -> usize {
    let i = (addr & 0x1F) as usize;
    if i & 0x13 == 0x10 {
        i & 0x0F
    } else {
        i
    }
}

impl Ppu {
    pub fn new() -> Ppu {
        let mut frame = vec![0u8; WIDTH * HEIGHT * 4];
        for px in frame.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ppu {
            frame,
            frame_complete: false,
            nmi: false,
            ctrl: 0,
            mask: 0,
            status: 0,
            oam: [0; 256],
            oam_addr: 0,
            vram: [0; 4096],
            palette: [0; 32],
            v: 0,
            t: 0,
            fine_x: 0,
            w: false,
            buffer: 0,
            scanline: 0,
            cycle: 0,
            odd_frame: false,
            nt_id: 0,
            nt_attr: 0,
            nt_lo: 0,
            nt_hi: 0,
            bg_lo: 0,
            bg_hi: 0,
            at_lo: 0,
            at_hi: 0,
            spr_count: 0,
            spr_zero: false,
            spr_x: [0; 8],
            spr_attr: [0; 8],
            spr_lo: [0; 8],
            spr_hi: [0; 8],
        }
    }

    // ---------- шина PPU ----------

    fn read(&mut self, addr: u16, cart: &mut Cartridge) -> u8 {
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => cart.ppu_read(addr),
            0x2000..=0x3EFF => self.vram[nt_index(addr, cart.mirroring())],
            _ => self.palette[pal_index(addr)],
        }
    }

    fn write(&mut self, addr: u16, val: u8, cart: &mut Cartridge) {
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => cart.ppu_write(addr, val),
            0x2000..=0x3EFF => self.vram[nt_index(addr, cart.mirroring())] = val,
            _ => self.palette[pal_index(addr)] = val,
        }
    }

    fn inc(&self) -> u16 {
        if self.ctrl & 0x04 != 0 {
            32
        } else {
            1
        }
    }

    // ---------- регистры $2000-$2007 ----------

    pub fn read_reg(&mut self, reg: u16, cart: &mut Cartridge) -> u8 {
        match reg {
            2 => {
                let r = (self.status & 0xE0) | (self.buffer & 0x1F);
                self.status &= !0x80;
                self.w = false;
                r
            }
            4 => self.oam[self.oam_addr as usize],
            7 => {
                let addr = self.v & 0x3FFF;
                let mut data = self.buffer;
                if addr >= 0x3F00 {
                    data = self.read(addr, cart);
                    self.buffer = self.read(addr - 0x1000, cart);
                } else {
                    self.buffer = self.read(addr, cart);
                }
                self.v = self.v.wrapping_add(self.inc());
                data
            }
            _ => 0,
        }
    }

    pub fn write_reg(&mut self, reg: u16, val: u8, cart: &mut Cartridge) {
        match reg {
            0 => {
                let was_nmi = self.ctrl & 0x80 != 0;
                self.ctrl = val;
                self.t = (self.t & 0xF3FF) | (((val & 3) as u16) << 10);
                if !was_nmi && val & 0x80 != 0 && self.status & 0x80 != 0 {
                    self.nmi = true;
                }
            }
            1 => self.mask = val,
            3 => self.oam_addr = val,
            4 => {
                self.oam[self.oam_addr as usize] = val;
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            5 => {
                if !self.w {
                    self.fine_x = val & 7;
                    self.t = (self.t & 0xFFE0) | ((val >> 3) as u16);
                } else {
                    self.t = (self.t & 0x8FFF) | (((val & 7) as u16) << 12);
                    self.t = (self.t & 0xFC1F) | (((val >> 3) as u16) << 5);
                }
                self.w = !self.w;
            }
            6 => {
                if !self.w {
                    self.t = (self.t & 0x00FF) | (((val & 0x3F) as u16) << 8);
                } else {
                    self.t = (self.t & 0xFF00) | val as u16;
                    self.v = self.t;
                }
                self.w = !self.w;
            }
            7 => {
                let addr = self.v & 0x3FFF;
                self.write(addr, val, cart);
                self.v = self.v.wrapping_add(self.inc());
            }
            _ => {}
        }
    }

    pub fn write_oam_dma(&mut self, data: &[u8; 256]) {
        for i in 0..256usize {
            self.oam[(self.oam_addr as usize + i) & 0xFF] = data[i];
        }
    }

    // ---------- скролл ----------

    fn inc_x(&mut self) {
        if self.v & 0x001F == 31 {
            self.v &= !0x001F;
            self.v ^= 0x0400;
        } else {
            self.v += 1;
        }
    }

    fn inc_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000;
        } else {
            self.v &= !0x7000;
            let mut y = (self.v & 0x03E0) >> 5;
            if y == 29 {
                y = 0;
                self.v ^= 0x0800;
            } else if y == 31 {
                y = 0;
            } else {
                y += 1;
            }
            self.v = (self.v & !0x03E0) | (y << 5);
        }
    }

    // ---------- фон ----------

    fn shift_bg(&mut self) {
        self.bg_lo <<= 1;
        self.bg_hi <<= 1;
        self.at_lo <<= 1;
        self.at_hi <<= 1;
    }

    fn load_bg(&mut self) {
        self.bg_lo = (self.bg_lo & 0xFF00) | self.nt_lo as u16;
        self.bg_hi = (self.bg_hi & 0xFF00) | self.nt_hi as u16;
        let a_lo: u16 = if self.nt_attr & 1 != 0 { 0xFF } else { 0 };
        let a_hi: u16 = if self.nt_attr & 2 != 0 { 0xFF } else { 0 };
        self.at_lo = (self.at_lo & 0xFF00) | a_lo;
        self.at_hi = (self.at_hi & 0xFF00) | a_hi;
    }

    fn bg_addr(&self) -> u16 {
        ((((self.ctrl >> 4) & 1) as u16) << 12) + ((self.nt_id as u16) << 4) + ((self.v >> 12) & 7)
    }

    fn fetch_step(&mut self, cy: u16, cart: &mut Cartridge) {
        match (cy - 1) % 8 {
            0 => {
                self.load_bg();
                self.nt_id = self.read(0x2000 | (self.v & 0x0FFF), cart);
            }
            2 => {
                let v = self.v;
                let addr = 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07);
                let mut a = self.read(addr, cart);
                if v & 0x40 != 0 {
                    a >>= 4;
                }
                if v & 0x02 != 0 {
                    a >>= 2;
                }
                self.nt_attr = a & 3;
            }
            4 => {
                let addr = self.bg_addr();
                self.nt_lo = self.read(addr, cart);
            }
            6 => {
                let addr = self.bg_addr() + 8;
                self.nt_hi = self.read(addr, cart);
            }
            7 => self.inc_x(),
            _ => {}
        }
    }

    // ---------- спрайты ----------

    /// Вызывается на 257-м такте строки `sl`: готовит спрайты для строки `sl + 1`.
    fn eval_sprites(&mut self, sl: i16, cart: &mut Cartridge) {
        let h: i16 = if self.ctrl & 0x20 != 0 { 16 } else { 8 };
        for i in 0..64usize {
            let y = self.oam[i * 4] as i16;
            let row = sl - y;
            if row < 0 || row >= h {
                continue;
            }
            if self.spr_count == 8 {
                self.status |= 0x20; // sprite overflow
                break;
            }
            let tile = self.oam[i * 4 + 1] as u16;
            let attr = self.oam[i * 4 + 2];
            let x = self.oam[i * 4 + 3];
            let mut row = row as u16;
            if attr & 0x80 != 0 {
                row = (h as u16) - 1 - row; // flip vertical
            }
            let addr = if h == 8 {
                ((((self.ctrl >> 3) & 1) as u16) << 12) + (tile << 4) + row
            } else {
                ((tile & 1) << 12) + ((tile & 0xFE) << 4) + ((row & 8) << 1) + (row & 7)
            };
            let n = self.spr_count;
            self.spr_lo[n] = cart.ppu_read(addr);
            self.spr_hi[n] = cart.ppu_read(addr + 8);
            self.spr_x[n] = x;
            self.spr_attr[n] = attr;
            if i == 0 {
                self.spr_zero = true;
            }
            self.spr_count += 1;
        }
    }

    // ---------- вывод пикселя ----------

    fn draw_pixel(&mut self) {
        let x = (self.cycle - 1) as usize;
        let y = self.scanline as usize;

        let mut bg: u8 = 0;
        let mut bg_pal: u8 = 0;
        if self.mask & 0x08 != 0 && (x >= 8 || self.mask & 0x02 != 0) {
            let m = 0x8000u16 >> self.fine_x;
            bg = (((self.bg_hi & m != 0) as u8) << 1) | ((self.bg_lo & m != 0) as u8);
            bg_pal = (((self.at_hi & m != 0) as u8) << 1) | ((self.at_lo & m != 0) as u8);
        }

        let mut fg: u8 = 0;
        let mut fg_pal: u8 = 0;
        let mut fg_front = false;
        let mut fg_zero = false;
        if self.mask & 0x10 != 0 && (x >= 8 || self.mask & 0x04 != 0) {
            for i in 0..self.spr_count {
                let dx = x as i16 - self.spr_x[i] as i16;
                if !(0..8).contains(&dx) {
                    continue;
                }
                let bit = (if self.spr_attr[i] & 0x40 != 0 { dx } else { 7 - dx }) as u8;
                let px = (((self.spr_hi[i] >> bit) & 1) << 1) | ((self.spr_lo[i] >> bit) & 1);
                if px == 0 {
                    continue;
                }
                fg = px;
                fg_pal = (self.spr_attr[i] & 3) + 4;
                fg_front = self.spr_attr[i] & 0x20 == 0;
                fg_zero = i == 0 && self.spr_zero;
                break;
            }
        }

        if fg_zero && bg != 0 && x != 255 {
            self.status |= 0x40; // sprite 0 hit
        }

        let (px, pal) = match (bg != 0, fg != 0) {
            (false, false) => (0u8, 0u8),
            (false, true) => (fg, fg_pal),
            (true, false) => (bg, bg_pal),
            (true, true) => {
                if fg_front {
                    (fg, fg_pal)
                } else {
                    (bg, bg_pal)
                }
            }
        };

        let idx = if px == 0 {
            self.palette[0]
        } else {
            self.palette[((pal << 2) | px) as usize]
        };
        let mut ci = (idx & 0x3F) as usize;
        if self.mask & 0x01 != 0 {
            ci &= 0x30; // greyscale
        }
        let o = (y * WIDTH + x) * 4;
        self.frame[o] = PALETTE[ci * 3];
        self.frame[o + 1] = PALETTE[ci * 3 + 1];
        self.frame[o + 2] = PALETTE[ci * 3 + 2];
    }

    // ---------- один такт PPU ----------

    pub fn tick(&mut self, cart: &mut Cartridge) {
        let rendering = self.mask & 0x18 != 0;
        let sl = self.scanline;
        let cy = self.cycle;

        if sl < 240 {
            if sl == -1 && cy == 1 {
                self.status &= !0xE0; // vblank, sprite 0 hit, overflow
            }
            if rendering {
                if (2..=257).contains(&cy) || (321..=337).contains(&cy) {
                    if self.mask & 0x08 != 0 {
                        self.shift_bg();
                    }
                    self.fetch_step(cy, cart);
                }
                if cy == 256 {
                    self.inc_y();
                }
                if cy == 257 {
                    self.v = (self.v & !0x041F) | (self.t & 0x041F);
                }
                if sl == -1 && (280..=304).contains(&cy) {
                    self.v = (self.v & !0x7BE0) | (self.t & 0x7BE0);
                }
                if cy == 338 || cy == 340 {
                    self.nt_id = self.read(0x2000 | (self.v & 0x0FFF), cart);
                }
            }
            if cy == 257 {
                self.spr_count = 0;
                self.spr_zero = false;
                if rendering && sl >= 0 {
                    self.eval_sprites(sl, cart);
                }
            }
            if sl >= 0 && (1..=256).contains(&cy) {
                self.draw_pixel();
            }
        }

        if sl == 241 && cy == 1 {
            self.status |= 0x80;
            if self.ctrl & 0x80 != 0 {
                self.nmi = true;
            }
        }

        self.cycle += 1;
        // на нечётных кадрах pre-render строка на такт короче
        if self.scanline == -1 && self.cycle == 339 && self.odd_frame && rendering {
            self.cycle = 340;
        }
        if self.cycle > 340 {
            self.cycle = 0;
            self.scanline += 1;
            if self.scanline > 260 {
                self.scanline = -1;
                self.frame_complete = true;
                self.odd_frame = !self.odd_frame;
            }
        }
    }
}
