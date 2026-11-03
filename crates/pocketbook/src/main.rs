use std::rc::Rc;

use inkview::bindings::Inkview;
use inkview::device::DeviceInfo;
use inkview::screen::{PanelMode, WhichScreen};
use inkview_slint::{Backend, BackendOptions, Event, UpdatePolicy};
use pbanki_core::card_render::PageFormat;
use pbanki_core::config::Config;
use pbanki_core::frontend::{FrontendHooks, SyncHooks};
use pbanki_core::{EditField, MainWindow, app};
use slint::ComponentHandle;

mod keyboard;
mod network;

use keyboard::KeyboardKind;

const DEFAULT_CONFIG_PATH: &str = "/mnt/ext1/applications/pbanki/config.toml";

fn main() {
    pbanki_core::logging::init();
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("[panic] {info}");
    }));

    let iv = Box::leak(Box::new(inkview::load())) as &'static _;

    let (evt_tx, evt_rx) = std::sync::mpsc::channel();
    let backend_tx = evt_tx.clone();

    std::thread::Builder::new()
        .name("pbanki-ui".into())
        .stack_size(8 * 1024 * 1024)
        .spawn({
            move || {
                if !matches!(
                    evt_rx.recv().unwrap(),
                    Event::InkviewEvent(inkview::Event::Init)
                ) {
                    panic!("expected init event first");
                }
                // hwconfig is loaded by InkView's init.
                let device = log_device(iv);

                // Read before the screen is opened: the panel setting decides
                // how the framebuffer is set up.
                let config_path =
                    std::env::var("PBANKI_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.into());
                let config = Config::load_or_default(&config_path);

                let panel = if config.general.system_panel {
                    PanelMode::System
                } else {
                    PanelMode::Hidden
                };
                let options = BackendOptions {
                    policy: UpdatePolicy::for_panel(device.is_color().unwrap_or(false)),
                };

                // I hope this thing lives as long as the process
                // Card pages use the framebuffer's format: gray pages for a
                // gray panel, color pages for a color one.
                let (platform, page_format): (Box<dyn slint::platform::Platform>, _) =
                    match WhichScreen::with_panel(iv, panel) {
                        WhichScreen::BB8(screen) => (
                            Box::new(Backend::with_options(screen, backend_tx, evt_rx, options)),
                            PageFormat::Gray8,
                        ),
                        WhichScreen::RGB24(screen) => (
                            Box::new(Backend::with_options(screen, backend_tx, evt_rx, options)),
                            PageFormat::Rgb8,
                        ),
                    };
                tracing::info!("[card] page format {page_format:?}");
                slint::platform::set_platform(platform).unwrap();

                let hooks = FrontendHooks {
                    sync: SyncHooks {
                        before_sync: Some(Rc::new(move || network::ensure_connected(iv))),
                        after_sync: Some(Rc::new(move || network::disconnect_after_sync(iv))),
                    },
                    full_present: Some(Rc::new(inkview_slint::request_full_present)),
                    page_format,
                };
                let app = app::start(config, true, hooks);
                wire_native_keyboard(&app.ui, iv);
                app.run().unwrap();
            }
        })
        .expect("failed to spawn UI thread");

    inkview::iv_main(iv, {
        move |evt| {
            if evt_tx.send(Event::InkviewEvent(evt)).is_err() {
                unsafe {
                    iv.CloseApp();
                }
            }

            Some(())
        }
    })
}

/// Logs what the firmware reports about the device, and warns when the
/// hwconfig DPI disagrees with the DPI the backend scales by.
fn log_device(iv: &Inkview) -> DeviceInfo {
    let device = DeviceInfo::query(iv);
    tracing::info!("[device] {device}");
    let screen_dpi = iv.get_screen_dpi.as_ref().ok().map(|f| unsafe { (*f)() });
    if let (Some(display), Some(screen_dpi)) = (device.display, screen_dpi)
        && display.dpi != 0
        && i64::from(display.dpi) != i64::from(screen_dpi)
    {
        // Bound first: tracing's macros shadow the name `display`.
        let hw_dpi = display.dpi;
        tracing::warn!("[device] warning: hwconfig dpi={hw_dpi} but get_screen_dpi()={screen_dpi}");
    }
    device
}

/// Edits the settings text fields with the InkView keyboard instead of
/// Slint's `LineEdit`.
fn wire_native_keyboard(ui: &MainWindow, iv: &'static Inkview) {
    let ui_weak = ui.as_weak();
    ui.on_edit_field(move |field, current| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let tr = ui.get_tr();
        let (title, kind) = match field {
            EditField::Endpoint => (tr.server_url, KeyboardKind::Url),
            EditField::Username => (tr.email, KeyboardKind::Text),
            EditField::Password => (tr.password, KeyboardKind::Password),
        };
        let ui_weak = ui_weak.clone();
        keyboard::open(
            iv,
            &title,
            current.as_str(),
            kind,
            Box::new(move |text| {
                // Cancelled: keep the current value.
                let Some(text) = text else {
                    return;
                };
                // The keyboard closes on the InkView thread.
                let _ = slint::invoke_from_event_loop(move || {
                    set_settings_field(&ui_weak, field, text);
                });
            }),
        );
    });
}

fn set_settings_field(ui: &slint::Weak<MainWindow>, field: EditField, text: String) {
    let Some(ui) = ui.upgrade() else {
        return;
    };
    let mut settings = ui.get_sync_settings();
    match field {
        EditField::Endpoint => settings.endpoint = text.into(),
        EditField::Username => settings.username = text.into(),
        EditField::Password => settings.password = text.into(),
    }
    ui.set_sync_settings(settings);
}
