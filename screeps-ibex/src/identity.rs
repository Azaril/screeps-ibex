//! The bot's own identity (statics-review M6 — replaces the
//! `globals::USERNAME` thread_local). A specs Resource, refreshed each
//! tick by the game loop from controller ownership; `Default` (empty
//! username) classifies nothing as Mine, exactly like the old
//! pre-`set_name` static — and host tests construct whatever identity
//! they need per instance (the disposition logic is decision-bearing:
//! friendly/hostile classification flows from this value).

#[derive(Debug, Clone, Default)]
pub struct BotIdentity {
    pub username: String,
}
