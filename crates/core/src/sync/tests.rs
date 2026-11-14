//! Sync tests against anki's own sync server, run in this process.
//!
//! Each test starts a `SimpleServer` (the code of `anki-sync-server`) on a
//! free port and drives `sync_now` and `full_sync` as the UI does. The layout
//! follows anki's `rslib/src/sync/collection/tests.rs`.
//!
//! `dev_server_roundtrip` runs against the Docker dev sync server instead. See
//! `notes/sync.md`, "Tests".

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Once, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anki::decks::DeckId;
use anki::sync::http_server::{SimpleServer, SyncServerConfig, default_ip_header};

use super::{SyncAction, SyncResult, full_sync, normalize_endpoint, sync_now};
use crate::config::Config;
use crate::session::{LearnSession, init_session, media_folder};

const USER: &str = "pbanki";
const PASSWORD: &str = "devpass";

fn unique_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pbanki-sync-test-{name}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Starts a sync server with one user (`pbanki:devpass`) and its own data
/// folder. Returns its endpoint. The server runs until the test process ends.
fn start_server() -> String {
    static USERS: Once = Once::new();
    USERS.call_once(|| {
        // SAFETY: set once, before the first server reads it. No other code in
        // this crate reads or writes `SYNC_USER1`.
        unsafe { std::env::set_var("SYNC_USER1", format!("{USER}:{PASSWORD}")) };
    });

    let base_folder = unique_dir("server");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let (addr, server) = SimpleServer::make_server(SyncServerConfig {
                host: "127.0.0.1".parse().unwrap(),
                port: 0,
                base_folder,
                ip_header: default_ip_header(),
            })
            .await
            .unwrap();
            sender.send(addr).unwrap();
            let _ = server.await;
        });
    });
    let addr = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("sync server did not start");
    format!("http://{addr}/")
}

/// Starts a server that answers every request with `308` to `location`, as
/// AnkiWeb does when it moves an account to another shard.
fn start_redirector(location: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read the whole request, or the client can see a reset instead
            // of the response.
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            let body_start = loop {
                let Ok(n) = stream.read(&mut buf) else {
                    break None;
                };
                if n == 0 {
                    break None;
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(body_start) = body_start else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..body_start]).to_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            while request.len() < body_start + length {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 308 Permanent Redirect\r\nlocation: {location}\r\n\
                 content-length: 0\r\nconnection: close\r\n\r\n"
            );
        }
    });
    format!("http://{addr}/")
}

/// One pbAnki install: a config file, a collection, and a session.
struct Device {
    dir: PathBuf,
    config: Config,
    session: Rc<LearnSession>,
}

impl Device {
    fn new(name: &str, endpoint: &str, user: &str, password: &str) -> Self {
        let dir = unique_dir(name);
        let mut config = Config {
            // Never the default path: `Config::save` would write to the device path.
            path: dir.join("config.toml"),
            ..Config::default()
        };
        config.general.collection_path = dir.join("col").to_string_lossy().into_owned();
        config.general.card_fallback_fonts.clear();
        config.general.font_dirs.clear();
        config.ankiweb.username = user.into();
        config.ankiweb.password = password.into();
        config.ankiweb.endpoint = Some(endpoint.into());
        let session = init_session(&config);
        Self {
            dir,
            config,
            session,
        }
    }

    fn local(name: &str, endpoint: &str) -> Self {
        Self::new(name, endpoint, USER, PASSWORD)
    }

    fn sync(&mut self) -> SyncResult {
        let outcome = sync_now(
            &mut self.session.collection.borrow_mut(),
            &self.config,
            &self.session.sync_manager,
        );
        outcome.apply_config(&mut self.config);
        outcome.result
    }

    fn full_sync(&mut self, upload: bool) -> SyncResult {
        let outcome = full_sync(
            &mut self.session.collection.borrow_mut(),
            &self.config,
            &self.session.sync_manager,
            upload,
        );
        outcome.apply_config(&mut self.config);
        outcome.result
    }

    fn add_note(&self, text: &str) {
        let mut borrow = self.session.collection.borrow_mut();
        let col = borrow.as_mut().unwrap();
        let notetype = col.get_notetype_by_name("Basic").unwrap().unwrap();
        let mut note = notetype.new_note();
        note.set_field(0, text).unwrap();
        col.add_note(&mut note, DeckId(1)).unwrap();
    }

    fn note_count(&self) -> u32 {
        let borrow = self.session.collection.borrow();
        let col = borrow.as_ref().unwrap();
        col.storage
            .db()
            .query_row("select count() from notes", [], |row| row.get(0))
            .unwrap()
    }

    fn media_dir(&self) -> PathBuf {
        media_folder(&self.config)
    }

    fn saved_config(&self) -> String {
        fs::read_to_string(&self.config.path).unwrap()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        drop(self.session.collection.borrow_mut().take());
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[track_caller]
fn assert_ok(result: &SyncResult) {
    assert!(result.success, "sync failed: {}", result.message);
    assert_eq!(result.media_error, None);
}

/// Anki's `upload_download`: device A uploads a note and device B downloads it.
fn upload_download(endpoint: &str) -> (Device, Device) {
    let mut a = Device::local("a", endpoint);
    a.add_note("1");
    let result = a.sync();
    assert!(result.success, "{}", result.message);
    assert!(
        matches!(
            result.action,
            SyncAction::FullSyncRequired {
                upload_ok: true,
                ..
            }
        ),
        "{:?}",
        result.action
    );
    assert_ok(&a.full_sync(true));

    let mut b = Device::local("b", endpoint);
    let result = b.sync();
    assert!(result.success, "{}", result.message);
    // An empty collection must not overwrite the server.
    assert_eq!(
        result.action,
        SyncAction::FullSyncRequired {
            upload_ok: false,
            download_ok: true,
        }
    );
    assert_ok(&b.full_sync(false));
    (a, b)
}

#[test]
fn first_sync_logs_in_and_saves_token() {
    let endpoint = start_server();
    let mut device = Device::local("login", &endpoint);

    let result = device.sync();

    assert_ok(&result);
    assert_eq!(result.action, SyncAction::NoChanges);
    let token = device.config.ankiweb.token.clone().expect("token saved");
    assert!(!token.is_empty());
    assert_eq!(
        device.config.ankiweb.token_endpoint,
        normalize_endpoint(&endpoint).unwrap()
    );
    assert!(device.saved_config().contains(&token));
}

#[test]
fn wrong_password_is_reported() {
    let endpoint = start_server();
    let mut device = Device::new("badpass", &endpoint, USER, "wrong");

    let result = device.sync();

    assert!(!result.success);
    assert!(
        result.message.starts_with("Login failed"),
        "{}",
        result.message
    );
    assert_eq!(device.config.ankiweb.token, None);
}

#[test]
fn token_for_other_endpoint_is_replaced() {
    let endpoint = start_server();
    let mut device = Device::local("stale", &endpoint);
    device.config.ankiweb.token = Some("stale".into());
    device.config.ankiweb.token_endpoint = Some("http://other.invalid/".into());

    let result = device.sync();

    assert_ok(&result);
    assert_ne!(device.config.ankiweb.token.as_deref(), Some("stale"));
    assert_eq!(
        device.config.ankiweb.token_endpoint,
        normalize_endpoint(&endpoint).unwrap()
    );
}

#[test]
fn upload_download_roundtrip() {
    let endpoint = start_server();
    let (_a, b) = upload_download(&endpoint);

    assert_eq!(b.note_count(), 1);
    let backups: Vec<_> = fs::read_dir(b.dir.join("col"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("collection.anki2.backup-")
        })
        .collect();
    assert!(!backups.is_empty(), "no backup before the full download");
}

#[test]
fn normal_sync_merges_both_ways() {
    let endpoint = start_server();
    let (mut a, mut b) = upload_download(&endpoint);

    a.add_note("2");
    let result = a.sync();
    assert_ok(&result);
    assert!(!matches!(
        result.action,
        SyncAction::FullSyncRequired { .. }
    ));

    let result = b.sync();
    assert_ok(&result);
    assert!(!matches!(
        result.action,
        SyncAction::FullSyncRequired { .. }
    ));
    assert_eq!(b.note_count(), 2);

    b.add_note("3");
    assert_ok(&b.sync());
    assert_ok(&a.sync());
    assert_eq!(a.note_count(), 3);
}

#[test]
fn media_roundtrip() {
    let endpoint = start_server();
    let (mut a, mut b) = upload_download(&endpoint);

    // Media change detection compares millisecond mtimes with the last scan.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(a.media_dir().join("a.txt"), "media").unwrap();
    assert_ok(&a.sync());
    assert_ok(&b.sync());

    assert_eq!(
        fs::read_to_string(b.media_dir().join("a.txt")).unwrap(),
        "media"
    );
}

#[test]
fn endpoint_redirect_is_saved() {
    let endpoint = start_server();
    let (mut a, _b) = upload_download(&endpoint);
    let redirector = start_redirector(endpoint.clone());
    a.config.ankiweb.endpoint = Some(redirector.clone());
    // The saved token belongs to this account, so keep it for the old URL.
    a.config.ankiweb.token_endpoint = normalize_endpoint(&redirector).unwrap();
    a.add_note("2");

    let result = a.sync();

    assert_ok(&result);
    assert_eq!(
        a.config.ankiweb.endpoint.as_deref(),
        Some(endpoint.as_str())
    );
    assert_eq!(
        a.config.ankiweb.token_endpoint,
        normalize_endpoint(&endpoint).unwrap()
    );
    assert!(a.saved_config().contains(&endpoint));
}

/// The upload, download, normal sync, and media flow against an external
/// server, by default the Docker dev sync server. It checks the client from
/// anki `main` against the pinned server image.
///
/// This **replaces the account's collection** on that server.
///
/// ```sh
/// make -C ../pbanki-sync-server up
/// PBANKI_SYNC_TEST_ENDPOINT=http://127.0.0.1:27701/ \
///     cargo test -p pbanki-core dev_server_roundtrip -- --ignored
/// ```
///
/// `PBANKI_SYNC_TEST_USER` (`user:password`) defaults to `pbanki:devpass`.
#[test]
#[ignore = "needs a sync server; set PBANKI_SYNC_TEST_ENDPOINT"]
fn dev_server_roundtrip() {
    let endpoint = std::env::var("PBANKI_SYNC_TEST_ENDPOINT")
        .expect("set PBANKI_SYNC_TEST_ENDPOINT, for example http://127.0.0.1:27701/");
    let user = std::env::var("PBANKI_SYNC_TEST_USER").unwrap_or(format!("{USER}:{PASSWORD}"));
    let (user, password) = user
        .split_once(':')
        .expect("PBANKI_SYNC_TEST_USER must be user:password");

    let mut a = Device::new("dev-a", &endpoint, user, password);
    a.add_note("1");
    let result = a.sync();
    assert!(result.success, "{}", result.message);
    assert!(
        matches!(
            result.action,
            SyncAction::FullSyncRequired {
                upload_ok: true,
                ..
            }
        ),
        "{:?}",
        result.action
    );
    assert_ok(&a.full_sync(true));

    let mut b = Device::new("dev-b", &endpoint, user, password);
    let result = b.sync();
    assert!(result.success, "{}", result.message);
    assert_eq!(
        result.action,
        SyncAction::FullSyncRequired {
            upload_ok: false,
            download_ok: true,
        }
    );
    assert_ok(&b.full_sync(false));
    assert_eq!(b.note_count(), 1);

    a.add_note("2");
    std::thread::sleep(Duration::from_millis(20));
    let media = format!("pbanki-test-{}.txt", std::process::id());
    fs::write(a.media_dir().join(&media), "media").unwrap();
    assert_ok(&a.sync());
    assert_ok(&b.sync());
    assert_eq!(b.note_count(), 2);
    assert!(b.media_dir().join(&media).exists());
}
