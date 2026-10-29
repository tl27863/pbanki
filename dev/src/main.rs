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

    let session = init_session(&config.borrow());
    register_catalog_fonts(&session.font_catalog);

    let ui = MainWindow::new().unwrap();
    init_ui(&ui, &session, &config.borrow(), false);
    let sync_hooks = SyncHooks::default();
    wire_callbacks(&ui, session.clone(), config.clone(), sync_hooks.clone());
    maybe_auto_sync(&ui, session.clone(), config.clone(), sync_hooks);

    let _ = ui.run();
}
