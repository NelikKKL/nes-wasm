use crate::bus::Bus;
use crate::state::{Reader, Writer};

const FC: u8 = 0x01;
const FZ: u8 = 0x02;
const FI: u8 = 0x04;
const FD: u8 = 0x08;
const FB: u8 = 0x10;
const FU: u8 = 0x20;
const FV: u8 = 0x40;
const FN: u8 = 0x80;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Ind,
    Izx,
    Izy,
    Rel,
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    ADC, AND, ASL, BCC, BCS, BEQ, BIT, BMI, BNE, BPL, BRK, BVC, BVS, CLC, CLD, CLI, CLV,
    CMP, CPX, CPY, DEC, DEX, DEY, EOR, INC, INX, INY, JMP, JSR, LDA, LDX, LDY, LSR, NOP,
    ORA, PHA, PHP, PLA, PLP, ROL, ROR, RTI, RTS, SBC, SEC, SED, SEI, STA, STX, STY, TAX,
    TAY, TSX, TXA, TXS, TYA,
    // неофициальные
    ALR, ANC, ARR, AXS, DCP, ISB, LAX, RLA, RRA, SAX, SLO, SRE,
    // неофициальные «нестабильные» и зависание процессора
    AHX, ATX, KIL, LAS, SHX, SHY, TAS, XAA,
}

pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
    pub cycles: u64,
    /// Выставляется PPU, обрабатывается в начале следующего шага.
    pub nmi: bool,
    /// Уровень линии IRQ (APU, позже мапперы). Обрабатывается, если флаг I сброшен.
    pub irq: bool,
    /// Процессор завис после KIL/JAM. Выводит из этого состояния только сброс.
    /// В сохранение не пишется: после загрузки pc указывает на сам KIL, он зависнет заново.
    halted: bool,
}

impl Cpu {
    pub fn new() -> Cpu {
        Cpu { a: 0, x: 0, y: 0, sp: 0xFD, pc: 0, p: FI | FU, cycles: 0, nmi: false, irq: false, halted: false }
    }

    pub fn save(&self, out: &mut Writer) {
        out.u8(self.a);
        out.u8(self.x);
        out.u8(self.y);
        out.u8(self.sp);
        out.u16(self.pc);
        out.u8(self.p);
        out.u64(self.cycles);
        out.bool(self.nmi);
        out.bool(self.irq);
    }

    pub fn load(&mut self, r: &mut Reader) -> Result<(), String> {
        self.a = r.u8()?;
        self.x = r.u8()?;
        self.y = r.u8()?;
        self.sp = r.u8()?;
        self.pc = r.u16()?;
        self.p = r.u8()?;
        self.cycles = r.u64()?;
        self.nmi = r.bool()?;
        self.irq = r.bool()?;
        self.halted = false;
        Ok(())
    }

    pub fn reset(&mut self, bus: &mut Bus) {
        self.a = 0;
        self.x = 0;
        self.y = 0;
        self.sp = 0xFD;
        self.p = FI | FU;
        self.pc = bus.read16(0xFFFC);
        self.cycles = 7;
        self.nmi = false;
        self.irq = false;
        self.halted = false;
    }

    /// Выполняет одну инструкцию (или вход в NMI), возвращает число тактов CPU.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        if self.halted {
            // KIL: ни NMI, ни IRQ процессор уже не слышит, живёт только PPU/APU
            self.nmi = false;
            self.cycles += 2;
            return 2;
        }
        if self.nmi {
            self.nmi = false;
            self.interrupt(bus, 0xFFFA, false);
            self.cycles += 7;
            return 7;
        }
        if self.irq && self.p & FI == 0 {
            self.interrupt(bus, 0xFFFE, false);
            self.cycles += 7;
            return 7;
        }
        let opcode = self.fetch(bus);
        let (op, mode, base) = decode(opcode);
        let mut cycles = base as u32;
        let (addr, crossed) = self.operand(bus, mode);
        if crossed
            && matches!(
                op,
                Op::LDA | Op::LDX | Op::LDY | Op::ADC | Op::SBC | Op::AND | Op::ORA | Op::EOR | Op::CMP | Op::NOP | Op::LAX | Op::LAS
            )
        {
            cycles += 1;
        }
        cycles += self.execute(bus, op, mode, addr, crossed);
        self.cycles += cycles as u64;
        cycles
    }

    // ---------- вспомогательное ----------

    fn fetch(&mut self, bus: &mut Bus) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        (hi << 8) | lo
    }

    fn push(&mut self, bus: &mut Bus, v: u8) {
        bus.write(0x0100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pull(&mut self, bus: &mut Bus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x0100 | self.sp as u16)
    }

    fn push16(&mut self, bus: &mut Bus, v: u16) {
        self.push(bus, (v >> 8) as u8);
        self.push(bus, v as u8);
    }

    fn pull16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.pull(bus) as u16;
        let hi = self.pull(bus) as u16;
        (hi << 8) | lo
    }

    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.p |= flag;
        } else {
            self.p &= !flag;
        }
    }

    fn zn(&mut self, v: u8) {
        self.set(FZ, v == 0);
        self.set(FN, v & 0x80 != 0);
    }

    fn adc(&mut self, v: u8) {
        let carry = (self.p & FC) as u16;
        let sum = self.a as u16 + v as u16 + carry;
        let r = sum as u8;
        self.set(FC, sum > 0xFF);
        self.set(FV, (!(self.a ^ v) & (self.a ^ r) & 0x80) != 0);
        self.a = r;
        self.zn(r);
    }

    fn cmp(&mut self, reg: u8, v: u8) {
        self.set(FC, reg >= v);
        self.zn(reg.wrapping_sub(v));
    }

    fn interrupt(&mut self, bus: &mut Bus, vector: u16, brk: bool) {
        self.push16(bus, self.pc);
        let flags = if brk { self.p | FB | FU } else { (self.p | FU) & !FB };
        self.push(bus, flags);
        self.p |= FI;
        self.pc = bus.read16(vector);
    }

    /// Возвращает доп. такты: +1 за взятый переход, +1 за пересечение страницы.
    fn branch(&mut self, bus: &mut Bus, addr: u16, cond: bool) -> u32 {
        let off = bus.read(addr) as i8;
        if !cond {
            return 0;
        }
        let new = self.pc.wrapping_add(off as i16 as u16);
        let extra = if (new & 0xFF00) != (self.pc & 0xFF00) { 2 } else { 1 };
        self.pc = new;
        extra
    }

    /// Вычисляет эффективный адрес. Второй элемент — было ли пересечение страницы.
    fn operand(&mut self, bus: &mut Bus, mode: Mode) -> (u16, bool) {
        match mode {
            Mode::Imp | Mode::Acc => (0, false),
            Mode::Imm | Mode::Rel => {
                let a = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (a, false)
            }
            Mode::Zp => (self.fetch(bus) as u16, false),
            Mode::Zpx => (self.fetch(bus).wrapping_add(self.x) as u16, false),
            Mode::Zpy => (self.fetch(bus).wrapping_add(self.y) as u16, false),
            Mode::Abs => (self.fetch16(bus), false),
            Mode::Abx => {
                let base = self.fetch16(bus);
                let a = base.wrapping_add(self.x as u16);
                (a, (a ^ base) & 0xFF00 != 0)
            }
            Mode::Aby => {
                let base = self.fetch16(bus);
                let a = base.wrapping_add(self.y as u16);
                (a, (a ^ base) & 0xFF00 != 0)
            }
            Mode::Ind => {
                // JMP ($xxFF) читает старший байт с начала той же страницы (баг 6502)
                let ptr = self.fetch16(bus);
                let lo = bus.read(ptr) as u16;
                let hi = bus.read((ptr & 0xFF00) | (ptr.wrapping_add(1) & 0x00FF)) as u16;
                ((hi << 8) | lo, false)
            }
            Mode::Izx => {
                let z = self.fetch(bus).wrapping_add(self.x);
                let lo = bus.read(z as u16) as u16;
                let hi = bus.read(z.wrapping_add(1) as u16) as u16;
                ((hi << 8) | lo, false)
            }
            Mode::Izy => {
                let z = self.fetch(bus);
                let lo = bus.read(z as u16) as u16;
                let hi = bus.read(z.wrapping_add(1) as u16) as u16;
                let base = (hi << 8) | lo;
                let a = base.wrapping_add(self.y as u16);
                (a, (a ^ base) & 0xFF00 != 0)
            }
        }
    }

    fn execute(&mut self, bus: &mut Bus, op: Op, mode: Mode, addr: u16, crossed: bool) -> u32 {
        use Op::*;
        let mut extra = 0;
        match op {
            LDA => { self.a = bus.read(addr); self.zn(self.a); }
            LDX => { self.x = bus.read(addr); self.zn(self.x); }
            LDY => { self.y = bus.read(addr); self.zn(self.y); }
            STA => bus.write(addr, self.a),
            STX => bus.write(addr, self.x),
            STY => bus.write(addr, self.y),

            TAX => { self.x = self.a; self.zn(self.x); }
            TAY => { self.y = self.a; self.zn(self.y); }
            TXA => { self.a = self.x; self.zn(self.a); }
            TYA => { self.a = self.y; self.zn(self.a); }
            TSX => { self.x = self.sp; self.zn(self.x); }
            TXS => self.sp = self.x,

            PHA => self.push(bus, self.a),
            PHP => self.push(bus, self.p | FB | FU),
            PLA => { self.a = self.pull(bus); self.zn(self.a); }
            PLP => self.p = (self.pull(bus) & !FB) | FU,

            AND => { self.a &= bus.read(addr); self.zn(self.a); }
            ORA => { self.a |= bus.read(addr); self.zn(self.a); }
            EOR => { self.a ^= bus.read(addr); self.zn(self.a); }
            ADC => { let v = bus.read(addr); self.adc(v); }
            SBC => { let v = bus.read(addr); self.adc(!v); }
            CMP => { let v = bus.read(addr); self.cmp(self.a, v); }
            CPX => { let v = bus.read(addr); self.cmp(self.x, v); }
            CPY => { let v = bus.read(addr); self.cmp(self.y, v); }
            BIT => {
                let v = bus.read(addr);
                self.set(FZ, self.a & v == 0);
                self.set(FV, v & 0x40 != 0);
                self.set(FN, v & 0x80 != 0);
            }

            INC => { let v = bus.read(addr).wrapping_add(1); bus.write(addr, v); self.zn(v); }
            DEC => { let v = bus.read(addr).wrapping_sub(1); bus.write(addr, v); self.zn(v); }
            INX => { self.x = self.x.wrapping_add(1); self.zn(self.x); }
            INY => { self.y = self.y.wrapping_add(1); self.zn(self.y); }
            DEX => { self.x = self.x.wrapping_sub(1); self.zn(self.x); }
            DEY => { self.y = self.y.wrapping_sub(1); self.zn(self.y); }

            ASL | LSR | ROL | ROR => {
                let v = if mode == Mode::Acc { self.a } else { bus.read(addr) };
                let carry = self.p & FC;
                let r = match op {
                    ASL => { self.set(FC, v & 0x80 != 0); v << 1 }
                    LSR => { self.set(FC, v & 0x01 != 0); v >> 1 }
                    ROL => { self.set(FC, v & 0x80 != 0); (v << 1) | carry }
                    _ => { self.set(FC, v & 0x01 != 0); (v >> 1) | (carry << 7) }
                };
                self.zn(r);
                if mode == Mode::Acc {
                    self.a = r;
                } else {
                    bus.write(addr, r);
                }
            }

            JMP => self.pc = addr,
            JSR => { self.push16(bus, self.pc.wrapping_sub(1)); self.pc = addr; }
            RTS => { self.pc = self.pull16(bus).wrapping_add(1); }
            RTI => { self.p = (self.pull(bus) & !FB) | FU; self.pc = self.pull16(bus); }
            BRK => { self.pc = self.pc.wrapping_add(1); self.interrupt(bus, 0xFFFE, true); }

            BCC => extra = self.branch(bus, addr, self.p & FC == 0),
            BCS => extra = self.branch(bus, addr, self.p & FC != 0),
            BNE => extra = self.branch(bus, addr, self.p & FZ == 0),
            BEQ => extra = self.branch(bus, addr, self.p & FZ != 0),
            BPL => extra = self.branch(bus, addr, self.p & FN == 0),
            BMI => extra = self.branch(bus, addr, self.p & FN != 0),
            BVC => extra = self.branch(bus, addr, self.p & FV == 0),
            BVS => extra = self.branch(bus, addr, self.p & FV != 0),

            CLC => self.set(FC, false),
            SEC => self.set(FC, true),
            CLI => self.set(FI, false),
            SEI => self.set(FI, true),
            CLD => self.set(FD, false),
            SED => self.set(FD, true),
            CLV => self.set(FV, false),
            // ----- неофициальные опкоды -----
            LAX => { let v = bus.read(addr); self.a = v; self.x = v; self.zn(v); }
            SAX => bus.write(addr, self.a & self.x),
            DCP => { let v = bus.read(addr).wrapping_sub(1); bus.write(addr, v); self.cmp(self.a, v); }
            ISB => { let v = bus.read(addr).wrapping_add(1); bus.write(addr, v); self.adc(!v); }
            SLO => {
                let v = bus.read(addr);
                self.set(FC, v & 0x80 != 0);
                let r = v << 1;
                bus.write(addr, r);
                self.a |= r;
                self.zn(self.a);
            }
            RLA => {
                let v = bus.read(addr);
                let c = self.p & FC;
                self.set(FC, v & 0x80 != 0);
                let r = (v << 1) | c;
                bus.write(addr, r);
                self.a &= r;
                self.zn(self.a);
            }
            SRE => {
                let v = bus.read(addr);
                self.set(FC, v & 0x01 != 0);
                let r = v >> 1;
                bus.write(addr, r);
                self.a ^= r;
                self.zn(self.a);
            }
            RRA => {
                let v = bus.read(addr);
                let c = self.p & FC;
                self.set(FC, v & 0x01 != 0);
                let r = (v >> 1) | (c << 7);
                bus.write(addr, r);
                self.adc(r); // флаг C, оставшийся после сдвига, идёт в сложение
            }
            ANC => {
                self.a &= bus.read(addr);
                self.zn(self.a);
                self.set(FC, self.a & 0x80 != 0);
            }
            ALR => {
                self.a &= bus.read(addr);
                self.set(FC, self.a & 0x01 != 0);
                self.a >>= 1;
                self.zn(self.a);
            }
            ARR => {
                self.a &= bus.read(addr);
                self.a = (self.a >> 1) | ((self.p & FC) << 7);
                self.zn(self.a);
                self.set(FC, self.a & 0x40 != 0);
                self.set(FV, ((self.a >> 6) ^ (self.a >> 5)) & 1 != 0);
            }
            AXS => {
                let t = self.a & self.x;
                let v = bus.read(addr);
                self.set(FC, t >= v);
                self.x = t.wrapping_sub(v);
                self.zn(self.x);
            }
            // ----- нестабильные опкоды -----
            // LXA/ATX #imm: на железе A = X = (A | магическая константа) & imm; у NES константа $FF
            ATX => { let v = bus.read(addr); self.a = v; self.x = v; self.zn(v); }
            // XAA/ANE #imm: A = (A | магическая константа) & X & imm
            XAA => { let v = bus.read(addr); self.a = (self.a | 0xFF) & self.x & v; self.zn(self.a); }
            // LAS abs,Y: A = X = SP = память & SP
            LAS => {
                let v = bus.read(addr) & self.sp;
                self.a = v;
                self.x = v;
                self.sp = v;
                self.zn(v);
            }
            // AHX/SHA, TAS, SHY, SHX: пишут (регистр & (старший байт базового адреса + 1)).
            // Если индексация перешла страницу, этим же значением портится старший байт адреса.
            AHX | TAS | SHY | SHX => {
                let reg = match op {
                    SHY => self.y,
                    SHX => self.x,
                    TAS => { self.sp = self.a & self.x; self.sp }
                    _ => self.a & self.x,
                };
                let hi = ((addr >> 8) as u8).wrapping_sub(crossed as u8).wrapping_add(1);
                let v = reg & hi;
                let target = if crossed { ((v as u16) << 8) | (addr & 0x00FF) } else { addr };
                bus.write(target, v);
            }
            // KIL/JAM: процессор намертво встаёт на этом опкоде
            KIL => { self.halted = true; self.pc = self.pc.wrapping_sub(1); }
            NOP => {}
        }
        extra
    }
}

/// opcode -> (операция, режим адресации, базовые такты)
#[allow(unreachable_patterns)]
fn decode(op: u8) -> (Op, Mode, u8) {
    use Mode::*;
    use Op::*;
    match op {
        0x00 => (BRK, Imp, 7), 0x01 => (ORA, Izx, 6), 0x05 => (ORA, Zp, 3), 0x06 => (ASL, Zp, 5),
        0x08 => (PHP, Imp, 3), 0x09 => (ORA, Imm, 2), 0x0A => (ASL, Acc, 2), 0x0D => (ORA, Abs, 4), 0x0E => (ASL, Abs, 6),
        0x10 => (BPL, Rel, 2), 0x11 => (ORA, Izy, 5), 0x15 => (ORA, Zpx, 4), 0x16 => (ASL, Zpx, 6),
        0x18 => (CLC, Imp, 2), 0x19 => (ORA, Aby, 4), 0x1D => (ORA, Abx, 4), 0x1E => (ASL, Abx, 7),
        0x20 => (JSR, Abs, 6), 0x21 => (AND, Izx, 6), 0x24 => (BIT, Zp, 3), 0x25 => (AND, Zp, 3), 0x26 => (ROL, Zp, 5),
        0x28 => (PLP, Imp, 4), 0x29 => (AND, Imm, 2), 0x2A => (ROL, Acc, 2), 0x2C => (BIT, Abs, 4), 0x2D => (AND, Abs, 4), 0x2E => (ROL, Abs, 6),
        0x30 => (BMI, Rel, 2), 0x31 => (AND, Izy, 5), 0x35 => (AND, Zpx, 4), 0x36 => (ROL, Zpx, 6),
        0x38 => (SEC, Imp, 2), 0x39 => (AND, Aby, 4), 0x3D => (AND, Abx, 4), 0x3E => (ROL, Abx, 7),
        0x40 => (RTI, Imp, 6), 0x41 => (EOR, Izx, 6), 0x45 => (EOR, Zp, 3), 0x46 => (LSR, Zp, 5),
        0x48 => (PHA, Imp, 3), 0x49 => (EOR, Imm, 2), 0x4A => (LSR, Acc, 2), 0x4C => (JMP, Abs, 3), 0x4D => (EOR, Abs, 4), 0x4E => (LSR, Abs, 6),
        0x50 => (BVC, Rel, 2), 0x51 => (EOR, Izy, 5), 0x55 => (EOR, Zpx, 4), 0x56 => (LSR, Zpx, 6),
        0x58 => (CLI, Imp, 2), 0x59 => (EOR, Aby, 4), 0x5D => (EOR, Abx, 4), 0x5E => (LSR, Abx, 7),
        0x60 => (RTS, Imp, 6), 0x61 => (ADC, Izx, 6), 0x65 => (ADC, Zp, 3), 0x66 => (ROR, Zp, 5),
        0x68 => (PLA, Imp, 4), 0x69 => (ADC, Imm, 2), 0x6A => (ROR, Acc, 2), 0x6C => (JMP, Ind, 5), 0x6D => (ADC, Abs, 4), 0x6E => (ROR, Abs, 6),
        0x70 => (BVS, Rel, 2), 0x71 => (ADC, Izy, 5), 0x75 => (ADC, Zpx, 4), 0x76 => (ROR, Zpx, 6),
        0x78 => (SEI, Imp, 2), 0x79 => (ADC, Aby, 4), 0x7D => (ADC, Abx, 4), 0x7E => (ROR, Abx, 7),
        0x81 => (STA, Izx, 6), 0x84 => (STY, Zp, 3), 0x85 => (STA, Zp, 3), 0x86 => (STX, Zp, 3),
        0x88 => (DEY, Imp, 2), 0x8A => (TXA, Imp, 2), 0x8C => (STY, Abs, 4), 0x8D => (STA, Abs, 4), 0x8E => (STX, Abs, 4),
        0x90 => (BCC, Rel, 2), 0x91 => (STA, Izy, 6), 0x94 => (STY, Zpx, 4), 0x95 => (STA, Zpx, 4), 0x96 => (STX, Zpy, 4),
        0x98 => (TYA, Imp, 2), 0x99 => (STA, Aby, 5), 0x9A => (TXS, Imp, 2), 0x9D => (STA, Abx, 5),
        0xA0 => (LDY, Imm, 2), 0xA1 => (LDA, Izx, 6), 0xA2 => (LDX, Imm, 2), 0xA4 => (LDY, Zp, 3), 0xA5 => (LDA, Zp, 3), 0xA6 => (LDX, Zp, 3),
        0xA8 => (TAY, Imp, 2), 0xA9 => (LDA, Imm, 2), 0xAA => (TAX, Imp, 2), 0xAC => (LDY, Abs, 4), 0xAD => (LDA, Abs, 4), 0xAE => (LDX, Abs, 4),
        0xB0 => (BCS, Rel, 2), 0xB1 => (LDA, Izy, 5), 0xB4 => (LDY, Zpx, 4), 0xB5 => (LDA, Zpx, 4), 0xB6 => (LDX, Zpy, 4),
        0xB8 => (CLV, Imp, 2), 0xB9 => (LDA, Aby, 4), 0xBA => (TSX, Imp, 2), 0xBC => (LDY, Abx, 4), 0xBD => (LDA, Abx, 4), 0xBE => (LDX, Aby, 4),
        0xC0 => (CPY, Imm, 2), 0xC1 => (CMP, Izx, 6), 0xC4 => (CPY, Zp, 3), 0xC5 => (CMP, Zp, 3), 0xC6 => (DEC, Zp, 5),
        0xC8 => (INY, Imp, 2), 0xC9 => (CMP, Imm, 2), 0xCA => (DEX, Imp, 2), 0xCC => (CPY, Abs, 4), 0xCD => (CMP, Abs, 4), 0xCE => (DEC, Abs, 6),
        0xD0 => (BNE, Rel, 2), 0xD1 => (CMP, Izy, 5), 0xD5 => (CMP, Zpx, 4), 0xD6 => (DEC, Zpx, 6),
        0xD8 => (CLD, Imp, 2), 0xD9 => (CMP, Aby, 4), 0xDD => (CMP, Abx, 4), 0xDE => (DEC, Abx, 7),
        0xE0 => (CPX, Imm, 2), 0xE1 => (SBC, Izx, 6), 0xE4 => (CPX, Zp, 3), 0xE5 => (SBC, Zp, 3), 0xE6 => (INC, Zp, 5),
        0xE8 => (INX, Imp, 2), 0xE9 | 0xEB => (SBC, Imm, 2), 0xEA => (NOP, Imp, 2), 0xEC => (CPX, Abs, 4), 0xED => (SBC, Abs, 4), 0xEE => (INC, Abs, 6),
        0xF0 => (BEQ, Rel, 2), 0xF1 => (SBC, Izy, 5), 0xF5 => (SBC, Zpx, 4), 0xF6 => (INC, Zpx, 6),
        0xF8 => (SED, Imp, 2), 0xF9 => (SBC, Aby, 4), 0xFD => (SBC, Abx, 4), 0xFE => (INC, Abx, 7),
        // неофициальные опкоды
        0xA7 => (LAX, Zp, 3), 0xB7 => (LAX, Zpy, 4), 0xAF => (LAX, Abs, 4), 0xBF => (LAX, Aby, 4),
        0xA3 => (LAX, Izx, 6), 0xB3 => (LAX, Izy, 5),
        0x87 => (SAX, Zp, 3), 0x97 => (SAX, Zpy, 4), 0x8F => (SAX, Abs, 4), 0x83 => (SAX, Izx, 6),
        0xC7 => (DCP, Zp, 5), 0xD7 => (DCP, Zpx, 6), 0xCF => (DCP, Abs, 6), 0xDF => (DCP, Abx, 7),
        0xDB => (DCP, Aby, 7), 0xC3 => (DCP, Izx, 8), 0xD3 => (DCP, Izy, 8),
        0xE7 => (ISB, Zp, 5), 0xF7 => (ISB, Zpx, 6), 0xEF => (ISB, Abs, 6), 0xFF => (ISB, Abx, 7),
        0xFB => (ISB, Aby, 7), 0xE3 => (ISB, Izx, 8), 0xF3 => (ISB, Izy, 8),
        0x07 => (SLO, Zp, 5), 0x17 => (SLO, Zpx, 6), 0x0F => (SLO, Abs, 6), 0x1F => (SLO, Abx, 7),
        0x1B => (SLO, Aby, 7), 0x03 => (SLO, Izx, 8), 0x13 => (SLO, Izy, 8),
        0x27 => (RLA, Zp, 5), 0x37 => (RLA, Zpx, 6), 0x2F => (RLA, Abs, 6), 0x3F => (RLA, Abx, 7),
        0x3B => (RLA, Aby, 7), 0x23 => (RLA, Izx, 8), 0x33 => (RLA, Izy, 8),
        0x47 => (SRE, Zp, 5), 0x57 => (SRE, Zpx, 6), 0x4F => (SRE, Abs, 6), 0x5F => (SRE, Abx, 7),
        0x5B => (SRE, Aby, 7), 0x43 => (SRE, Izx, 8), 0x53 => (SRE, Izy, 8),
        0x67 => (RRA, Zp, 5), 0x77 => (RRA, Zpx, 6), 0x6F => (RRA, Abs, 6), 0x7F => (RRA, Abx, 7),
        0x7B => (RRA, Aby, 7), 0x63 => (RRA, Izx, 8), 0x73 => (RRA, Izy, 8),
        0x0B | 0x2B => (ANC, Imm, 2), 0x4B => (ALR, Imm, 2), 0x6B => (ARR, Imm, 2), 0xCB => (AXS, Imm, 2),
        // нестабильные опкоды
        0x8B => (XAA, Imm, 2), 0xAB => (ATX, Imm, 2),
        0x93 => (AHX, Izy, 6), 0x9F => (AHX, Aby, 5), 0x9B => (TAS, Aby, 5),
        0x9C => (SHY, Abx, 5), 0x9E => (SHX, Aby, 5), 0xBB => (LAS, Aby, 4),
        // KIL/JAM: 12 опкодов, после них процессор не отвечает
        0x02 | 0x12 | 0x22 | 0x32 | 0x42 | 0x52 | 0x62 | 0x72 | 0x92 | 0xB2 | 0xD2 | 0xF2 => (KIL, Imp, 2),
        // неофициальные NOP (встречаются в некоторых играх)
        0x1A | 0x3A | 0x5A | 0x7A | 0xDA | 0xFA => (NOP, Imp, 2),
        0x80 | 0x82 | 0x89 | 0xC2 | 0xE2 => (NOP, Imm, 2),
        0x04 | 0x44 | 0x64 => (NOP, Zp, 3),
        0x14 | 0x34 | 0x54 | 0x74 | 0xD4 | 0xF4 => (NOP, Zpx, 4),
        0x0C => (NOP, Abs, 4),
        0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => (NOP, Abx, 4),
        // все 256 опкодов описаны выше; эта ветка нужна только компилятору
        _ => (NOP, Imp, 2),
    }
}
