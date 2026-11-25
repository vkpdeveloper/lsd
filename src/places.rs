//! Sidebar places: Favorites and Locations, like Finder's sidebar. Everything
//! here is a few syscalls and small file reads, with no processes spawned.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
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
    /// Movies on macOS, Videos on Linux.
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

/// Existing standard folders, in Finder's order. Home is named "Home"; the
/// others use their folder's name, which on Linux may be localized.
pub fn favorites() -> Vec<Place> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    standard_folders(&home)
        .into_iter()
        // A Linux user dir set to the home folder itself means "not used".
        .filter(|(kind, path)| *kind == PlaceKind::Home || *path != home)
        .filter(|(_, path)| path.is_dir())
        .map(|(kind, path)| Place {
            name: if kind == PlaceKind::Home {
                "Home".into()
            } else {
                file_name(&path)
            },
            path,
            kind,
        })
        .collect()
}

/// The root disk, mounted volumes, then the Trash.
pub fn locations() -> Vec<Place> {
    let mut places = vec![Place {
        name: root_name(),
        path: PathBuf::from("/"),
        kind: PlaceKind::Root,
    }];
    places.extend(volumes());
    if let Some(path) = trash_dir().filter(|path| path.is_dir()) {
        places.push(Place {
            name: "Trash".into(),
            path,
            kind: PlaceKind::Trash,
        });
    }
    places
}

/// Free bytes on the volume holding `path`, as available to this user.
pub fn free_space(path: &Path) -> Option<u64> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // statvfs on macOS has 32-bit block counts, so ask statfs, which has 64-bit ones.
    #[cfg(target_os = "macos")]
    {
        let mut st = libc::statfs::default();
        // SAFETY: path is NUL-terminated and st is a valid statfs to write into.
        (unsafe { libc::statfs(path.as_ptr(), &mut st) } == 0)
            .then(|| st.f_bavail * u64::from(st.f_bsize))
    }
    #[cfg(not(target_os = "macos"))]
    #[allow(clippy::useless_conversion)] // The fields are only 32-bit on 32-bit targets.
    {
        let mut st = libc::statvfs::default();
        // SAFETY: path is NUL-terminated and st is a valid statvfs to write into.
        (unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0)
            .then(|| u64::from(st.f_bavail) * u64::from(st.f_frsize))
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(target_os = "macos")]
fn standard_folders(home: &Path) -> Vec<(PlaceKind, PathBuf)> {
    use PlaceKind::*;
    vec![
        (Applications, PathBuf::from("/Applications")),
        (Desktop, home.join("Desktop")),
        (Documents, home.join("Documents")),
        (Downloads, home.join("Downloads")),
        (Home, home.to_path_buf()),
        (Pictures, home.join("Pictures")),
        (Music, home.join("Music")),
        (Movies, home.join("Movies")),
    ]
}

#[cfg(not(target_os = "macos"))]
fn standard_folders(home: &Path) -> Vec<(PlaceKind, PathBuf)> {
    use PlaceKind::*;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let text = fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    let dir = |key, default| user_dir(&text, key, home).unwrap_or_else(|| home.join(default));
    vec![
        (Desktop, dir("XDG_DESKTOP_DIR", "Desktop")),
        (Documents, dir("XDG_DOCUMENTS_DIR", "Documents")),
        (Downloads, dir("XDG_DOWNLOAD_DIR", "Downloads")),
        (Home, home.to_path_buf()),
        (Pictures, dir("XDG_PICTURES_DIR", "Pictures")),
        (Music, dir("XDG_MUSIC_DIR", "Music")),
        (Movies, dir("XDG_VIDEOS_DIR", "Videos")),
    ]
}

/// Looks up `key` in a user-dirs.dirs file. Values are quoted, and are either
/// absolute or start with $HOME, which is the only expansion the format has.
#[cfg(any(not(target_os = "macos"), test))]
fn user_dir(text: &str, key: &str, home: &Path) -> Option<PathBuf> {
    text.lines().rev().find_map(|line| {
        let (k, value) = line.trim().split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let value = value.trim().trim_matches('"');
        match value.strip_prefix("$HOME") {
            Some(rest) => Some(home.join(rest.trim_start_matches('/'))),
            None if value.starts_with('/') => Some(PathBuf::from(value)),
            None => None,
        }
    })
}

/// The boot volume's name ("Macintosh HD"). macOS lists it in /Volumes as a
/// symlink to "/".
#[cfg(target_os = "macos")]
fn root_name() -> String {
    fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        // The type comes from readdir itself. Calling readlink on the other
        // entries would look into those volumes, which can take milliseconds.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_symlink()))
        .find(|entry| fs::read_link(entry.path()).is_ok_and(|target| target == Path::new("/")))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

#[cfg(not(target_os = "macos"))]
fn root_name() -> String {
    "File System".into()
}

/// Mounted volumes Finder would show: everything not flagged "don't browse",
/// which hides system volumes, the data volume, and helper mounts.
#[cfg(target_os = "macos")]
fn volumes() -> Vec<Place> {
    use std::ffi::CStr;
    use std::os::raw::c_int;

    // SAFETY: a null buffer asks only for the number of mounts.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    // Room for a few mounts that appear in between the two calls.
    let mut mounts = vec![libc::statfs::default(); count + 4];
    let size = mounts.len() * std::mem::size_of::<libc::statfs>();
    // SAFETY: the buffer holds `size` bytes of statfs records.
    let count = unsafe { libc::getfsstat(mounts.as_mut_ptr(), size as c_int, libc::MNT_NOWAIT) };
    mounts.truncate(usize::try_from(count).unwrap_or(0));

    mounts
        .iter()
        .filter(|fs| fs.f_flags & libc::MNT_DONTBROWSE as u32 == 0)
        .filter_map(|fs| {
            // SAFETY: the kernel fills f_mntonname with a NUL-terminated path.
            let mount = unsafe { CStr::from_ptr(fs.f_mntonname.as_ptr()) };
            let path = PathBuf::from(std::ffi::OsStr::from_bytes(mount.to_bytes()));
            (path != Path::new("/")).then(|| Place {
                name: file_name(&path),
                path,
                kind: PlaceKind::Volume,
            })
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn volumes() -> Vec<Place> {
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    user_mounts(&text, home_dir().as_deref())
}

/// Picks the mounts a person would think of as disks from /proc/self/mounts:
/// removable media and anything under /mnt or the home folder, plus other
/// real block devices unless they hold system folders (/boot, /home, /var).
/// Pseudo file systems (proc, tmpfs, cgroup, overlay) have no /dev device, so
/// they only show up if mounted in one of the user-facing places.
#[cfg(any(not(target_os = "macos"), test))]
fn user_mounts(text: &str, home: Option<&Path>) -> Vec<Place> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    const SYSTEM: [&str; 21] = [
        "/bin", "/boot", "/dev", "/efi", "/etc", "/gnu", "/home", "/lib", "/lib64", "/nix", "/opt",
        "/proc", "/root", "/run", "/sbin", "/snap", "/srv", "/sys", "/tmp", "/usr", "/var",
    ];
    let mut places: Vec<Place> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split(' ');
        let (Some(device), Some(mount)) = (fields.next(), fields.next()) else {
            continue;
        };
        let path = PathBuf::from(OsString::from_vec(unescape_octal(mount)));
        let shown = if path == Path::new("/") || mount.contains("/.") {
            // Mounts in hidden folders (snapshots, sandboxes) are internal.
            false
        } else if ["/media", "/run/media", "/mnt"]
            .iter()
            .any(|dir| path.starts_with(dir))
            || home.is_some_and(|home| path.starts_with(home) && path != home)
        {
            true
        } else {
            device.starts_with("/dev/") && !SYSTEM.iter().any(|dir| path.starts_with(dir))
        };
        if !shown {
            continue;
        }
        // A later mount on the same folder hides the earlier one.
        places.retain(|place| place.path != path);
        places.push(Place {
            name: file_name(&path),
            path,
            kind: PlaceKind::Volume,
        });
    }
    places
}

/// /proc/self/mounts writes space, tab, newline and backslash as \ooo.
#[cfg(any(not(target_os = "macos"), test))]
fn unescape_octal(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let code = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)));
        match code {
            Some(digits) => {
                let value = digits.iter().fold(0u32, |n, d| n * 8 + u32::from(d - b'0'));
                out.push(value as u8);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    out
}

/// Listing ~/.Trash needs Full Disk Access for the terminal app; without it,
/// reading the folder fails with "Operation not permitted".
#[cfg(target_os = "macos")]
fn trash_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".Trash"))
}

#[cfg(not(target_os = "macos"))]
fn trash_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
    Some(data.join("Trash/files"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_xdg_user_dirs() {
        let home = Path::new("/home/ana");
        let text = r#"
# written by xdg-user-dirs-update
XDG_DESKTOP_DIR="$HOME/Schreibtisch"
XDG_DOWNLOAD_DIR="/data/downloads"
XDG_MUSIC_DIR="$HOME/"
XDG_VIDEOS_DIR="relative/is/ignored"
"#;
        let dir = |key| user_dir(text, key, home);
        assert_eq!(dir("XDG_DESKTOP_DIR"), Some(home.join("Schreibtisch")));
        assert_eq!(
            dir("XDG_DOWNLOAD_DIR"),
            Some(PathBuf::from("/data/downloads"))
        );
        // Set to the home folder itself: favorites() then leaves it out.
        assert_eq!(dir("XDG_MUSIC_DIR"), Some(home.to_path_buf()));
        assert_eq!(dir("XDG_VIDEOS_DIR"), None);
        assert_eq!(dir("XDG_PICTURES_DIR"), None);
    }

    #[test]
    fn keeps_only_user_facing_mounts() {
        let text = "\
/dev/nvme0n1p2 / ext4 rw 0 0
proc /proc proc rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
/dev/nvme0n1p3 /home ext4 rw 0 0
/dev/loop3 /snap/core22/1380 squashfs ro 0 0
/dev/sdb1 /media/ana/My\\040Stick vfat rw 0 0
/dev/sdc1 /run/media/ana/Backup ext4 rw 0 0
/dev/sdd1 /data xfs rw 0 0
/dev/sdd2 /.snapshots btrfs rw 0 0
nas:/share /mnt/nas nfs4 rw 0 0
sshfs#me@host: /home/ana/remote fuse.sshfs rw 0 0
portal /run/user/1000/doc fuse.portal rw 0 0
/dev/sdd1 /data xfs rw 0 0
";
        let places = user_mounts(text, Some(Path::new("/home/ana")));
        let found: Vec<_> = places
            .iter()
            .map(|p| (p.name.as_str(), p.path.to_str().unwrap()))
            .collect();
        assert_eq!(
            found,
            [
                ("My Stick", "/media/ana/My Stick"),
                ("Backup", "/run/media/ana/Backup"),
                ("nas", "/mnt/nas"),
                ("remote", "/home/ana/remote"),
                ("data", "/data"),
            ]
        );
    }
}
