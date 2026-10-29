use common::*;
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    let config_path = "./pbanki/config.toml";
    let mut config = Config::load_or_create(config_path).unwrap_or_else(|e| {
        eprintln!("Config error: {:?}, using defaults", e);
        let mut cfg = Config::default();
        cfg.path = config_path.into();
        cfg
    });
    if config.general.collection_path == "/mnt/ext1/applications/pbanki/collection" {
        config.general.collection_path = "./pbanki/collection".into();
    }
    let config = Rc::new(RefCell::new(config));

    let setup_timer = ProfileTimer::start();
    let session = init_session(&config.borrow());
    let session_elapsed = setup_timer.elapsed();

    let fonts_timer = ProfileTimer::start();
    register_catalog_fonts(&session.font_catalog);
    let fonts_elapsed = fonts_timer.elapsed();
    spawn_font_benchmark(&session.font_catalog, "Monospace");

    let window_timer = ProfileTimer::start();
    let ui = MainWindow::new().unwrap();
    let window_elapsed = window_timer.elapsed();

    let init_ui_timer = ProfileTimer::start();
    init_ui(&ui, &session, &config.borrow(), false);
    profile_log!(
        "startup: init_session={session_elapsed:?} \
         register_fonts={fonts_elapsed:?} window={window_elapsed:?} \
         init_ui={:?} total={:?}",
        init_ui_timer.elapsed(),
        setup_timer.elapsed()
    );
    let hooks = FrontendHooks::default();
    wire_callbacks(&ui, session.clone(), config.clone(), hooks.clone());
    maybe_auto_sync(&ui, session.clone(), config.clone(), hooks);

    let _ = ui.run();
}
