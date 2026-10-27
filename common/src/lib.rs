slint::include_modules!();

pub mod api;
pub mod config;
pub mod frontend;
pub mod sync;
pub mod utils;

pub use api::{
    LearnSession, full_sync, get_sync_status, init_session, init_translations, next_card,
    rate_card, save_sync_settings, sync_now, sync_settings_from_config, sync_state_from_result,
    sync_state_from_status, update_deck_tree,
};
pub use config::Config;
pub use frontend::{init_ui, maybe_auto_sync, wire_callbacks};
pub use sync::{SyncAction, SyncManager, SyncResult, SyncStatus};
