use pbanki_core::app;
use pbanki_core::config::Config;
use pbanki_core::frontend::FrontendHooks;

#[cfg(feature = "eink-sim")]
mod eink_sim;

const CONFIG_PATH: &str = "./pbanki/config.toml";
const DEVICE_COLLECTION_PATH: &str = "/mnt/ext1/applications/pbanki/collection";
const DESKTOP_COLLECTION_PATH: &str = "./pbanki/collection";

fn load_config() -> Config {
    let mut config = Config::load_or_default(CONFIG_PATH);
    // The generated config uses the device path; on desktop, keep the
    // collection next to the config instead.
    if config.general.collection_path == DEVICE_COLLECTION_PATH {
        config.general.collection_path = DESKTOP_COLLECTION_PATH.into();
    }
    config
}

#[cfg(not(feature = "eink-sim"))]
fn main() {
    pbanki_core::logging::init();
    let app = app::start(load_config(), false, FrontendHooks::default());
    let _ = app.run();
}

/// The window and input run on the main thread; Slint runs on the inkview-slint
/// backend on a `pbanki-ui` thread, as on the PocketBook.
#[cfg(feature = "eink-sim")]
fn main() {
    use std::rc::Rc;

    pbanki_core::logging::init();

    use inkview_slint::{Backend, Event};

    let (events_tx, events_rx) = std::sync::mpsc::channel::<Event>();
    let backend_tx = events_tx.clone();
    let panel = eink_sim::Panel::new();
    let screen_panel = panel.clone();

    let ui = std::thread::Builder::new()
        .name("pbanki-ui".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let screen = eink_sim::SimScreen::new(screen_panel);
            let backend = Backend::new(screen, backend_tx, events_rx);
            slint::platform::set_platform(Box::new(backend)).unwrap();
            let hooks = FrontendHooks {
                full_present: Some(Rc::new(inkview_slint::request_full_present)),
                ..FrontendHooks::default()
            };
            let app = app::start(load_config(), false, hooks);
            let _ = app.run();
        })
        .expect("failed to spawn UI thread");

    eink_sim::run_window(panel, events_tx);
    let _ = ui.join();
}
