mod album_grid;
mod artwork;
mod collection;
mod commands;
mod flac;
mod gallery;
mod history;
mod library;
mod lyrics;
mod meter;
mod model;
mod motion;
mod mpris;
mod player;
mod playlist;
mod presentation;
mod scene;
mod storage;
mod ui;
mod validation;
mod wake;
use adw::prelude::*;
use gtk::{gio, glib};
pub const APP_ID: &str = "io.github.gap19.FLACker";
fn main() -> glib::ExitCode {
    // The interface is English only; keep GTK, libadwaita and GStreamer messages in the
    // same language instead of mixing them with the desktop locale.
    // SAFETY: set before any thread or library that reads the environment is started.
    unsafe { std::env::set_var("LANGUAGE", "en") };
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("FLACker {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--help") {
        println!(
            "FLACker — offline FLAC player for GNOME\n\nUsage: flacker [file.flac | playlist.m3u8 | folder]\n       flacker --scan FOLDER\n       flacker --validate FOLDER\n       flacker --audio-test FILE...\n       flacker --play-pause | --next | --previous\n\nFLACKER_DATA_DIR isolates preferences and the library index."
        );
        return glib::ExitCode::SUCCESS;
    }
    if let Some(result) = validation::cli(&args) {
        return if let Err(e) = result {
            eprintln!("{e}");
            glib::ExitCode::FAILURE
        } else {
            glib::ExitCode::SUCCESS
        };
    }
    if let Some(action) = args.get(1).and_then(|a| match a.as_str() {
        "--play-pause" => Some("play-pause"),
        "--next" => Some("next"),
        "--previous" => Some("previous"),
        _ => None,
    }) {
        let remote = adw::Application::builder().application_id(APP_ID).build();
        if let Err(e) = remote.register(gio::Cancellable::NONE) {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
        if remote.is_remote() {
            // Only send the action: running the remote instance would also activate it and
            // raise the window, stealing focus from a media-key binding.
            remote.activate_action(action, None);
            if let Some(c) = remote.dbus_connection() {
                let _ = c.flush_sync(gio::Cancellable::NONE);
            }
            // The action is delivered; leave without tearing down the registered instance.
            std::process::exit(0);
        }
        remote.connect_activate(move |app| match ui::Ui::build(app, Vec::new()) {
            Ok(_) => app.activate_action(action, None),
            Err(e) => {
                eprintln!("{e}");
                app.quit();
            }
        });
        return remote.run_with_args(&[args[0].as_str()]);
    }
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        if let Err(e) = ui::Ui::build(app, Vec::new()) {
            eprintln!("Could not start FLACker: {e}");
            app.quit();
        }
    });
    app.connect_open(|app, files, _| {
        let paths = files.iter().filter_map(|f| f.path()).collect::<Vec<_>>();
        if app.active_window().is_some() {
            // Existing process routes files through its registered action.
            if let Some(action) = app.lookup_action("open-paths") {
                action.activate(Some(
                    &paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .to_variant(),
                ));
            }
        } else if let Err(e) = ui::Ui::build(app, paths) {
            eprintln!("{e}");
            app.quit();
        }
    });
    app.run()
}
