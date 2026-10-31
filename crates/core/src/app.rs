//! Startup shared by both frontends.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::MainWindow;
use crate::config::Config;
use crate::fonts::register_catalog_fonts;
use crate::frontend::{FrontendHooks, init_ui, maybe_auto_sync, wire_callbacks};
use crate::profile::{ProfileTimer, spawn_font_probe};
use crate::profile_log;
use crate::session::{LearnSession, init_session};

/// UI font family from `ui/constants.slint`, reported by the font probe.
const UI_FONT_FAMILY: &str = "Monospace";

/// The window with the shared callbacks wired. Platform-specific callbacks can
/// be added to `ui` before `run`.
pub struct App {
    pub ui: MainWindow,
    pub session: Rc<LearnSession>,
    pub config: Rc<RefCell<Config>>,
    hooks: FrontendHooks,
}

/// Opens the collection, registers the card fonts, and creates the window.
///
/// Platforms with a custom Slint platform must call `set_platform` first:
/// font registration uses the platform's shared font collection.
pub fn start(config: Config, native_keyboard: bool, hooks: FrontendHooks) -> App {
    let config = Rc::new(RefCell::new(config));

    let setup_timer = ProfileTimer::start();
    let session = init_session(&config.borrow());
    let session_elapsed = setup_timer.elapsed();

    let fonts_timer = ProfileTimer::start();
    register_catalog_fonts(&session.font_catalog);
    let fonts_elapsed = fonts_timer.elapsed();
    spawn_font_probe(&session.font_catalog, UI_FONT_FAMILY);

    let window_timer = ProfileTimer::start();
    let ui = MainWindow::new().unwrap();
    let window_elapsed = window_timer.elapsed();

    let init_ui_timer = ProfileTimer::start();
    init_ui(&ui, &session, &config.borrow(), native_keyboard);
    profile_log!(
        "startup: init_session={session_elapsed:?} \
         register_fonts={fonts_elapsed:?} window={window_elapsed:?} \
         init_ui={:?} total={:?}",
        init_ui_timer.elapsed(),
        setup_timer.elapsed()
    );

    wire_callbacks(&ui, session.clone(), config.clone(), hooks.clone());

    App {
        ui,
        session,
        config,
        hooks,
    }
}

impl App {
    /// Starts the startup sync if it is enabled, then runs the event loop.
    pub fn run(self) -> Result<(), slint::PlatformError> {
        maybe_auto_sync(
            &self.ui,
            self.session.clone(),
            self.config.clone(),
            self.hooks.clone(),
        );
        self.ui.run()
    }
}
