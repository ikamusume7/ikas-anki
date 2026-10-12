// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// DeckConfig inside deck_config.proto
#![allow(clippy::module_inception)]

mod generic_helpers;

macro_rules! protobuf {
    ($ident:ident, $name:literal) => {
        pub mod $ident {
            include!(concat!(env!("OUT_DIR"), "/anki.", $name, ".rs"));
        }
    };
}

protobuf!(ankidroid, "ankidroid");
protobuf!(ankiweb, "ankiweb");
protobuf!(backend, "backend");
protobuf!(card_rendering, "card_rendering");
protobuf!(cards, "cards");
protobuf!(collection, "collection");
protobuf!(config, "config");
protobuf!(deck_config, "deck_config");
protobuf!(decks, "decks");
// frontend.proto 的 service 在 rust_interface.rs 中被过滤（由 GUI 层实现），
// 但它的 message/enum 需要被外部外壳（如 tauri/）引用，因此在此暴露。
protobuf!(frontend, "frontend");
protobuf!(generic, "generic");
protobuf!(i18n, "i18n");
protobuf!(image_occlusion, "image_occlusion");
protobuf!(import_export, "import_export");
protobuf!(links, "links");
protobuf!(media, "media");
protobuf!(notes, "notes");
protobuf!(notetypes, "notetypes");
protobuf!(scheduler, "scheduler");
protobuf!(search, "search");
protobuf!(stats, "stats");
protobuf!(sync, "sync");
protobuf!(tags, "tags");
protobuf!(ankihub, "ankihub");
protobuf!(github, "github");
