//! Slint callback wiring shared by both frontends.
//!
//! Platform differences come in through `FrontendHooks`. Card loads run on a
//! worker thread (`card_jobs`); sync still blocks the UI thread (`sync`).

mod card_jobs;
mod card_pages;
mod models;
mod settings;
mod study;
mod sync;

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::card_render::{CardRenderer, ImageCacheSize, PageFormat, RendererConfig};
use crate::config::Config;
use crate::fonts::{card_font_families, card_font_files};
use crate::session::{LearnSession, media_folder};
use crate::{MainWindow, SyncState, Theme};
use models::{deck_tree, translations};
use settings::sync_settings_from_config;

pub use sync::{SyncHooks, maybe_auto_sync};

/// Platform hooks passed to `wire_callbacks` and `maybe_auto_sync`.
#[derive(Clone, Default)]
pub struct FrontendHooks {
    pub sync: SyncHooks,
    /// Requests a non-flashing, full-frame present of the next rendered frame.
    ///
    /// Called on the UI thread for genuine screen transitions only (deck entry
    /// and its first result, Home, Settings open/back). Desktop leaves this
    /// unset, making transition requests a plain redraw.
    pub full_present: Option<Rc<dyn Fn()>>,
    /// Pixel format of the card pages: `Gray8` for a gray panel (`BB8`
    /// framebuffer), `Rgb8` (the default) for a color panel and the desktop.
    pub page_format: PageFormat,
}

impl FrontendHooks {
    fn request_full_present(&self, ui: &MainWindow) {
        ui.window().request_redraw();
        if let Some(hook) = &self.full_present {
            hook();
        }
    }
}

pub fn init_ui(ui: &MainWindow, session: &LearnSession, config: &Config, native_keyboard: bool) {
    ui.set_native_keyboard(native_keyboard);
    ui.set_busy(false);
    ui.set_tr(translations(session));
    ui.set_sync_settings(sync_settings_from_config(config));
    ui.set_sync_state(SyncState::default());
    ui.set_deck_tree(deck_tree(session));
}

pub fn wire_callbacks(
    ui: &MainWindow,
    session: Rc<LearnSession>,
    config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
) {
    // UI colors follow the panel kind, as the card pages do (`ui/theme.slint`).
    ui.global::<Theme>()
        .set_color_panel(hooks.page_format == PageFormat::Rgb8);
    let renderer = spawn_card_renderer(ui, &session, &config.borrow(), hooks.page_format);
    card_pages::set_renderer(renderer.clone());
    ui.on_page_image(|answer, page, _version| card_pages::page_image(answer, page));

    settings::wire(ui, &session, &config, &hooks);
    sync::wire(ui, &session, &config, &hooks);
    study::wire(ui, &session, &config, &hooks, &renderer);
}

/// Starts the card-render thread. Pages rendered after the first ones arrive
/// on the UI thread through the event loop.
fn spawn_card_renderer(
    ui: &MainWindow,
    session: &LearnSession,
    config: &Config,
    format: PageFormat,
) -> CardRenderer {
    let ui_weak = ui.as_weak();
    CardRenderer::spawn(
        RendererConfig {
            format,
            font_files: card_font_files(&session.font_catalog),
            font_families: card_font_families(&session.font_catalog),
            media_folder: media_folder(config),
            image_cache: ImageCacheSize::from_mb(
                config.general.image_cache_mb,
                config.general.image_cache_max_image_mb,
            ),
        },
        Box::new(move |page| {
            let _ = ui_weak.upgrade_in_event_loop(move |ui| card_pages::deliver(&ui, page));
        }),
    )
}
