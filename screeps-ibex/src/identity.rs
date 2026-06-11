//! The bot's own identity (statics-review M6 — replaces the
//! `globals::USERNAME` thread_local). A specs Resource, refreshed each
//! tick by the game loop from controller ownership; `Default` (empty
//! username) classifies nothing as Mine, exactly like the old
//! pre-`set_name` static — and host tests construct whatever identity
//! they need per instance (the disposition logic is decision-bearing:
//! friendly/hostile classification flows from this value).
//!
//! Known lifetime edge (accepted): the old thread_local survived an
//! environment rebuild; the Resource dies with the world. In the
//! zero-owned-rooms window after a rebuild (i.e. while respawning) the
//! bot's own old signs/reservations read as Hostile until a room is
//! owned again — harmless, and the value re-derives on the first owned
//! tick.

#[derive(Debug, Clone, Default)]
pub struct BotIdentity {
    pub username: String,
}
