pub mod bodies;
pub mod boostqueue;
pub mod composition;
pub mod damage;
pub mod economy;
pub mod formation;
pub mod squad;
pub mod threatmap;

/// Screeps NPC owner usernames. Use these constants instead of hardcoding
/// string literals in functional code.
pub const NPC_INVADER: &str = "Invader";
pub const NPC_SOURCE_KEEPER: &str = "Source Keeper";

/// Returns true if the given username belongs to an NPC (Invader or Source Keeper).
pub fn is_npc_owner(username: &str) -> bool {
    username == NPC_INVADER || username == NPC_SOURCE_KEEPER
}

/// Returns true if the given username belongs to an Invader NPC specifically
/// (not a Source Keeper). Source Keepers are permanent room residents and
/// should not be treated as hostile invaders.
pub fn is_invader_owner(username: &str) -> bool {
    username == NPC_INVADER
}

/// Returns true if the given username belongs to a Source Keeper NPC.
pub fn is_source_keeper_owner(username: &str) -> bool {
    username == NPC_SOURCE_KEEPER
}
