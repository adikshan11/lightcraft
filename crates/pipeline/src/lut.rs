//! 3D LUT profiles: `.cube` files (the plain-text Cube LUT format: `LUT_3D_SIZE`, optional
//! `DOMAIN_MIN` / `DOMAIN_MAX`, then size³ "r g b" lines with red changing fastest), applied to
//! the display-encoded output colour and blended by the profile amount. Profiles are registered
//! here by id (`lut:…`) and found by the finishing stage (CPU loop and GPU kernel alike).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

/// A 3D colour lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: Option<String>,
}

impl Lut3d {
    /// Parse a `.cube` file.
    pub fn parse_cube(text: &str) -> Result<Lut3d, String> {
        let (mut size, mut title) = (0usize, None);
        let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut data = Vec::new();
        let three = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some(r) = l.strip_prefix("TITLE") {
                title = Some(r.trim().trim_matches('"').to_string());
            } else if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
                size = r.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE `{}`", r.trim()))?;
                if !(2..=256).contains(&size) {
                    return Err(format!("LUT_3D_SIZE {size} out of range"));
                }
            } else if l.starts_with("LUT_1D_SIZE") {
                return Err("1D LUTs are not supported (only 3D .cube files)".into());
            } else if let Some(r) = l.strip_prefix("DOMAIN_MIN") {
                min = three(r).ok_or("bad DOMAIN_MIN")?;
            } else if let Some(r) = l.strip_prefix("DOMAIN_MAX") {
                max = three(r).ok_or("bad DOMAIN_MAX")?;
            } else if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
                // a data line of `nan` / `inf` starts with a letter too: it is bad data, not a keyword
                if l.split_whitespace().next().is_some_and(|w| w.parse::<f32>().is_ok()) {
                    return Err(format!("bad line `{l}`"));
                }
                // other keywords (LUT_IN_VIDEO_RANGE…) are ignored
            } else {
                data.push(three(l).ok_or_else(|| format!("bad line `{l}`"))?);
            }
        }
        if size == 0 {
            return Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into());
        }
        if data.len() != size * size * size {
            return Err(format!("expected {} entries, found {}", size * size * size, data.len()));
        }
        Ok(Lut3d { size, data, domain_min: min, domain_max: max, title })
    }

    /// Look up `c` (trilinear).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let mut idx = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let span = (self.domain_max[k] - self.domain_min[k]).max(1e-6);
            let x = (((c[k] - self.domain_min[k]) / span).clamp(0.0, 1.0)) * s;
            let i = (x.floor() as usize).min(n - 2);
            idx[k] = i;
            frac[k] = x - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + n * (g + n * b)];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let c00 = lerp(at(r, g, b), at(r + 1, g, b), frac[0]);
        let c10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), frac[0]);
        let c01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), frac[0]);
        let c11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), frac[0]);
        lerp(lerp(c00, c10, frac[1]), lerp(c01, c11, frac[1]), frac[2])
    }
}

/// A registered LUT and the owners that made it available: `0` is shared by every scope, any other number is one
/// session's (see [`in_scope`]).
struct Entry {
    lut: Arc<Lut3d>,
    owners: HashSet<u64>,
}

fn registry() -> &'static RwLock<HashMap<String, Entry>> {
    static R: OnceLock<RwLock<HashMap<String, Entry>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

thread_local! {
    /// The session whose renders run on this thread (`None`: no session is named, every registered LUT may be used).
    static SCOPE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Run `f` with renders on this thread resolving LUTs for session `owner`: a LUT only another session registered is
/// not found. `None` names no session.
pub fn in_scope<T>(owner: Option<u64>, f: impl FnOnce() -> T) -> T {
    let before = SCOPE.with(|s| s.replace(owner));
    let out = f();
    SCOPE.with(|s| s.set(before));
    out
}

/// The session whose renders run on this thread, to hand to a worker ([`in_scope`]).
pub fn current_scope() -> Option<u64> {
    SCOPE.with(Cell::get)
}

/// Make a LUT available to renders under profile id `id` (`lut:…`), to every scope.
pub fn register(id: &str, lut: Lut3d) {
    register_for(0, id, lut);
}

/// Make a LUT available to renders of session `owner` under profile id `id`. A session that made its own copy of the
/// id shares the entry with the others that did; the last LUT registered is the one all of them use.
pub fn register_for(owner: u64, id: &str, lut: Lut3d) {
    let mut map = registry().write().unwrap_or_else(|e| e.into_inner());
    match map.get_mut(id) {
        Some(entry) => {
            entry.lut = Arc::new(lut);
            entry.owners.insert(owner);
        }
        None => {
            map.insert(id.to_string(), Entry { lut: Arc::new(lut), owners: HashSet::from([owner]) });
        }
    }
}

/// The LUT registered for profile `id`, for the scope of this thread ([`in_scope`]).
pub fn get(id: &str) -> Option<Arc<Lut3d>> {
    if !id.starts_with("lut:") {
        return None;
    }
    let map = registry().read().unwrap_or_else(|e| e.into_inner());
    let entry = map.get(id)?;
    match current_scope() {
        Some(scope) if !entry.owners.contains(&scope) && !entry.owners.contains(&0) => None,
        _ => Some(entry.lut.clone()),
    }
}

/// Forget a LUT profile, for every owner.
pub fn unregister(id: &str) {
    registry().write().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// Forget session `owner`'s claim on a LUT profile; the LUT goes once no owner is left.
pub fn unregister_for(owner: u64, id: &str) {
    let mut map = registry().write().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = map.get_mut(id) {
        entry.owners.remove(&owner);
        if entry.owners.is_empty() {
            map.remove(id);
        }
    }
}

/// Profile ids that render with a LUT.
pub fn is_lut_profile(id: &str) -> bool {
    id.starts_with("lut:")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A LUT registered for one session is found by that session and by a thread that names none, not by another session;
    /// a shared one is found by all, and the LUT goes once its last owner lets it go.
    #[test]
    fn a_registered_lut_is_found_only_in_the_scope_of_its_owners() {
        let lut = || Lut3d::parse_cube(&cube(|c| c, 2)).unwrap();
        let (mine, shared) = ("lut:scope-test-mine", "lut:scope-test-shared");
        register_for(4_000_001, mine, lut());
        register(shared, lut());
        let found = |scope: Option<u64>, id: &str| in_scope(scope, || get(id)).is_some();
        assert!(found(Some(4_000_001), mine) && found(None, mine), "its owner and a thread that names no session");
        assert!(!found(Some(4_000_002), mine), "not another session");
        assert!(found(Some(4_000_002), shared), "a shared one is every session's");
        unregister_for(4_000_001, mine);
        assert!(!found(None, mine), "gone with its last owner");
        unregister(shared);
    }

    fn cube(f: impl Fn([f32; 3]) -> [f32; 3], n: usize) -> String {
        let mut s = format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = f([r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                    s.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
                }
            }
        }
        s
    }

    #[test]
    fn identity_and_swap() {
        let id = Lut3d::parse_cube(&cube(|c| c, 17)).unwrap();
        assert_eq!(id.title.as_deref(), Some("test"));
        let v = id.apply([0.2, 0.55, 0.9]);
        assert!(v.iter().zip([0.2, 0.55, 0.9]).all(|(a, b)| (a - b).abs() < 1e-5), "{v:?}");
        let swap = Lut3d::parse_cube(&cube(|c| [c[2], c[1], c[0]], 9)).unwrap();
        let v = swap.apply([0.1, 0.5, 0.8]);
        assert!((v[0] - 0.8).abs() < 1e-5 && (v[2] - 0.1).abs() < 1e-5, "{v:?}");
        assert!(Lut3d::parse_cube("LUT_1D_SIZE 4\n0 0 0").is_err());
        assert!(Lut3d::parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err(), "too few entries");
    }

    #[test]
    fn non_finite_entries_rejected() {
        let bad_line = |body: &str| {
            let e = Lut3d::parse_cube(body).expect_err(body);
            assert!(e.starts_with("bad line"), "{e}");
        };
        bad_line(&format!("LUT_3D_SIZE 2\n{}", "-inf inf nan\n".repeat(8)));
        bad_line(&format!("LUT_3D_SIZE 2\n{}", "1e39 0 0\n".repeat(8)));
        bad_line(&format!("LUT_3D_SIZE 2\n-inf 0 0\n{}", "0 0 0\n".repeat(7)));
        bad_line(&format!("LUT_3D_SIZE 2\nnan 0 0\n{}", "0 0 0\n".repeat(8)));
        bad_line(&format!("LUT_3D_SIZE 2\ninf 0 0\n{}", "0 0 0\n".repeat(8)));
        let ok = cube(|c| c, 2);
        assert!(Lut3d::parse_cube(&ok).is_ok());
        assert!(Lut3d::parse_cube(&format!("DOMAIN_MIN nan 0 0\n{ok}")).is_err());
        assert!(Lut3d::parse_cube(&format!("DOMAIN_MAX inf 1 1\n{ok}")).is_err());
    }
}
