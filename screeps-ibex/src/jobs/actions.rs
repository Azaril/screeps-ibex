use bitflags::*;

// Screeps simultaneous action pipelines (from docs.screeps.com/simultaneous-actions.html):
//
// Pipeline A (melee/work): harvest, attack, build, repair, dismantle, attackController, generateSafeMode
// Pipeline B (ranged):      rangedAttack, rangedMassAttack, rangedHeal
// Pipeline C (heal):        heal
// Pipeline D (transfer):    withdraw, transfer, drop, pickup
// Pipeline E:               upgradeController
// Pipeline F:               claimController, reserveController, signController
//
// Actions within the same pipeline are mutually exclusive (share the same bit).
// Actions in different pipelines can coexist (different bits).
//
// Key combat combinations now correctly allowed:
//   rangedAttack + heal       (Pipeline B + C)
//   attack + heal             (Pipeline A + C)
//   rangedAttack + attack     (Pipeline A + B)

bitflags! {
    #[derive(Copy, Clone)]
    pub struct SimultaneousActionFlags: u16 {
        const UNSET = 0;

        const MOVE = 1;

        // Pipeline A: melee/work actions (mutually exclusive within pipeline)
        const HARVEST            = 1 << 1;
        const ATTACK             = 1 << 1;
        const BUILD              = 1 << 1;
        const REPAIR             = 1 << 1;
        const DISMANTLE          = 1 << 1;
        const ATTACK_CONTROLLER  = 1 << 1;
        const GENERATE_SAFE_MODE = 1 << 1;

        // Pipeline B: ranged actions (mutually exclusive within pipeline)
        const RANGED_ATTACK      = 1 << 2;
        const RANGED_MASS_ATTACK = 1 << 2;
        const RANGED_HEAL        = 1 << 2;

        // Pipeline C: heal (own pipeline, independent of A and B)
        const HEAL = 1 << 3;

        // Pipeline D: transfer/logistics actions (mutually exclusive within pipeline)
        const WITHDRAW = 1 << 4;
        const TRANSFER = 1 << 4;
        const DROP     = 1 << 4;
        const PICKUP   = 1 << 4;

        // Pipeline E: upgrade (own pipeline)
        const UPGRADE_CONTROLLER = 1 << 5;

        // Pipeline F: claim/reserve/sign (mutually exclusive within pipeline)
        const CLAIM_CONTROLLER   = 1 << 6;
        const RESERVE_CONTROLLER = 1 << 6;
        const SIGN               = 1 << 6;
    }
}

impl SimultaneousActionFlags {
    pub fn consume(&mut self, flags: SimultaneousActionFlags) -> bool {
        if !self.intersects(flags) {
            self.insert(flags);

            true
        } else {
            false
        }
    }
}
