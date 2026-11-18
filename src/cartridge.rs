#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mirroring {
    Horizontal,
    Vertical,
    SingleLow,
    SingleHigh,
    FourScreen,
}

/// Картридж + мапперы 0 (NROM), 1 (MMC1), 2 (UxROM), 3 (CNROM), 7 (AxROM).
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
        if !matches!(mapper, 0 | 1 | 2 | 3 | 7) {
            return Err(format!("Маппер {} пока не поддерживается", mapper));
        }

        let prg = data[start..start + prg_size].to_vec();
        let (chr, chr_ram) = if chr_size == 0 {
            (vec![0u8; 8192], true)
        } else {
            (data[start + prg_size..start + prg_size + chr_size].to_vec(), false)
        };

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
        })
    }

    pub fn mirroring(&self) -> Mirroring {
        self.mirroring
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
            _ => a,
        };
        idx % self.chr.len()
    }

    pub fn ppu_read(&mut self, addr: u16) -> u8 {
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
