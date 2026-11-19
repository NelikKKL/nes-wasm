use crate::state::{fnv1a, fnv1a_from, Reader, Writer};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    SingleLow,
    SingleHigh,
    FourScreen,
}

/// Картридж + мапперы 0 (NROM), 1 (MMC1), 2 (UxROM), 3 (CNROM), 4 (MMC3), 7 (AxROM).
pub struct Cartridge {
    mapper: u8,
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    prg_ram: Vec<u8>,
    mirroring: Mirroring,
    // регистры мапперов (часть используется только MMC1)
    shift: u8,
    control: u8,
    chr0: u8,
    chr1: u8,
    prg_bank: u8,
    // MMC3
    mmc3_cmd: u8,
    bank_regs: [u8; 8],
    irq_latch: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enable: bool,
    irq_pending: bool,
    a12: bool,
    a12_low_since: u64,
    ppu_clock: u64,
    /// Отпечаток ROM: сохранение подходит только к той игре, из которой сделано.
    rom_id: u32,
}

impl Cartridge {
    pub fn from_ines(data: &[u8]) -> Result<Cartridge, String> {
        if data.len() < 16 || &data[0..4] != b"NES\x1a" {
            return Err("Это не .nes файл (нет заголовка iNES)".to_string());
        }
        let prg_size = data[4] as usize * 16 * 1024;
        let chr_size = data[5] as usize * 8 * 1024;
        let f6 = data[6];
        let f7 = data[7];
        let mapper = (f7 & 0xF0) | (f6 >> 4);
        let mirroring = if f6 & 0x08 != 0 {
            Mirroring::FourScreen
        } else if f6 & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        let start = 16 + if f6 & 0x04 != 0 { 512 } else { 0 };

        if prg_size == 0 || data.len() < start + prg_size + chr_size {
            return Err("ROM повреждён или обрезан".to_string());
        }
        if !matches!(mapper, 0 | 1 | 2 | 3 | 4 | 7) {
            return Err(format!("Маппер {} пока не поддерживается", mapper));
        }

        let prg = data[start..start + prg_size].to_vec();
        let (chr, chr_ram) = if chr_size == 0 {
            (vec![0u8; 8192], true)
        } else {
            (data[start + prg_size..start + prg_size + chr_size].to_vec(), false)
        };

        let mut rom_id = fnv1a(&prg);
        if !chr_ram {
            rom_id = fnv1a_from(rom_id, &chr);
        }
        rom_id = fnv1a_from(rom_id, &[mapper]);

        Ok(Cartridge {
            mapper,
            prg,
            chr,
            chr_ram,
            prg_ram: vec![0u8; 8192],
            mirroring,
            shift: 0x10,
            control: 0x0C,
            chr0: 0,
            chr1: 0,
            prg_bank: 0,
            mmc3_cmd: 0,
            bank_regs: [0, 2, 4, 5, 6, 7, 0, 1],
            irq_latch: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enable: false,
            irq_pending: false,
            a12: false,
            a12_low_since: 0,
            ppu_clock: 0,
            rom_id,
        })
    }

    pub fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    pub fn rom_id(&self) -> u32 {
        self.rom_id
    }

    // ---------- сохранения ----------

    pub fn save(&self, out: &mut Writer) {
        out.bytes(&self.prg_ram);
        if self.chr_ram {
            out.bytes(&self.chr);
        }
        out.u8(match self.mirroring {
            Mirroring::Horizontal => 0,
            Mirroring::Vertical => 1,
            Mirroring::SingleLow => 2,
            Mirroring::SingleHigh => 3,
            Mirroring::FourScreen => 4,
        });
        out.u8(self.shift);
        out.u8(self.control);
        out.u8(self.chr0);
        out.u8(self.chr1);
        out.u8(self.prg_bank);
        out.u8(self.mmc3_cmd);
        out.bytes(&self.bank_regs);
        out.u8(self.irq_latch);
        out.u8(self.irq_counter);
        out.bool(self.irq_reload);
        out.bool(self.irq_enable);
        out.bool(self.irq_pending);
        out.bool(self.a12);
        out.u64(self.a12_low_since);
        out.u64(self.ppu_clock);
    }

    pub fn load(&mut self, r: &mut Reader) -> Result<(), String> {
        r.bytes(&mut self.prg_ram)?;
        if self.chr_ram {
            r.bytes(&mut self.chr)?;
        }
        self.mirroring = match r.u8()? {
            0 => Mirroring::Horizontal,
            1 => Mirroring::Vertical,
            2 => Mirroring::SingleLow,
            3 => Mirroring::SingleHigh,
            4 => Mirroring::FourScreen,
            _ => return Err("Сохранение повреждено: неверное зеркалирование".to_string()),
        };
        self.shift = r.u8()?;
        self.control = r.u8()?;
        self.chr0 = r.u8()?;
        self.chr1 = r.u8()?;
        self.prg_bank = r.u8()?;
        self.mmc3_cmd = r.u8()?;
        r.bytes(&mut self.bank_regs)?;
        self.irq_latch = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enable = r.bool()?;
        self.irq_pending = r.bool()?;
        self.a12 = r.bool()?;
        self.a12_low_since = r.u64()?;
        self.ppu_clock = r.u64()?;
        Ok(())
    }

    /// PPU сообщает текущий такт (в точках PPU) — нужно MMC3 для фильтра линии A12.
    pub fn set_ppu_clock(&mut self, dots: u64) {
        self.ppu_clock = dots;
    }

    /// Линия IRQ маппера (MMC3).
    pub fn irq_pending(&self) -> bool {
        self.irq_pending
    }

    /// MMC3 считает фронты A12 (0 -> 1). Фронт засчитывается, только если линия
    /// перед этим была в нуле достаточно долго — как фильтр по M2 в железе.
    fn watch_a12(&mut self, addr: u16) {
        let high = addr & 0x1000 != 0;
        if high && !self.a12 {
            if self.ppu_clock.wrapping_sub(self.a12_low_since) >= 9 {
                self.clock_irq();
            }
        } else if !high && self.a12 {
            self.a12_low_since = self.ppu_clock;
        }
        self.a12 = high;
    }

    fn clock_irq(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter -= 1;
        }
        if self.irq_counter == 0 && self.irq_enable {
            self.irq_pending = true;
        }
    }

    // ---------- CPU сторона ----------

    pub fn cpu_read(&self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize],
            0x8000..=0xFFFF => self.prg[self.prg_index(addr)],
            _ => 0,
        }
    }

    pub fn cpu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x6000..=0x7FFF => self.prg_ram[(addr - 0x6000) as usize] = val,
            0x8000..=0xFFFF => self.write_reg(addr, val),
            _ => {}
        }
    }

    fn prg_index(&self, addr: u16) -> usize {
        let off = (addr & 0x7FFF) as usize; // 0..0x7FFF внутри окна $8000-$FFFF
        let len = self.prg.len();
        let idx = match self.mapper {
            1 => {
                let banks = len / 0x4000;
                let bank = (self.prg_bank & 0x0F) as usize;
                match (self.control >> 2) & 3 {
                    0 | 1 => (bank & !1) * 0x4000 + off, // 32K
                    2 => {
                        if off < 0x4000 {
                            off
                        } else {
                            bank * 0x4000 + (off - 0x4000)
                        }
                    }
                    _ => {
                        if off < 0x4000 {
                            bank * 0x4000 + off
                        } else {
                            (banks - 1) * 0x4000 + (off - 0x4000)
                        }
                    }
                }
            }
            2 => {
                if off < 0x4000 {
                    self.prg_bank as usize * 0x4000 + off
                } else {
                    len - 0x4000 + (off - 0x4000)
                }
            }
            4 => {
                let n = len / 0x2000; // число 8K-банков
                let swap = self.mmc3_cmd & 0x40 != 0;
                let bank = match (off >> 13, swap) {
                    (0, false) => self.bank_regs[6] as usize,
                    (0, true) => n - 2,
                    (1, _) => self.bank_regs[7] as usize,
                    (2, false) => n - 2,
                    (2, true) => self.bank_regs[6] as usize,
                    _ => n - 1,
                };
                (bank % n) * 0x2000 + (off & 0x1FFF)
            }
            7 => (self.prg_bank as usize & 7) * 0x8000 + off,
            _ => off, // 0 и 3
        };
        idx % len
    }

    fn write_reg(&mut self, addr: u16, val: u8) {
        match self.mapper {
            1 => {
                if val & 0x80 != 0 {
                    self.shift = 0x10;
                    self.control |= 0x0C;
                } else {
                    let done = self.shift & 1 != 0;
                    self.shift = (self.shift >> 1) | ((val & 1) << 4);
                    if done {
                        let v = self.shift;
                        match (addr >> 13) & 3 {
                            0 => {
                                self.control = v;
                                self.mirroring = match v & 3 {
                                    0 => Mirroring::SingleLow,
                                    1 => Mirroring::SingleHigh,
                                    2 => Mirroring::Vertical,
                                    _ => Mirroring::Horizontal,
                                };
                            }
                            1 => self.chr0 = v,
                            2 => self.chr1 = v,
                            _ => self.prg_bank = v & 0x0F,
                        }
                        self.shift = 0x10;
                    }
                }
            }
            2 => self.prg_bank = val,
            3 => self.chr0 = val,
            4 => match addr & 0xE001 {
                0x8000 => self.mmc3_cmd = val,
                0x8001 => {
                    let r = (self.mmc3_cmd & 7) as usize;
                    self.bank_regs[r] = val;
                }
                0xA000 => {
                    if self.mirroring != Mirroring::FourScreen {
                        self.mirroring = if val & 1 != 0 { Mirroring::Horizontal } else { Mirroring::Vertical };
                    }
                }
                0xC000 => self.irq_latch = val,
                0xC001 => {
                    self.irq_counter = 0;
                    self.irq_reload = true;
                }
                0xE000 => {
                    self.irq_enable = false;
                    self.irq_pending = false;
                }
                0xE001 => self.irq_enable = true,
                _ => {}
            },
            7 => {
                self.prg_bank = val & 7;
                self.mirroring = if val & 0x10 != 0 {
                    Mirroring::SingleHigh
                } else {
                    Mirroring::SingleLow
                };
            }
            _ => {}
        }
    }

    // ---------- PPU сторона (CHR, $0000-$1FFF) ----------

    fn chr_index(&self, addr: u16) -> usize {
        let a = (addr & 0x1FFF) as usize;
        let idx = match self.mapper {
            1 => {
                if self.control & 0x10 == 0 {
                    (self.chr0 as usize & !1) * 0x1000 + a // 8K режим
                } else if a < 0x1000 {
                    self.chr0 as usize * 0x1000 + a
                } else {
                    self.chr1 as usize * 0x1000 + (a - 0x1000)
                }
            }
            3 => self.chr0 as usize * 0x2000 + a,
            4 => {
                let r = &self.bank_regs;
                let slot = a >> 10; // номер 1K-окна: 0..7
                let bank = if self.mmc3_cmd & 0x80 == 0 {
                    match slot {
                        0 => r[0] & 0xFE,
                        1 => r[0] | 1,
                        2 => r[1] & 0xFE,
                        3 => r[1] | 1,
                        4 => r[2],
                        5 => r[3],
                        6 => r[4],
                        _ => r[5],
                    }
                } else {
                    match slot {
                        0 => r[2],
                        1 => r[3],
                        2 => r[4],
                        3 => r[5],
                        4 => r[0] & 0xFE,
                        5 => r[0] | 1,
                        6 => r[1] & 0xFE,
                        _ => r[1] | 1,
                    }
                };
                bank as usize * 0x400 + (a & 0x3FF)
            }
            _ => a,
        };
        idx % self.chr.len()
    }

    pub fn ppu_read(&mut self, addr: u16) -> u8 {
        if self.mapper == 4 {
            self.watch_a12(addr);
        }
        let i = self.chr_index(addr);
        self.chr[i]
    }

    pub fn ppu_write(&mut self, addr: u16, val: u8) {
        if self.chr_ram {
            let i = self.chr_index(addr);
            self.chr[i] = val;
        }
    }
}
