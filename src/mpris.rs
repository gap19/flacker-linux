use crate::{
    commands::Command,
    model::{Repeat, Track},
    ui::Ui,
};
use gtk::{
    gio::{self, prelude::*},
    glib::{self, variant::ToVariant},
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
pub struct Service {
    connection: gio::DBusConnection,
    _owner: gio::OwnerId,
    _registrations: Vec<gio::RegistrationId>,
    last: RefCell<String>,
}
fn track_id(t: &Track) -> String {
    format!(
        "/io/github/gap19/flacker/track/t{:x}",
        Sha256::digest(t.path.to_string_lossy().as_bytes())
    )
}
fn metadata(ui: &Ui) -> glib::Variant {
    let mut map = HashMap::<String, glib::Variant>::new();
    if let Some(t) = ui.player.borrow().current() {
        map.insert(
            "mpris:trackid".into(),
            glib::variant::ObjectPath::try_from(track_id(&t))
                .unwrap()
                .to_variant(),
        );
        map.insert(
            "mpris:length".into(),
            ((t.duration * 1e6) as i64).to_variant(),
        );
        map.insert("xesam:title".into(), t.title.to_variant());
        map.insert("xesam:artist".into(), vec![t.artist.clone()].to_variant());
        map.insert("xesam:album".into(), t.album.to_variant());
        map.insert(
            "xesam:albumArtist".into(),
            vec![t.album_artist.clone()].to_variant(),
        );
        map.insert("xesam:trackNumber".into(), (t.number as i32).to_variant());
        map.insert("xesam:discNumber".into(), (t.disc as i32).to_variant());
        map.insert(
            "xesam:url".into(),
            gio::File::for_path(&t.path).uri().to_variant(),
        );
        if let Some(p) = ui.art.borrow().exported() {
            map.insert(
                "mpris:artUrl".into(),
                gio::File::for_path(p).uri().to_variant(),
            );
        }
    }
    map.to_variant()
}
fn property(ui: &Ui, name: &str) -> glib::Variant {
    if name == "Metadata" {
        return metadata(ui);
    }
    let player = ui.player.borrow();
    let (current, index, count, repeat, shuffle) = player.summary();
    match name {
        "Identity" => "FLACker".to_variant(),
        "DesktopEntry" => crate::APP_ID.to_variant(),
        "HasTrackList" => false.to_variant(),
        "CanQuit" | "CanRaise" | "CanControl" => true.to_variant(),
        "SupportedUriSchemes" => vec!["file"].to_variant(),
        "SupportedMimeTypes" => vec![
            "audio/flac",
            "audio/x-flac",
            "audio/x-mpegurl",
            "application/vnd.apple.mpegurl",
        ]
        .to_variant(),
        "PlaybackStatus" => (if player.playing {
            "Playing"
        } else if player.loaded {
            "Paused"
        } else {
            "Stopped"
        })
        .to_variant(),
        "LoopStatus" => (match repeat {
            Repeat::Off => "None",
            Repeat::All => "Playlist",
            Repeat::One => "Track",
        })
        .to_variant(),
        "Shuffle" => shuffle.to_variant(),
        "Rate" | "MinimumRate" | "MaximumRate" => 1f64.to_variant(),
        "Volume" => player.volume.to_variant(),
        "Position" => ((player.position() * 1e6) as i64).to_variant(),
        "CanGoNext" => (index.is_some_and(|i| i + 1 < count) || repeat == Repeat::All).to_variant(),
        "CanGoPrevious" => (index.is_some_and(|i| i > 0) || player.position() > 3.).to_variant(),
        "CanPlay" | "CanPause" | "CanSeek" => current.is_some().to_variant(),
        _ => false.to_variant(),
    }
}
impl Service {
    pub fn new(ui: &Rc<Ui>) -> Result<Self, String> {
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|e| e.to_string())?;
        let info = gio::DBusNodeInfo::for_xml(include_str!("../assets/mpris.xml"))
            .map_err(|e| e.to_string())?;
        let mut registrations = Vec::new();
        for name in ["org.mpris.MediaPlayer2", PLAYER] {
            let interface = info
                .lookup_interface(name)
                .ok_or("MPRIS interface missing")?;
            let weak = Rc::downgrade(ui);
            let prop = Rc::downgrade(ui);
            let set = Rc::downgrade(ui);
            let id = connection
                .register_object(PATH, &interface)
                .method_call(move |_, _, _, _, method, args, invocation| {
                    let Some(ui) = weak.upgrade() else {
                        invocation.return_dbus_error(
                            "org.mpris.MediaPlayer2.Error.Failed",
                            "Application closed",
                        );
                        return;
                    };
                    let c = match method {
                        "Raise" => Some(Command::Raise),
                        "Quit" => Some(Command::Quit),
                        "Next" => Some(Command::Next),
                        "Previous" => Some(Command::Previous),
                        "PlayPause" => Some(Command::Toggle),
                        "Pause" => Some(Command::Pause),
                        "Play" => Some(Command::Resume),
                        "Stop" => Some(Command::Stop),
                        "Seek" => args
                            .get::<(i64,)>()
                            .map(|(v,)| Command::SeekRelative(v as f64 / 1e6)),
                        "SetPosition" => {
                            args.get::<(glib::variant::ObjectPath, i64)>()
                                .and_then(|(id, pos)| {
                                    ui.player
                                        .borrow()
                                        .current()
                                        .filter(|t| {
                                            id.as_str() == track_id(t)
                                                && pos >= 0
                                                && (pos as f64 / 1e6) <= t.duration
                                        })
                                        .map(|_| Command::Seek(pos as f64 / 1e6))
                                })
                        }
                        "OpenUri" => {
                            let path = args
                                .get::<(String,)>()
                                .filter(|(uri,)| uri.starts_with("file:"))
                                .and_then(|(uri,)| gio::File::for_uri(&uri).path());
                            match path {
                                Some(p) => Some(Command::OpenPaths(vec![p])),
                                None => {
                                    invocation.return_dbus_error(
                                        "org.mpris.MediaPlayer2.Error.InvalidUri",
                                        "Only local files are accepted",
                                    );
                                    return;
                                }
                            }
                        }
                        _ => {
                            invocation.return_dbus_error(
                                "org.freedesktop.DBus.Error.UnknownMethod",
                                "Unknown method",
                            );
                            return;
                        }
                    };
                    if let Some(c) = c {
                        let _ = ui.sender.send(c);
                    }
                    invocation.return_value(None);
                })
                .property(move |_, _, _, _, name| {
                    prop.upgrade()
                        .map_or_else(|| false.to_variant(), |ui| property(&ui, name))
                })
                .set_property(move |_, _, _, _, name, v| {
                    let Some(ui) = set.upgrade() else {
                        return false;
                    };
                    let c = match name {
                        "Volume" => v
                            .get::<f64>()
                            .filter(|v| v.is_finite())
                            .map(Command::Volume),
                        "Shuffle" => v.get::<bool>().map(Command::Shuffle),
                        "LoopStatus" => v
                            .get::<String>()
                            .and_then(|v| match v.as_str() {
                                "None" => Some(Repeat::Off),
                                "Track" => Some(Repeat::One),
                                "Playlist" => Some(Repeat::All),
                                _ => None,
                            })
                            .map(Command::Repeat),
                        "Rate" => return v.get::<f64>() == Some(1.),
                        _ => None,
                    };
                    if let Some(c) = c {
                        ui.sender.send(c).is_ok()
                    } else {
                        false
                    }
                })
                .build()
                .map_err(|e| e.to_string())?;
            registrations.push(id);
        }
        let owner = gio::bus_own_name_on_connection(
            &connection,
            "org.mpris.MediaPlayer2.flacker",
            gio::BusNameOwnerFlags::NONE,
            |_, _| {},
            |_, _| {},
        );
        Ok(Self {
            connection,
            _owner: owner,
            _registrations: registrations,
            last: RefCell::new(String::new()),
        })
    }
    pub fn update(&self, ui: &Ui) {
        let p = ui.player.borrow();
        let (current, index, count, repeat, shuffle) = p.summary();
        // Index and queue length change CanGoNext/CanGoPrevious after queue edits.
        let key = format!(
            "{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{}",
            current.map(|t| t.path),
            index,
            count,
            p.playing,
            p.loaded,
            repeat,
            shuffle,
            p.volume,
            ui.art.borrow().revision
        );
        drop(p);
        if key == *self.last.borrow() {
            return;
        }
        self.last.replace(key);
        let mut properties = HashMap::new();
        for name in [
            "PlaybackStatus",
            "Metadata",
            "LoopStatus",
            "Shuffle",
            "Volume",
            "CanGoNext",
            "CanGoPrevious",
            "CanPlay",
            "CanPause",
            "CanSeek",
        ] {
            properties.insert(name, property(ui, name));
        }
        let value = (PLAYER, properties, Vec::<String>::new()).to_variant();
        let _ = self.connection.emit_signal(
            None,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&value),
        );
    }
    pub fn seeked(&self, seconds: f64) {
        let _ = self.connection.emit_signal(
            None,
            PATH,
            PLAYER,
            "Seeked",
            Some(&(((seconds * 1e6) as i64),).to_variant()),
        );
    }
}
