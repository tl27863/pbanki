use common::*;
use inkview::Event;

use std::cell::RefCell;
use std::os::raw::c_int;
use std::rc::Rc;
use std::time::Duration;

mod keyboard;
mod network;

fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[panic] {info}");
    }));

    let iv = Box::leak(Box::new(inkview::load())) as &'static _;

    let (evt_tx, evt_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("pbanki-ui".into())
        .stack_size(8 * 1024 * 1024)
        .spawn({
            move || {
                if evt_rx.recv().unwrap() != Event::Init {
                    panic!("expected init event first");
                }

                // I hope this thing lives as long as the process
                let screen = inkview::screen::Screen::new(iv);

                let display = inkview_slint::Backend::new(screen, evt_rx);

                slint::platform::set_platform(Box::new(display)).unwrap();

                let config_path = std::env::var("PBANKI_CONFIG")
                    .unwrap_or_else(|_| "/mnt/ext1/applications/pbanki/config.toml".to_string());
                let config = Rc::new(RefCell::new(
                    Config::load_or_create(&config_path).unwrap_or_else(|e| {
                        eprintln!("Config error: {:?}, using defaults", e);
                        let mut cfg = Config::default();
                        cfg.path = config_path.into();
                        cfg
                    }),
                ));

                let setup_timer = ProfileTimer::start();
                let session = init_session(&config.borrow());
                let session_elapsed = setup_timer.elapsed();

                let fonts_timer = ProfileTimer::start();
                register_catalog_fonts(&session.font_catalog);
                let fonts_elapsed = fonts_timer.elapsed();
                spawn_font_benchmark(&session.font_catalog, "Monospace");

                let window_timer = ProfileTimer::start();
                let ui = Rc::new(MainWindow::new().unwrap());
                let window_elapsed = window_timer.elapsed();

                let init_ui_timer = ProfileTimer::start();
                init_ui(&ui, &session, &config.borrow(), true);
                profile_log!(
                    "startup: init_session={session_elapsed:?} \
                     register_fonts={fonts_elapsed:?} window={window_elapsed:?} \
                     init_ui={:?} total={:?}",
                    init_ui_timer.elapsed(),
                    setup_timer.elapsed()
                );
                let hooks = FrontendHooks {
                    sync: SyncHooks {
                        before_sync: Some(Rc::new(move || network::ensure_connected(iv))),
                        after_sync: Some(Rc::new(move || network::disconnect_after_sync(iv))),
                    },
                    full_present: Some(Rc::new(inkview_slint::request_full_present)),
                };
                wire_callbacks(&ui, session.clone(), config.clone(), hooks.clone());

                let (keyboard_tx, keyboard_rx) = std::sync::mpsc::channel::<(i32, String)>();
                let keyboard_timer = slint::Timer::default();
                {
                    let ui_weak = ui.as_weak();
                    keyboard_timer.start(
                        slint::TimerMode::Repeated,
                        Duration::from_millis(100),
                        move || {
                            while let Ok((field, text)) = keyboard_rx.try_recv() {
                                if let Some(ui) = ui_weak.upgrade() {
                                    let mut settings = ui.get_sync_settings();
                                    match field {
                                        keyboard::FIELD_ENDPOINT => settings.endpoint = text.into(),
                                        keyboard::FIELD_USERNAME => settings.username = text.into(),
                                        keyboard::FIELD_PASSWORD => settings.password = text.into(),
                                        _ => {}
                                    }
                                    ui.set_sync_settings(settings);
                                }
                            }
                        },
                    );
                }

                {
                    let ui_weak = ui.as_weak();
                    ui.on_edit_field(move |field, current| {
                        let title = if let Some(ui) = ui_weak.upgrade() {
                            let tr = ui.get_tr();
                            match field {
                                keyboard::FIELD_ENDPOINT => tr.server_url.to_string(),
                                keyboard::FIELD_USERNAME => tr.email.to_string(),
                                keyboard::FIELD_PASSWORD => tr.password.to_string(),
                                _ => "Enter text".into(),
                            }
                        } else {
                            "Enter text".into()
                        };
                        let flags = match field {
                            keyboard::FIELD_ENDPOINT => inkview::bindings::KBD_URL as c_int,
                            keyboard::FIELD_PASSWORD => inkview::bindings::KBD_PASSWORD as c_int,
                            _ => inkview::bindings::KBD_NORMAL as c_int,
                        };
                        keyboard::open(
                            iv,
                            field,
                            &title,
                            current.as_str(),
                            flags,
                            keyboard_tx.clone(),
                        );
                    });
                }

                maybe_auto_sync(&ui, session.clone(), config.clone(), hooks);

                ui.run().unwrap();
            }
        })
        .expect("failed to spawn UI thread");

    inkview::iv_main(iv, {
        move |evt| {
            if evt_tx.send(evt).is_err() {
                unsafe {
                    iv.CloseApp();
                }
            }

            Some(())
        }
    })
}
