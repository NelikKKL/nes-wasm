/// Джойстик NES. Биты: A, B, Select, Start, Up, Down, Left, Right (0..7).
#[derive(Default)]
pub struct Controller {
    pub state: u8,
    shift: u8,
    strobe: bool,
}

impl Controller {
    pub fn write(&mut self, val: u8) {
        self.strobe = val & 1 != 0;
        if self.strobe {
            self.shift = self.state;
        }
    }

    pub fn read(&mut self) -> u8 {
        if self.strobe {
            self.shift = self.state;
        }
        let bit = self.shift & 1;
        self.shift = (self.shift >> 1) | 0x80;
        bit
    }
}
