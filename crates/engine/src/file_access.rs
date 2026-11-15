//! The folders automation may read and write.
//!
//! `lightcraft-cli mcp --automation-read-root <dir> --automation-write-root <dir>` confines the
//! files an agent's commands reach. [`AutomationRoots`] holds the two folders. While they are in force, every path a command
//! reads goes through [`check_read`] and every path it writes through [`check_write`]:
//!
//! - The path is made absolute against the working directory, as the file system reads it. Its
//!   links are followed: for a file that doesn't exist yet, the links of its nearest existing
//!   folder, the rest of the path being plain names (`..` below a missing folder is refused, and so
//!   is a link that leads nowhere).
//! - The result must lie inside the root, compared by whole components (`/work2` isn't inside
//!   `/work`), exactly: on Windows what exists is resolved to its real case first.
//! - Read access is permitted inside either root; write access only inside the write root.
//!
//! Roots are in force on a thread inside [`confine`], and in the whole process after
//! [`confine_process`] (`lightcraft-cli mcp`), except inside [`unconfined`]. Without roots
//! nothing is checked.
//!
//! [`Session::execute`](crate::Session::execute) also checks the paths among a command's
//! parameters before it runs, and refuses a command that isn't classified: see [`commands`].
//!
//! A path is checked, then opened: a process that can change the folders in between (swapping in
//! a link) can still win that race. A file with more than one name is refused, because another
//! name may lie outside the roots, and a name count that cannot be read is refused too. A file
//! with one name is an ordinary file. The roots keep an agent to the files it was given; they don't
//! guard against other programs on the computer.

pub mod commands;

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, OnceLock};

pub use commands::check_command;

/// Read access or write access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// The folders automation may read and write; a root left out grants no access of its kind.
#[derive(Debug)]
pub struct AutomationRoots {
    read: Option<Root>,
    write: Option<Root>,
    /// Which roots these are, among all that were made in this process: never reused, unlike an address.
    generation: usize,
}

/// An existing filesystem root folder with its canonical path and display path.
#[derive(Debug)]
struct Root {
    /// The folder made absolute, links followed: what paths are compared with.
    resolved: PathBuf,
    /// The same without Windows' `\\?\` prefix, for people.
    shown: PathBuf,
}

impl Root {
    /// The folder at `path`, which must exist.
    fn open(path: &Path, access: Access) -> Result<Self, String> {
        let kind = match access {
            Access::Read => "read",
            Access::Write => "write",
        };
        // The operator's own choice may be a share; a device is no folder to confine to.
        if unsafe_prefix(path).is_some_and(|p| !matches!(p, Prefix::UNC(..) | Prefix::VerbatimUNC(..))) {
            return Err(format!("cannot open the automation {kind} root `{}`: network and device paths are not allowed", path.display()));
        }
        let resolved = std::fs::canonicalize(path)
            .and_then(|p| if p.is_dir() { Ok(p) } else { Err(std::io::ErrorKind::NotADirectory.into()) })
            .map_err(|e| format!("cannot open the automation {kind} root `{}`: {e}", path.display()))?;
        Ok(Self { shown: plain(resolved.clone()), resolved })
    }
}

impl AutomationRoots {
    /// The roots `read` and `write`, which must be existing folders; `None` when neither is given
    /// (nothing is confined then).
    pub fn new(read: Option<&Path>, write: Option<&Path>) -> Result<Option<Arc<Self>>, String> {
        if read.is_none() && write.is_none() {
            return Ok(None);
        }
        let read = read.map(|p| Root::open(p, Access::Read)).transpose()?;
        let write = write.map(|p| Root::open(p, Access::Write)).transpose()?;
        static GENERATION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Some(Arc::new(Self { read, write, generation })))
    }

    /// The read root, absolute with its links followed.
    pub fn read_root(&self) -> Option<&Path> {
        self.read.as_ref().map(|r| r.shown.as_path())
    }

    /// The write root, absolute with its links followed.
    pub fn write_root(&self) -> Option<&Path> {
        self.write.as_ref().map(|r| r.shown.as_path())
    }

    /// May the file or folder at `path` be read (or written)? `Err` says why not.
    ///
    /// A file with more than one name is rejected: another name may lie outside the roots, and
    /// opening this one would read or change that file too. A name count that cannot be read is
    /// rejected for the same reason. A directory is judged only by where it is, because its link
    /// count includes `.`, `..` and child directories rather than extra names of the directory.
    pub fn check(&self, path: impl AsRef<Path>, access: Access) -> Result<(), String> {
        let path = path.as_ref();
        self.check_location(path, access)?;
        one_name(path)
    }

    /// May `path` and its known placed destination be written/removed without another name
    /// escaping the roots? A same-file pair is allowed only when those are its exactly two names.
    pub fn check_write_pair(&self, path: impl AsRef<Path>, placed: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let placed = placed.as_ref();
        self.check_location(path, Access::Write)?;
        self.check_location(placed, Access::Write)?;
        // one entry named twice is no pair: its other name, if it has one, is somebody else's
        let one_entry = path == placed || std::fs::canonicalize(path).ok().is_some_and(|a| std::fs::canonicalize(placed).ok() == Some(a));
        if one_entry {
            return one_name(path);
        }
        let same = lightcraft_catalog::safe_file::same_file_strict(path, placed)
            .map_err(|e| format!("automation path rejected: cannot verify the placed file pair ({path:?}, {placed:?}): {e}"))?;
        if same {
            // two names of one file, not a link to it: the count follows links, and a link left standing
            // when the source goes would point at nothing
            for name in [path, placed] {
                if std::fs::symlink_metadata(name).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(format!("automation path rejected: the placed file pair is a link, not a second name: {}", name.display()));
                }
            }
            match lightcraft_catalog::safe_file::link_count(path) {
                Ok(2) => Ok(()),
                Ok(count) => Err(format!("automation path rejected: the placed file pair has {count} names, not two: {}", path.display())),
                Err(e) => Err(format!("automation path rejected: cannot verify the names of the placed file pair at {}: {e}", path.display())),
            }
        } else {
            one_name(path)?;
            one_name(placed)
        }
    }

    /// Check root membership and link-entry location without applying the multiple-name rule.
    ///
    /// A path that is not inside answers with one error, whether or not it exists. The root
    /// folders are logged, not returned: the client must not learn the server's layout, nor
    /// whether an outside folder is there.
    fn check_location(&self, path: &Path, access: Access) -> Result<(), String> {
        let roots: Vec<&Root> = match access {
            Access::Read => self.read.iter().chain(self.write.iter()).collect(),
            Access::Write => self.write.iter().collect(),
        };
        if roots.is_empty() {
            return match access {
                Access::Read => Err(format!("automation filesystem access is not granted: read authority is absent: {}", path.display())),
                Access::Write => Err(format!("automation filesystem access is not granted: write authority is absent: {}", path.display())),
            };
        }
        let in_roots = |p: &Path| roots.iter().any(|r| inside(p, &r.resolved));
        let resolved = match self.resolve(path) {
            Ok(found) => found,
            Err(Unresolved::Named(e)) => return Err(e),
            Err(Unresolved::Hidden(e)) if self.dangling_link_inside(path, &in_roots) => return Err(e),
            Err(Unresolved::Hidden(_)) => return Err(outside_error(access, path)),
        };
        let mut allowed = in_roots(&resolved);
        // Writing to a link replaces, renames or removes the link where it stands, whatever it points to.
        if allowed
            && access == Access::Write
            && let Some(folder) = folder_of_link(path)
        {
            allowed = self.resolve(folder).is_ok_and(|found| in_roots(&found));
        }
        if allowed {
            return Ok(());
        }
        Err(outside_error(access, path))
    }

    /// May a new file be created inside this folder, following a folder link to its target?
    /// Unlike [`Self::check`] for a link being changed, only the destination folder matters.
    pub fn check_container(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let Some(root) = &self.write else {
            return Err(format!("automation filesystem access is not granted: write authority is absent: {}", path.display()));
        };
        let resolved = match self.resolve(path) {
            Ok(found) => found,
            Err(Unresolved::Named(e)) => return Err(e),
            Err(Unresolved::Hidden(_)) => return Err(outside_error(Access::Write, path)),
        };
        if inside(&resolved, &root.resolved) {
            // The destination is a folder; its link count is not an extra file name.
            return one_name(path);
        }
        Err(outside_error(Access::Write, path))
    }
}

/// One rejection for every path that is not inside a root. The configured folders are not in it.
fn outside_error(access: Access, path: &Path) -> String {
    let kind = match access {
        Access::Read => "read",
        Access::Write => "write",
    };
    log::debug!("automation path outside the {kind} root: {}", path.display());
    format!("automation path rejected: outside the {kind} root: {}", path.display())
}

/// Why [`AutomationRoots::resolve`] gave no path.
enum Unresolved {
    /// A fault of the path as written (empty, a device name, another computer): the same whether
    /// or not anything is there, so it is told as it is.
    Named(String),
    /// A fault that depends on what exists below the path. A client is told only that the path is
    /// outside the roots, so it can't probe for folders with it.
    Hidden(String),
}

impl AutomationRoots {
    /// Is `path` a link standing in a root? Then "a link that leads nowhere" says what is wrong, and does not
    /// describe an outside path.
    fn dangling_link_inside(&self, path: &Path, in_roots: &impl Fn(&Path) -> bool) -> bool {
        let Some(folder) = folder_of_link(path) else { return false };
        self.resolve(folder).is_ok_and(|found| in_roots(&found))
    }
}

/// Where the roots are in force on a thread ([`confine`], [`unconfined`]).
#[derive(Clone, Debug)]
enum Scope {
    Confined(Arc<AutomationRoots>),
    Unconfined,
}

thread_local! {
    /// This thread's scope; `None`: the process's roots ([`confine_process`]).
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// The roots in force on one thread, to put in force on another: a thread started inside [`confine`]
/// has no scope of its own, so work handed to a worker takes this with it ([`Handoff::run`]). Roots
/// set for the whole process ([`confine_process`]) need none.
#[derive(Clone, Default)]
pub(crate) struct Handoff(Option<Arc<AutomationRoots>>);

impl Handoff {
    /// Which roots these are: the same number for the same roots, and none for no roots.
    pub(crate) fn id(&self) -> usize {
        self.0.as_ref().map_or(0, |r| r.generation)
    }

    /// Run `f` with the roots that were in force when this was made.
    pub(crate) fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        confine(self.0.as_ref(), f)
    }
}

/// The roots in force on this thread now, ready to hand to a worker.
pub(crate) fn handoff() -> Handoff {
    Handoff(current())
}

/// The roots in force in every thread outside [`confine`] and [`unconfined`].
static PROCESS: OnceLock<Arc<AutomationRoots>> = OnceLock::new();

/// Put `roots` in force in the whole process, for good. `false` when other roots were in force
/// already (they stay as they were).
pub fn confine_process(roots: Arc<AutomationRoots>) -> bool {
    PROCESS.set(roots.clone()).is_ok() || PROCESS.get().is_some_and(|r| Arc::ptr_eq(r, &roots))
}

/// Puts the scope it replaced back when dropped (also when `f` panics).
struct Restore(Option<Scope>);

impl Drop for Restore {
    /// Put the thread's previous scope back.
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|s| *s.borrow_mut() = previous);
    }
}

/// Executes `f` within the given thread-local scope, restoring the previous scope on drop.
fn with_scope<T>(scope: Option<Scope>, f: impl FnOnce() -> T) -> T {
    let _restore = Restore(SCOPE.with(|s| s.replace(scope)));
    f()
}

/// Run `f` with `roots` in force on this thread (`None`: as things stand).
///
/// Process roots, once set, cannot be replaced by a different set on this thread. A caller that
/// passes other roots still runs under the process roots.
pub fn confine<T>(roots: Option<&Arc<AutomationRoots>>, f: impl FnOnce() -> T) -> T {
    let roots = match (roots, PROCESS.get()) {
        (Some(asked), Some(process)) if !Arc::ptr_eq(asked, process) => Some(process),
        (asked, _) => asked,
    };
    match roots {
        Some(r) => with_scope(Some(Scope::Confined(r.clone())), f),
        None => f(),
    }
}

/// Run `f` with no roots in force on this thread.
///
/// Crate-private: a host must not wrap arbitrary work in it. The operator's library is opened
/// through [`crate::Session::open_operator_library`], which is the one public use.
pub(crate) fn unconfined<T>(f: impl FnOnce() -> T) -> T {
    with_scope(Some(Scope::Unconfined), f)
}

/// The roots in force on this thread, if any.
pub fn current() -> Option<Arc<AutomationRoots>> {
    match SCOPE.with(|s| s.borrow().clone()) {
        Some(Scope::Confined(r)) => Some(r),
        Some(Scope::Unconfined) => None,
        None => PROCESS.get().cloned(),
    }
}

/// `path` without Windows' `\\?\` prefix (which `canonicalize` adds), for people to read.
fn plain(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    // (a name that isn't Unicode is shown with its prefix: replacing characters would show another name)
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// May `path` be read here? Always, without roots in force.
pub fn check_read(path: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check(path, Access::Read))
}

/// Like [`check_read`], but judged by the process's roots when this thread is [`unconfined`] (the
/// library the operator chose is open): a file that is not the library's own, such as a LUT or
/// profile the library names elsewhere, is no more the operator's choice than any other path.
pub(crate) fn check_read_process(path: impl AsRef<Path>) -> Result<(), String> {
    effective_roots().map_or(Ok(()), |r| r.check(path, Access::Read))
}

/// The roots that judge a path that is not the operator's own choice: this thread's, or the process's when this thread is
/// [`unconfined`].
fn effective_roots() -> Option<Arc<AutomationRoots>> {
    match SCOPE.with(|s| s.borrow().clone()) {
        Some(Scope::Confined(r)) => Some(r),
        Some(Scope::Unconfined) | None => PROCESS.get().cloned(),
    }
}

/// May `path` be written here? Always, without roots in force.
pub fn check_write(path: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check(path, Access::Write))
}

/// Is `path` in a place `access` allows? Only the place: a file with a second name passes, as it
/// may be asked about, though not read or changed. For a look at the disk before the real check, so
/// a path outside the roots gets one answer there or not. Always, without roots in force.
pub(crate) fn check_place(path: impl AsRef<Path>, access: Access) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check_location(path.as_ref(), access))
}

/// May the disk be asked whether `path` is there? Yes when [`check_place`] allows a read.
pub(crate) fn may_probe(path: impl AsRef<Path>) -> bool {
    check_place(path, Access::Read).is_ok()
}

/// May a new file be created inside this folder, following a folder link to its target?
pub fn check_write_container(path: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check_container(path))
}

/// Check source removal when its only other name may be the move's verified destination.
pub fn check_write_pair(path: impl AsRef<Path>, placed: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check_write_pair(path, placed))
}

/// Refuse `path` when it is a file with another name, even if its folder is the library's own.
///
/// Location is not checked: a library copy outside the automation roots is still the operator's
/// file, but a second name of it may not be. Always, without roots in force: the rule is about
/// what an agent could reach, and a library may hold files with a second name. With the process's roots in force, an
/// unconfined thread (the operator's library opening) is judged by them.
pub(crate) fn reject_other_names(path: &Path) -> Result<(), String> {
    // (the library the operator chose opens unconfined, but the process's roots still say what a second name may be)
    if effective_roots().is_none() {
        return Ok(());
    }
    one_name(path)
}

/// Reject `path` when it is a file with another name, or when that count cannot be read.
///
/// `Ok(())` when the path has one name, names a directory, or does not exist yet. A directory is
/// not counted: its link count includes `.`, `..` and child directories.
fn one_name(path: &Path) -> Result<(), String> {
    match has_other_names(path) {
        Ok(false) => Ok(()),
        Ok(true) => Err(format!("automation path rejected: {} has more than one name, and another may lie outside the roots", path.display())),
        Err(e) => Err(format!("automation path rejected: cannot count the names of {}: {e}", path.display())),
    }
}

/// `Ok(true)` when `path` is an existing file with more than one directory entry.
///
/// `Ok(false)` when it has one name, names a directory, or does not exist yet. `Err` when the
/// count cannot be read, or comes back as zero: neither is a count a confinement check can trust,
/// so the caller refuses the path. Symbolic links are followed, so the count is the file a read
/// would open.
fn has_other_names(path: &Path) -> std::io::Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        return Ok(false);
    }
    // A real file has at least one name. Zero is not a count we can trust.
    match lightcraft_catalog::safe_file::link_count(path)? {
        1 => Ok(false),
        0 => Err(std::io::Error::other("the number of names was 0")),
        _ => Ok(true),
    }
}

/// The folder `path` stands in when its last name is a link (`a/link/` and `a/link/.` are that link too).
fn folder_of_link(path: &Path) -> Option<&Path> {
    let path = path.components().as_path();
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return None;
    }
    Some(path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")))
}

impl AutomationRoots {
    /// `path` made absolute with its links followed; for a path that doesn't exist (yet), its nearest
    /// existing folder's links followed and the rest appended.
    fn resolve(&self, path: &Path) -> Result<PathBuf, Unresolved> {
        if path.as_os_str().is_empty() {
            return Err(Unresolved::Named("automation path rejected: path is empty".into()));
        }
        // Checked as given, and first: making `…\NUL` absolute gives the device path `\\.\NUL` on Windows,
        // which the prefix check would refuse for the wrong reason.
        if cfg!(windows) && path.components().any(|c| matches!(c, Component::Normal(n) if n.to_str().is_none_or(is_device_name))) {
            return Err(Unresolved::Named(format!("automation path rejected: reserved device names are not allowed: {}", path.display())));
        }
        self.reject_unsafe_prefix(path)?;
        let absolute =
            std::path::absolute(path).map_err(|e| Unresolved::Hidden(format!("automation path rejected: cannot resolve {}: {e}", path.display())))?;
        // before any link on the way is followed: following one that leads to another computer contacts it
        if self.link_leads_to_network(&absolute, MAX_LINK_HOPS) {
            return Err(Unresolved::Named(format!("automation path rejected: network and device paths are not allowed: {}", path.display())));
        }
        // The names below the nearest existing folder, last first.
        let mut missing: Vec<OsString> = vec![];
        let mut at = absolute.as_path();
        loop {
            match std::fs::canonicalize(at) {
                Ok(mut found) => {
                    found.extend(missing.iter().rev());
                    return Ok(found);
                }
                Err(e) => match std::fs::symlink_metadata(at) {
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err(Unresolved::Hidden(format!("automation path rejected: a link that leads nowhere: {}", path.display())));
                    }
                    // There, yet its real path can't be had (no permission): nothing to compare.
                    Ok(_) => return Err(Unresolved::Hidden(format!("automation path rejected: cannot resolve {}: {e}", path.display()))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(Unresolved::Hidden(format!("automation path rejected: cannot resolve {}: {e}", path.display()))),
                },
            }
            let parent = at.parent();
            match (at.components().next_back(), parent) {
                (Some(Component::Normal(name)), Some(parent)) => {
                    missing.push(name.to_os_string());
                    at = parent;
                }
                (Some(Component::ParentDir), _) => {
                    return Err(Unresolved::Hidden(format!(
                        "automation path rejected: parent traversal below a folder that doesn't exist: {}",
                        path.display()
                    )));
                }
                // Down to the root (or drive) and it doesn't exist either.
                _ => {
                    let e = std::io::Error::from(std::io::ErrorKind::NotFound);
                    return Err(Unresolved::Hidden(format!("automation path rejected: cannot resolve {}: {e}", path.display())));
                }
            }
        }
    }

    /// Refuse a path that leads to another computer or a device before anything opens it. Opening
    /// `\\host\share\x` contacts `host` and offers it this account's credentials, whatever the roots
    /// would say afterwards, so the path is judged as written. A drive is let through, and so is a
    /// share one of the roots is on.
    fn reject_unsafe_prefix(&self, path: &Path) -> Result<(), Unresolved> {
        let refused = || Unresolved::Named(format!("automation path rejected: network and device paths are not allowed: {}", path.display()));
        if cfg!(windows) && is_nt_path(path) {
            return Err(refused());
        }
        // (making a path absolute opens nothing; a relative path takes the working folder's prefix)
        let absolute = std::path::absolute(path).ok();
        if [Some(path), absolute.as_deref()].into_iter().flatten().any(|p| self.prefix_is_refused(p)) {
            return Err(refused());
        }
        Ok(())
    }

    /// Does `path` start on another computer or a device that no root is on?
    fn prefix_is_refused(&self, path: &Path) -> bool {
        unsafe_prefix(path).is_some_and(|prefix| {
            let shares = self.read.iter().chain(self.write.iter()).filter_map(|r| unsafe_prefix(&r.resolved));
            !prefix_allowed(prefix, shares)
        })
    }

    /// Is a link on the way to `path` one whose target is another computer or a device that no root
    /// is on? Looked at without following it: following one (`canonicalize`) contacts the host, so
    /// the targets are read from the links themselves, down a bounded chain of them. A chain longer
    /// than that is refused, not trusted: its end is out of sight. Windows only.
    fn link_leads_to_network(&self, path: &Path, hops: u8) -> bool {
        if !cfg!(windows) {
            return false;
        }
        let mut at = PathBuf::new();
        for component in path.components() {
            at.push(component);
            if !matches!(component, Component::Normal(_)) || !std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            if hops == 0 {
                return true;
            }
            let Ok(target) = std::fs::read_link(&at) else { continue };
            // a relative target is read from the folder the link stands in
            let target = if target.is_relative() { at.parent().map_or(target.clone(), |folder| folder.join(&target)) } else { target };
            if self.prefix_is_refused(&target) || self.link_leads_to_network(&target, hops - 1) {
                return true;
            }
        }
        false
    }
}

/// How many links in a row are read for a target; a longer chain is refused.
const MAX_LINK_HOPS: u8 = 8;

/// The prefix of `path` when it is not a plain drive (`C:` or `\\?\C:`); only Windows paths have one.
fn unsafe_prefix(path: &Path) -> Option<Prefix<'_>> {
    match path.components().next() {
        Some(Component::Prefix(p)) if !matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => Some(p.kind()),
        _ => None,
    }
}

/// Is a prefix that is no drive one of the `shares` a root is on? A share is the same server and
/// share name in either case, in the plain or the `\\?\UNC\` spelling. A device (`\\.\`) or any other
/// verbatim name (`\\?\GLOBALROOT`) is never one.
fn prefix_allowed<'a>(prefix: Prefix<'_>, mut shares: impl Iterator<Item = Prefix<'a>>) -> bool {
    let (Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share)) = prefix else { return false };
    shares.any(|root| match root {
        Prefix::UNC(s, n) | Prefix::VerbatimUNC(s, n) => server.eq_ignore_ascii_case(s) && share.eq_ignore_ascii_case(n),
        _ => false,
    })
}

/// Does `path` start with `\\??\`, which Windows takes as a raw system path and [`Path`] does not parse?
fn is_nt_path(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    text.starts_with(r"\??\") || text.starts_with("/??/")
}

/// Is `path` (resolved) `root` or inside it, component by component? Exactly, on Windows too: what
/// exists has its real case from [`resolve`], and a folder that doesn't exist yet may be a different
/// one from the root where a parent folder is case sensitive.
fn inside(path: &Path, root: &Path) -> bool {
    let mut path = path.components();
    root.components().all(|r| path.next() == Some(r))
}

/// A Windows device name (`CON`, `NUL`, `COM1`… also with an extension or a stream, as in `NUL.txt` and `NUL:x`), which names no file.
fn is_device_name(component: &str) -> bool {
    let stem = component.split(['.', ':']).next().unwrap_or(component).trim_end_matches(' ').to_ascii_uppercase();
    let numbered =
        |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that differs in case is another name until the file system says otherwise.
    #[test]
    fn names_are_compared_exactly() {
        let root = Path::new("/data/Photos");
        assert!(inside(Path::new("/data/Photos/new/file"), root));
        assert!(!inside(Path::new("/data/PHOTOS/file"), root));
        assert!(!inside(Path::new("/DATA/Photos/file"), root));
        if cfg!(windows) {
            let root = Path::new(r"C:\data\Photos");
            assert!(inside(Path::new(r"c:\data\Photos\file"), root), "a drive letter is one drive in either case");
            assert!(!inside(Path::new(r"C:\data\PHOTOS\file"), root));
        }
    }

    /// In a case-sensitive folder a missing `PHOTOS` is not the root `Photos`, and creating it would leave the root.
    #[cfg(windows)]
    #[test]
    fn a_case_sensitive_folder_keeps_a_differently_cased_name_out() {
        let base = std::env::temp_dir().join(format!("lc-access-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let set = std::process::Command::new("fsutil").args(["file", "setCaseSensitiveInfo"]).arg(&base).arg("enable").output().unwrap();
        if !set.status.success() {
            let _ = std::fs::remove_dir_all(&base);
            lightcraft_testkit::skip!(System, "fsutil could not make a case-sensitive folder: {set:?}");
            return;
        }
        println!("CASE-SENSITIVE-FOLDER-USED");
        std::fs::create_dir(base.join("Photos")).unwrap();
        let roots = AutomationRoots::new(None, Some(&base.join("Photos"))).unwrap().unwrap();
        assert_eq!(roots.check(base.join("Photos/new.txt"), Access::Write), Ok(()));
        let e = roots.check(base.join("PHOTOS/new.txt"), Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "{e}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A junction is a link too: removing or renaming one acts on it where it stands.
    #[cfg(windows)]
    #[test]
    fn a_junction_is_written_where_it_stands() {
        let base = std::env::temp_dir().join(format!("lc-access-junction-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let junction = outside.join("backdir");
        let made = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(&junction).arg(root.join("sub")).output().unwrap();
        if !made.status.success() {
            let _ = std::fs::remove_dir_all(&base);
            lightcraft_testkit::skip!(System, "mklink /J could not make a junction: {made:?}");
            return;
        }
        let roots = AutomationRoots::new(Some(&root), Some(&root)).unwrap().unwrap();
        let e = roots.check(&junction, Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "{e}");
        assert!(roots.check(&junction, Access::Read).is_ok(), "read follows it");
        assert!(roots.check(junction.join("new"), Access::Write).is_ok(), "a file under it really is in the root");
        let _ = std::fs::remove_dir(&junction);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Writing to a link replaces, renames or removes the link where it stands, whatever it points to.
    #[cfg(unix)]
    #[test]
    fn a_link_is_written_where_it_stands() {
        let base = std::env::temp_dir().join(format!("lc-access-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("file"), b"x").unwrap();
        let link = |target: &Path, at: &Path| std::os::unix::fs::symlink(target, at).unwrap();
        link(&root.join("file"), &outside.join("back"));
        link(&root.join("sub"), &outside.join("backdir"));
        link(&root, &outside.join("root-alias"));
        link(&root.join("file"), &root.join("near"));
        let roots = AutomationRoots::new(Some(&root), Some(&root)).unwrap().unwrap();

        assert!(roots.check_container(outside.join("root-alias")).is_ok(), "new files go into the resolved destination folder");
        assert!(
            roots.check(outside.join("root-alias"), Access::Write).unwrap_err().contains("outside the write root"),
            "changing the link itself is still refused"
        );

        for refused in ["outside/back", "outside/backdir", "outside/backdir/", "outside/backdir/."] {
            let e = roots.check(base.join(refused), Access::Write).unwrap_err();
            assert!(e.contains("outside the write root"), "{refused}: {e}");
            assert!(roots.check(base.join(refused), Access::Read).is_ok(), "{refused}: read follows the link");
        }
        // inside a linked folder a file really is in the root; a link in the root stands in the root
        for allowed in ["outside/backdir/new", "root/near", "root/file", "root/new", "root"] {
            assert!(roots.check(base.join(allowed), Access::Write).is_ok(), "{allowed}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A fresh folder holding `in` (read root), `out` (write root) and `outside`.
    fn folders(name: &str) -> (PathBuf, Arc<AutomationRoots>) {
        let base = std::env::temp_dir().join(format!("lc-file-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["in/sub", "out", "outside", "in2"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("in/a.jpg"), "x").unwrap();
        std::fs::write(base.join("outside/secret.jpg"), "x").unwrap();
        let roots = AutomationRoots::new(Some(&base.join("in")), Some(&base.join("out"))).unwrap().unwrap();
        (base, roots)
    }

    /// A path as text.
    fn s(p: PathBuf) -> String {
        p.to_string_lossy().into_owned()
    }

    /// Paths inside the roots pass and others dont.
    #[test]
    fn paths_inside_the_roots_pass_and_others_dont() {
        let (base, roots) = folders("inside");
        assert_eq!(roots.check(s(base.join("in/a.jpg")), Access::Read), Ok(()));
        assert_eq!(roots.check(s(base.join("in")), Access::Read), Ok(()), "the root itself");
        assert_eq!(roots.check(s(base.join("in/sub/new.jpg")), Access::Read), Ok(()), "missing files are checked by their folder");
        assert_eq!(roots.check(s(base.join("out/new/deeper/x.png")), Access::Write), Ok(()), "and by their nearest folder");
        let e = roots.check(s(base.join("outside/secret.jpg")), Access::Read).unwrap_err();
        assert!(e.starts_with("automation path rejected: outside the read root"), "{e}");
        // Read and write access.
        let e = roots.check(s(base.join("in/a.jpg")), Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "{e}");
        assert_eq!(roots.check(s(base.join("out/x.png")), Access::Read), Ok(()), "reads are permitted under write root");
        let there = roots.check(s(base.join("outside/secret.jpg")), Access::Read).unwrap_err();
        let gone = roots.check(s(base.join("outside/no-such.jpg")), Access::Read).unwrap_err();
        assert!(there.contains("outside the read root") && gone.contains("outside the read root"), "{there} / {gone}");
        assert!(!there.contains("doesn't exist") && !gone.contains("doesn't exist"), "{there} / {gone}");
        let root = roots.read_root().unwrap().display().to_string();
        assert!(!there.contains(&root) && !gone.contains(&root), "the configured root is not in the error");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A sibling with the same prefix is outside.
    #[test]
    fn a_sibling_with_the_same_prefix_is_outside() {
        let (base, roots) = folders("prefix");
        std::fs::write(base.join("in2/b.jpg"), "x").unwrap();
        assert!(roots.check(s(base.join("in2/b.jpg")), Access::Read).is_err(), "in2 is not inside in");
        assert!(roots.check(format!("{}2/c.jpg", s(base.join("in"))), Access::Read).is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    /// Parent traversal is resolved or refused.
    #[test]
    fn parent_traversal_is_resolved_or_refused() {
        let (base, roots) = folders("dotdot");
        let sep = std::path::MAIN_SEPARATOR;
        let up = format!("{}{sep}..{sep}outside{sep}secret.jpg", s(base.join("in")));
        assert!(roots.check(&up, Access::Read).unwrap_err().contains("outside the read root"), "`..` out of the root");
        let back = format!("{}{sep}..{sep}a.jpg", s(base.join("in/sub")));
        assert_eq!(roots.check(&back, Access::Read), Ok(()), "`..` that stays inside");
        let missing = format!("{}{sep}nope{sep}..{sep}..{sep}outside{sep}x.jpg", s(base.join("out")));
        let e = roots.check(&missing, Access::Write).unwrap_err();
        // The missing component must not be reported: that would say whether `nope` exists.
        assert!(e.contains("outside the write root") && !e.contains("doesn't exist"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// Relative paths resolve against the working directory.
    #[test]
    fn relative_paths_resolve_against_the_working_directory() {
        let (base, roots) = folders("relative");
        assert!(roots.check("a.jpg", Access::Read).unwrap_err().contains("outside the read root"));
        assert!(roots.check("", Access::Read).unwrap_err().contains("path is empty"));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A missing root is an error and none is no confinement.
    #[test]
    fn a_missing_root_is_an_error_and_none_is_no_confinement() {
        let (base, _) = folders("missing");
        let e = AutomationRoots::new(Some(&base.join("nope")), None).unwrap_err();
        assert!(e.starts_with("cannot open the automation read root"), "{e}");
        let e = AutomationRoots::new(None, Some(&base.join("in/a.jpg"))).unwrap_err();
        assert!(
            e.starts_with("cannot open the automation write root")
                && e.ends_with(&std::io::Error::from(std::io::ErrorKind::NotADirectory).to_string()),
            "{e}"
        );
        assert!(AutomationRoots::new(Some(Path::new("")), None).unwrap_err().starts_with("cannot open the automation read root ``"));
        assert!(AutomationRoots::new(None, None).unwrap().is_none());
        // One root alone grants only its own access.
        let read_only = AutomationRoots::new(Some(&base.join("in")), None).unwrap().unwrap();
        let e = read_only.check(s(base.join("in/x.jpg")), Access::Write).unwrap_err();
        assert!(e.contains("write authority is absent"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// Links are followed before comparing.
    #[test]
    fn links_are_followed_before_comparing() {
        let (base, roots) = folders("links");
        #[cfg(unix)]
        let made = |target: &Path, link: &Path| std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        let made = |target: &Path, link: &Path| {
            let made =
                if target.is_dir() { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) };
            made.is_ok()
        };
        #[cfg(not(any(unix, windows)))]
        let made = |_: &Path, _: &Path| false;
        if !made(&base.join("outside/secret.jpg"), &base.join("in/leak.jpg")) {
            let _ = std::fs::remove_dir_all(&base);
            lightcraft_testkit::skip!(System, "symbolic links can't be made here");
            return;
        }
        assert!(roots.check(s(base.join("in/leak.jpg")), Access::Read).unwrap_err().contains("outside the read root"), "a file link out");
        assert!(made(&base.join("outside"), &base.join("out/away")));
        let e = roots.check(s(base.join("out/away/new.png")), Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "a folder link out: {e}");
        assert!(made(&base.join("outside/gone.jpg"), &base.join("out/dangling.png")));
        let e = roots.check(s(base.join("out/dangling.png")), Access::Write).unwrap_err();
        assert!(e.contains("a link that leads nowhere"), "writing would create the file outside: {e}");
        // A link to a folder inside the root stays inside.
        assert!(made(&base.join("in/sub"), &base.join("in/alias")));
        assert_eq!(roots.check(s(base.join("in/alias/x.jpg")), Access::Read), Ok(()));
        // A root given through a link is the folder it leads to.
        assert!(made(&base.join("in"), &base.join("in-link")));
        let linked = AutomationRoots::new(Some(&base.join("in-link")), None).unwrap().unwrap();
        assert_eq!(linked.check(s(base.join("in/a.jpg")), Access::Read), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// On windows a path takes its real case and devices are refused.
    #[cfg(windows)]
    #[test]
    fn on_windows_a_path_takes_its_real_case_and_devices_are_refused() {
        let (base, roots) = folders("case");
        let upper = s(base.join("IN/a.jpg"));
        assert_eq!(roots.check(&upper, Access::Read), Ok(()), "{upper}");
        assert_eq!(roots.check(s(base.join("In/Sub/New.jpg")), Access::Read), Ok(()));
        let e = roots.check(s(base.join("out/NUL")), Access::Write).unwrap_err();
        assert!(e.contains("reserved device names"), "{e}");
        assert!(roots.check(s(base.join("out/com1.txt")), Access::Write).is_err());
        assert_eq!(roots.check(s(base.join("out/console.txt")), Access::Write), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A second name is refused only while roots are in force, for the library's own files too.
    #[test]
    fn a_second_name_is_refused_for_a_library_file_only_while_roots_are_in_force() {
        let (base, roots) = folders("second-name");
        let (file, other) = (base.join("out/profile.dcp"), base.join("outside/also.dcp"));
        std::fs::write(&file, b"x").unwrap();
        if std::fs::hard_link(&file, &other).is_err() {
            lightcraft_testkit::skip!(System, "this volume has no hard links");
            return;
        }
        assert_eq!(reject_other_names(&file), Ok(()), "no roots: the library's file is read as it always was");
        let e = confine(Some(&roots), || reject_other_names(&file)).unwrap_err();
        assert!(e.contains("more than one name"), "{e}");
        assert_eq!(confine(Some(&roots), || unconfined(|| reject_other_names(&file))), Ok(()), "unconfined: the operator's own open");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A file with another name is refused, inside the root or not: the other name may lie outside it.
    /// A file with one name, a missing name, and a directory that has child folders all pass.
    #[test]
    fn a_file_with_hard_links_is_refused() {
        let base = std::env::temp_dir().join(format!("lc-file-access-hard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let file = root.join("a.jpg");
        std::fs::write(&file, b"x").unwrap();
        let roots = AutomationRoots::new(Some(&root), Some(&root)).unwrap().unwrap();
        assert_eq!(roots.check(&file, Access::Read), Ok(()), "one name");
        assert_eq!(roots.check(&file, Access::Write), Ok(()));
        assert_eq!(roots.check(&root, Access::Read), Ok(()), "a directory's link count is not extra names");
        assert_eq!(roots.check(root.join("new.jpg"), Access::Write), Ok(()), "a name that does not exist yet");
        let other = outside.join("also.jpg");
        match std::fs::hard_link(&file, &other) {
            Ok(()) => {}
            Err(e) => {
                let _ = std::fs::remove_dir_all(&base);
                lightcraft_testkit::skip!(System, "no hard links in {}: {e}", base.display());
                return;
            }
        }
        for access in [Access::Read, Access::Write] {
            let e = roots.check(&file, access).unwrap_err();
            assert!(e.contains("more than one name"), "{access:?}: {e}");
        }
        std::fs::remove_file(&other).unwrap();
        assert_eq!(roots.check(&file, Access::Read), Ok(()), "the extra name is gone");
        // Both names are inside the root. The other one is still invisible to a path check.
        std::fs::hard_link(&file, root.join("b.jpg")).unwrap();
        let e = roots.check(&file, Access::Read).unwrap_err();
        assert!(e.contains("more than one name"), "{e}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Scopes nest and stay on their thread.
    #[test]
    fn scopes_nest_and_stay_on_their_thread() {
        let (base, roots) = folders("scopes");
        let outside = s(base.join("outside/secret.jpg"));
        assert_eq!(check_read(&outside), Ok(()), "nothing is confined by default");
        confine(Some(&roots), || {
            assert!(check_read(&outside).is_err());
            assert_eq!(unconfined(|| check_read(&outside)), Ok(()));
            assert!(check_read(&outside).is_err(), "restored after unconfined");
            let o = outside.clone();
            let there = std::thread::spawn(move || check_read(&o).is_ok()).join().unwrap();
            assert!(there, "another thread is not confined by this thread's scope");
        });
        assert_eq!(check_read(&outside), Ok(()), "restored after confine");
        let r = std::panic::catch_unwind(|| confine(Some(&roots), || panic!("boom")));
        assert!(r.is_err());
        assert_eq!(check_read(&outside), Ok(()), "restored after a panic");
        assert_eq!(confine(None, || check_read(&outside)), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A name that isn't Unicode is shown as it is.
    #[cfg(windows)]
    #[test]
    fn a_name_that_is_not_unicode_keeps_its_prefix() {
        use std::os::windows::ffi::OsStringExt;
        let wide: Vec<u16> = r"\\?\C:\root\".encode_utf16().chain([0xD800]).collect();
        let path = PathBuf::from(std::ffi::OsString::from_wide(&wide));
        assert_eq!(plain(path.clone()), path);
    }

    /// Device names.
    #[test]
    fn device_names() {
        for d in ["CON", "nul", "Com1", "LPT9.txt", "aux.jpg", "NUL ", "COM¹", "lpt³.txt", "COM1:", "NUL:x", "con:stream.txt"] {
            assert!(is_device_name(d), "{d}");
        }
        for f in ["console", "com10", "COM0", "lpt", "a.jpg", "nul-x"] {
            assert!(!is_device_name(f), "{f}");
        }
    }

    /// A share is let through only when a root is on it, and a device or other verbatim name never.
    #[test]
    fn a_prefix_is_a_drive_or_a_roots_share() {
        use std::ffi::OsStr;
        let unc = |server, share| Prefix::UNC(OsStr::new(server), OsStr::new(share));
        let allowed = |prefix, roots: Vec<Prefix<'static>>| prefix_allowed(prefix, roots.into_iter());
        assert!(!allowed(unc("203.0.113.5", "s"), vec![]), "no root is on a share");
        assert!(!allowed(unc("203.0.113.5", "s"), vec![unc("host", "s")]));
        assert!(!allowed(Prefix::VerbatimUNC(OsStr::new("203.0.113.5"), OsStr::new("s")), vec![]));
        assert!(!allowed(unc("host@SSL", "s"), vec![unc("host", "s")]), "another server");
        assert!(!allowed(unc("host", "other"), vec![unc("host", "s")]), "another share");
        assert!(allowed(unc("HOST", "S"), vec![Prefix::VerbatimUNC(OsStr::new("host"), OsStr::new("s"))]), "same share, other case and spelling");
        assert!(!allowed(Prefix::DeviceNS(OsStr::new("pipe")), vec![unc("host", "s")]));
        assert!(!allowed(Prefix::Verbatim(OsStr::new("GLOBALROOT")), vec![unc("host", "s")]));
        assert!(unsafe_prefix(Path::new("/data/x")).is_none() && unsafe_prefix(Path::new("")).is_none());
        assert!(is_nt_path(Path::new(r"\??\UNC\203.0.113.5\s\x")) && is_nt_path(Path::new("/??/C:/x")));
        assert!(!is_nt_path(Path::new("/data/??/x")));
    }

    /// A path to another computer or a device is refused for what it says, without a look for it:
    /// opening it would contact the host, and an address that answers nothing would make the look
    /// slow and end in a resolve error instead.
    #[cfg(windows)]
    #[test]
    fn a_network_path_is_refused_before_it_is_opened() {
        let (base, roots) = folders("network");
        for (path, prefix) in [
            (r"\\203.0.113.5\s\x.dng", r"\\203.0.113.5\s"),
            ("//203.0.113.5/s/x.dng", "//203.0.113.5/s"),
            (r"\\?\UNC\203.0.113.5\s\x.dng", r"\\?\UNC\203.0.113.5\s"),
            (r"\\host@SSL\s\x.dng", r"\\host@SSL\s"),
            (r"\\.\pipe\x", r"\\.\pipe"),
            (r"\\.\NUL", r"\\.\NUL"),
            (r"\\?\GLOBALROOT\x", r"\\?\GLOBALROOT"),
            (r"\??\UNC\203.0.113.5\s\x", r"\??\"),
        ] {
            for access in [Access::Read, Access::Write] {
                let e = roots.check(path, access).unwrap_err();
                assert!(e.contains("network and device paths are not allowed") && e.contains(prefix), "{path}: {e}");
                assert!(!e.contains("cannot resolve") && !e.contains("outside the"), "{path}: {e}");
            }
        }
        assert!(roots.check(s(base.join("in/a.jpg")), Access::Read).is_ok(), "a drive path still passes");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A pair counts as two names of one file only when neither is a link.
    #[cfg(unix)]
    #[test]
    fn a_link_to_the_source_is_not_its_placed_second_name() {
        let (base, roots) = folders("pair-link");
        let (file, extra, placed_link, placed_name) =
            (base.join("out/a.xmp"), base.join("out/b.xmp"), base.join("out/c.xmp"), base.join("out/d.xmp"));
        std::fs::write(&file, b"x").unwrap();
        std::fs::hard_link(&file, &extra).unwrap();
        std::os::unix::fs::symlink(&file, &placed_link).unwrap();
        let e = roots.check_write_pair(&file, &placed_link).unwrap_err();
        assert!(e.contains("is a link, not a second name"), "{e}");
        std::fs::remove_file(&extra).unwrap();
        std::fs::hard_link(&file, &placed_name).unwrap();
        assert_eq!(roots.check_write_pair(&file, &placed_name), Ok(()), "two hard-link names are a pair");
        let refused = roots.check_write_pair(&file, &file).unwrap_err();
        assert!(refused.contains("more than one name"), "one entry named twice is no pair, whatever its link count: {refused}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// Roots made later are never taken for roots that were dropped: an address can be reused, a generation number is not.
    #[test]
    fn new_roots_never_share_the_number_of_dropped_ones() {
        let dir = std::env::temp_dir().join(format!("lc-roots-generation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let roots = AutomationRoots::new(Some(&dir), None).unwrap().unwrap();
            assert!(seen.insert(confine(Some(&roots), || handoff().id())), "a number was handed out twice");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What is handed to another thread puts the roots in force there; a thread that is handed nothing has none.
    #[test]
    fn a_handoff_puts_the_roots_in_force_on_another_thread() {
        let (base, roots) = folders("handoff");
        let hand = confine(Some(&roots), handoff);
        let outside = s(base.join("outside/secret.jpg"));
        let (o1, o2) = (outside.clone(), outside);
        let (with, without) = std::thread::scope(|sc| {
            let with = sc.spawn(|| hand.run(|| check_read(&o1).is_err()));
            let without = sc.spawn(|| check_read(&o2).is_err());
            (with.join().unwrap(), without.join().unwrap())
        });
        assert!(with, "the handed roots refuse an outside path");
        assert!(!without, "a thread started inside the scope has no roots of its own");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A chain of links too long to read to its end is refused, though every link in it is local.
    #[cfg(windows)]
    #[test]
    fn a_chain_of_links_longer_than_the_budget_is_refused() {
        let (base, roots) = folders("link-chain");
        let real = base.join("in/real");
        std::fs::create_dir_all(&real).unwrap();
        let mut last = real;
        for i in 1..=MAX_LINK_HOPS + 2 {
            let link = base.join(format!("in/hop{i}"));
            if let Err(e) = std::os::windows::fs::symlink_dir(&last, &link) {
                lightcraft_testkit::skip!(System, "a symbolic link could not be made: {e}");
                return;
            }
            last = link;
        }
        assert!(roots.check(s(last.join("x.jpg")), Access::Read).is_err(), "a chain past the budget is refused");
        assert!(roots.check(s(base.join("in/hop1/x.jpg")), Access::Read).is_ok(), "a short one is followed");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A link on the way that leads to another computer is refused from what it says, without being followed.
    #[cfg(windows)]
    #[test]
    fn a_link_to_another_computer_is_refused_without_following_it() {
        let (base, roots) = folders("link-network");
        let link = base.join("in/remote");
        if let Err(e) = std::os::windows::fs::symlink_dir(r"\\203.0.113.5\share", &link) {
            lightcraft_testkit::skip!(System, "a symbolic link could not be made: {e}");
            return;
        }
        for access in [Access::Read, Access::Write] {
            let e = roots.check(s(link.join("x.jpg")), access).unwrap_err();
            assert!(e.contains("network and device paths are not allowed") && !e.contains("cannot resolve"), "{access:?}: {e}");
        }
        let _ = std::fs::remove_dir_all(base);
    }

    /// A refusal that doesn't depend on what exists can't be forged with text in the path: a name
    /// that says "path is empty" below a folder that is there and below one that isn't give one answer.
    #[test]
    fn a_path_cannot_forge_a_refusal_that_shows_what_exists() {
        let (base, roots) = folders("forged");
        let ask = |dir: &str| {
            let path = s(base.join(dir).join("../path is empty"));
            roots.check(&path, Access::Read).unwrap_err().replace(&path, "P")
        };
        let (there, gone) = (ask("outside"), ask("outside/no-such-folder"));
        assert_eq!(there, gone, "{there} / {gone}");
        assert!(there.contains("outside the read root"), "{there}");
        let _ = std::fs::remove_dir_all(base);
    }
}
