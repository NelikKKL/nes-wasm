use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::controller::Controller;
use crate::ppu::Ppu;

pub struct Bus {
    pub ram: [u8; 2048],
    pub ppu: Ppu,
    pub apu: Apu,
    pub cart: Cartridge,
    pub pads: [Controller; 2],
    pub dma_pending: bool,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Bus {
        Bus {
            ram: [0; 2048],
            ppu: Ppu::new(),
            apu: Apu::new(),
            cart,
            pads: [Controller::default(), Controller::default()],
            dma_pending: false,
        }
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.ram[(addr & 0x07FF) as usize],
            0x2000..=0x3FFF => self.ppu.read_reg(addr & 7, &mut self.cart),
            0x4015 => self.apu.read_status(),
            0x4016 => 0x40 | self.pads[0].read(),
            0x4017 => 0x40 | self.pads[1].read(),
            0x6000..=0xFFFF => self.cart.cpu_read(addr),
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x1FFF => self.ram[(addr & 0x07FF) as usize] = val,
            0x2000..=0x3FFF => self.ppu.write_reg(addr & 7, val, &mut self.cart),
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write_reg(addr, val),
            0x4014 => {
                let base = (val as u16) << 8;
                let mut buf = [0u8; 256];
                for i in 0..256u16 {
                    buf[i as usize] = self.read(base + i);
                }
                self.ppu.write_oam_dma(&buf);
                self.dma_pending = true;
            }
            0x4016 => {
                self.pads[0].write(val);
                self.pads[1].write(val);
            }
            0x6000..=0xFFFF => self.cart.cpu_write(addr, val),
            _ => {}
        }
    }

    pub fn read16(&mut self, addr: u16) -> u16 {
        let lo = self.read(addr) as u16;
        let hi = self.read(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }
}
