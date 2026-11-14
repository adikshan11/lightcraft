"""Undo each fix of this round, one at a time, and confirm that its test fails.

usage: python .github/carry/undo.py [windows]    (with `windows`: only the Windows-only mutations)
"""
import subprocess
import sys

ONLY_WINDOWS = len(sys.argv) > 1 and sys.argv[1] == "windows"
ENGINE = "crates/engine/src/"

# (name, file, [(old, new), ...], package, [tests that must fail], windows only)
MUTATIONS = [
    ("a refusal told apart by its text again", ENGINE + "file_access.rs",
     [("Err(Unresolved::Hidden(_)) => return Err(outside_error(access, path)),", "Err(Unresolved::Hidden(e)) => return Err(e),")],
     "lightcraft-engine", ["a_path_cannot_forge_a_refusal_that_shows_what_exists"], False),
    ("a device name cut only at a period", ENGINE + "file_access.rs",
     [("component.split(['.', ':'])", "component.split('.')")],
     "lightcraft-engine", ["device_names"], False),
    ("any share let through", ENGINE + "file_access.rs",
     [("= prefix else { return false };", "= prefix else { return true };")],
     "lightcraft-engine", ["a_prefix_is_a_drive_or_a_roots_share"], False),
    ("a network path let through on Windows", ENGINE + "file_access.rs",
     [("if !prefix_allowed(prefix, &mut shares) {", "if false && !prefix_allowed(prefix, &mut shares) {")],
     "lightcraft-engine", ["a_network_path_is_refused_before_it_is_opened"], True),
    ("every cell of a listing is a path", ENGINE + "file_access/commands.rs",
     [('("library.autoImportScan", &[("listing[0]", Read)]),', '("library.autoImportScan", &[("listing", Read)]),')],
     "lightcraft-engine", ["a_listing_is_judged_by_its_path_column"], False),
    ("only the folder of a place-names dataset is checked", ENGINE + "cmd/geocode.rs",
     [("crate::file_access::check_read(dir.join(name)).map_err(|e| bad(c, e))?;", "let _ = (name, c);")],
     "lightcraft-engine", ["a_place_names_file_that_links_out_of_the_roots_is_refused"], False),
    ("the library's import index read through the agent check", ENGINE + "lightroom_catalog.rs",
     [("if is_link(dir) || is_link(&path) { crate::file_access::check_read(&path) } else { crate::file_access::reject_other_names(&path) }",
       "crate::file_access::check_read(&path)")],
     "lightcraft-engine", ["the_import_index_of_a_library_outside_the_roots_is_read"], False),
    ("a placed pair compared before it is checked", ENGINE + "import_move.rs",
     [("    crate::file_access::check_write_pair(&p.src, &p.dst)?;\n    // compare the bytes now", "    // compare the bytes now"),
      ("    fs::remove_file(&p.src).map_err(|e| format!(\"could not remove the original", "    crate::file_access::check_write_pair(&p.src, &p.dst)?;\n    fs::remove_file(&p.src).map_err(|e| format!(\"could not remove the original")],
     "lightcraft-engine", ["finish_checks_the_paths_before_comparing_the_bytes"], False),
    ("a sidecar save that looks at the original first", ENGINE + "sidecar.rs",
     [("        crate::file_access::check_place(orig, crate::file_access::Access::Read).map_err(EngineError::Other)?;\n", "")],
     "lightcraft-engine", ["an_original_outside_the_roots_is_refused_the_same_whether_or_not_it_is_there"], False),
    ("a move that looks at the original first", ENGINE + "cmd/browse.rs",
     [("        crate::file_access::check_write(path).map_err(|e| bad(C, e))?;\n        if !src.is_file() {", "        if !src.is_file() {"),
      ("        let Some(file) = src.file_name() else", "        crate::file_access::check_write(path).map_err(|e| bad(C, e))?;\n        let Some(file) = src.file_name() else")],
     "lightcraft-engine", ["an_original_outside_the_roots_is_refused_the_same_whether_or_not_it_is_there"], False),
    ("rename probes that answer for any folder", ENGINE + "rename/fs.rs",
     [("crate::file_access::may_probe(p) && std::fs::symlink_metadata(p).is_ok()", "std::fs::symlink_metadata(p).is_ok()")],
     "lightcraft-engine", ["rename_probes_say_nothing_of_a_folder_outside_the_roots"], False),
    ("a library profile with another name refused for writing", ENGINE + "cmd/dcp_profiles.rs",
     [("if library_owned(dir, file) { Ok(()) } else {", "if library_owned(dir, file) { crate::file_access::reject_other_names(file) } else {")],
     "lightcraft-engine", ["importing_over_a_hard_linked_library_profile_preserves_the_other_name", "deleting_a_hard_linked_library_profile_keeps_the_other_name"], False),
    ("the roots named in the initialize reply", "crates/mcp/src/headless.rs",
     [('parts.push("read root set".to_string());', 'parts.push(format!("read: {}", r.read_root().map(|p| p.display().to_string()).unwrap_or_default()));')],
     "lightcraft-mcp", ["initialize_does_not_name_the_roots"], False),
]


def run(args):
    """Run a command, return (exit code, its output)."""
    r = subprocess.run(args, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return r.returncode, r.stdout + r.stderr


failed_to_fail = []
for name, path, edits, package, tests, windows_only in MUTATIONS:
    if windows_only != ONLY_WINDOWS:
        continue
    text = open(path, encoding="utf-8").read()
    for old, new in edits:
        assert text.count(old) == 1, f"{name}: `{old[:60]}` is there {text.count(old)} times"
        text = text.replace(old, new)
    open(path, "w", encoding="utf-8").write(text)
    code, out = run(["cargo", "test", "--locked", "-p", package, "--lib", "--", *tests])
    run(["git", "checkout", "--", path])
    survived = [t for t in tests if f"{t} ... FAILED" not in out]
    verdict = "ok, its tests fail" if not survived else f"STILL PASSES: {survived}"
    print(f"{name}: {verdict}", flush=True)
    if survived:
        print(out[-3000:])
        failed_to_fail.append(name)
sys.exit(1 if failed_to_fail else 0)
