use bitflags::*;

//TODO: This needs a better API. It also needs to correctly represent pipelines.

bitflags! {
    pub struct SimultaneousActionFlags: u8 {
        const UNSET = 0;

        const MOVE = 1u8;

        const HARVEST = 1u8 << 1;
        const ATTACK = 1u8 << 1;
        const BUILD = 1u8 << 1;
        const REPAIR = 1u8 << 1;
        const DISMANTLE = 1u8 << 1;
        const ATTACK_CONTROLLER = 1u8 << 1;
        const RANGED_HEAL = 1u8 << 1;
        const HEAL = 1u8 << 1;

        const RANGED_ATTACK = 1u8 << 1;
        const RANGED_MASS_ATTACK = 1u8 << 1;

        const WITHDRAW = 1u8 << 2;
        const TRANSFER = 1u8 << 2;
        const DROP = 1u8 << 2;

        //TODO: Handle overlapping priorities.
    }
}
