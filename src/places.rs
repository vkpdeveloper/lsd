//! Sidebar places: Favorites and Locations, like Finder's sidebar.
//!
//! PLACEHOLDER: minimal version. The real implementation replaces this file and
//! keeps the public API unchanged.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Applications,
    Pictures,
    Music,
    Movies,
    Root,
    Volume,
    Trash,
}

#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub kind: PlaceKind,
}

/// Existing standard folders, in Finder's order.
pub fn favorites() -> Vec<Place> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    vec![Place {
        name: "Home".into(),
        path: home,
        kind: PlaceKind::Home,
    }]
}

/// The root disk plus mounted volumes.
pub fn locations() -> Vec<Place> {
    vec![Place {
        name: "/".into(),
        path: PathBuf::from("/"),
        kind: PlaceKind::Root,
    }]
}

/// Free bytes on the volume holding `path`.
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}
