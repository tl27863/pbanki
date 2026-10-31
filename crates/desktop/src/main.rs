use pbanki_core::app;
use pbanki_core::config::Config;
use pbanki_core::frontend::FrontendHooks;

const CONFIG_PATH: &str = "./pbanki/config.toml";
const DEVICE_COLLECTION_PATH: &str = "/mnt/ext1/applications/pbanki/collection";
const DESKTOP_COLLECTION_PATH: &str = "./pbanki/collection";

fn main() {
    let mut config = Config::load_or_default(CONFIG_PATH);
    // The generated config uses the device path; on desktop, keep the
    // collection next to the config instead.
    if config.general.collection_path == DEVICE_COLLECTION_PATH {
        config.general.collection_path = DESKTOP_COLLECTION_PATH.into();
    }

    let app = app::start(config, false, FrontendHooks::default());
    let _ = app.run();
}
