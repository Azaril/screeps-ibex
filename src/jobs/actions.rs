bitflags! {
    pub struct SimultaneousActionFlags: u8 {
        const UNSET = 0;

        const MOVE = 1u8;
        const TRANSFER = 1u8 << 1;
    }
}
