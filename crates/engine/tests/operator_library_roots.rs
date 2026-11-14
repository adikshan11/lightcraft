//! The operator's library opens while the process is confined, and a profile file it names somewhere
//! else is still judged by the process's roots. Its own test binary: a process keeps its roots for good.

use lightcraft_engine::Session;
use lightcraft_engine::cmd::lut_profiles::LutProfile;
use lightcraft_engine::file_access::{AutomationRoots, confine_process};

/// A small identity `.cube`.
fn identity_cube() -> String {
    let mut cube = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                cube.push_str(&format!("{r} {g} {b}\n"));
            }
        }
    }
    cube
}

/// The library's own profile loads; one the library names outside the roots does not, though the
/// library is open without roots.
#[test]
fn a_profile_elsewhere_is_judged_by_the_roots_while_the_library_opens() {
    let base = std::env::temp_dir().join(format!("lc-operator-library-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, lib, outside) = (base.join("root"), base.join("lib"), base.join("outside"));
    for d in [&root, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    assert!(confine_process(AutomationRoots::new(Some(&root), Some(&root)).unwrap().unwrap()));
    let mut s = Session::new().with_fs();
    s.open_operator_library(&lib, false).unwrap();
    let own = lib.join("Profiles").join("own.cube");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, identity_cube()).unwrap();
    let elsewhere = outside.join("secret.cube");
    std::fs::write(&elsewhere, identity_cube()).unwrap();
    let entry = |id: &str, file: &std::path::Path| LutProfile {
        id: id.into(),
        name: id.into(),
        group: "Imported".into(),
        file: file.to_string_lossy().to_string(),
    };
    s.lut_profiles = vec![entry("lut:operator-own", &own), entry("lut:operator-elsewhere", &elsewhere)];
    s.persist().unwrap();
    drop(s);
    let mut again = Session::new().with_fs();
    again.open_operator_library(&lib, false).unwrap();
    assert!(lightcraft_pipeline::lut::get("lut:operator-own").is_some(), "the library's own profile loads");
    assert!(lightcraft_pipeline::lut::get("lut:operator-elsewhere").is_none(), "a profile elsewhere is judged by the roots");
    let _ = std::fs::remove_dir_all(&base);
}
