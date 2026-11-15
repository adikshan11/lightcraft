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
     [("if [Some(path), absolute.as_deref()].into_iter().flatten().any(|p| self.prefix_is_refused(p)) {", "if false && [Some(path), absolute.as_deref()].into_iter().flatten().any(|p| self.prefix_is_refused(p)) {")],
     "lightcraft-engine", ["a_network_path_is_refused_before_it_is_opened"], True),
    ("every cell of a listing is a path", ENGINE + "file_access/commands.rs",
     [('("library.autoImportScan", &[("listing[0]", Read)]),', '("library.autoImportScan", &[("listing", Read)]),')],
     "lightcraft-engine", ["a_listing_is_judged_by_its_path_column"], False),
    ("only the folder of a place-names dataset is checked", ENGINE + "cmd/geocode.rs",
     [("crate::file_access::check_read(dir.join(name)).map_err(|e| bad(c, e))?;", "let _ = (name, c);")],
     "lightcraft-engine", ["a_place_names_file_that_links_out_of_the_roots_is_refused"], False),
    ("the library's import index read through the agent check", ENGINE + "lightroom_catalog.rs",
     [("if !intact || is_link(dir) || is_link(&path) { crate::file_access::check_read(&path) } else { crate::file_access::reject_other_names(&path) }",
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
     [("if library_owned(s, dir, file) { Ok(()) } else {", "if library_owned(s, dir, file) { crate::file_access::reject_other_names(file) } else {")],
     "lightcraft-engine", ["importing_over_a_hard_linked_library_profile_preserves_the_other_name", "deleting_a_hard_linked_library_profile_keeps_the_other_name"], False),
    ("a second name refused with no roots in force", ENGINE + "file_access.rs",
     [("    if effective_roots().is_none() {\n        return Ok(());\n    }\n    one_name(path)", "    one_name(path)")],
     "lightcraft-engine", ["a_second_name_is_refused_for_a_library_file_only_while_roots_are_in_force"], False),
    ("a device name judged by its prefix on Windows", ENGINE + "file_access.rs",
     [("if cfg!(windows) && path.components().any(|c| matches!(c, Component::Normal(n) if n.to_str().is_none_or(is_device_name))) {", "if false && path.components().any(|c| matches!(c, Component::Normal(n) if n.to_str().is_none_or(is_device_name))) {")],
     "lightcraft-engine", ["on_windows_a_path_takes_its_real_case_and_devices_are_refused"], True),
    ("a denied LUT left registered", ENGINE + "cmd/lut_profiles.rs",
     [("None => withdraw(s, &p.id),", "None => {}")],
     "lightcraft-engine", ["a_denied_lut_does_not_leave_another_librarys_registration_standing"], False),
    ("an auto-import entry listed without its own check", ENGINE + "cmd/library/import.rs",
     [("            crate::file_access::check_read(e.path()).ok()?;\n", "")],
     "lightcraft-engine", ["the_auto_import_listing_drops_a_link_that_leads_out_of_the_roots"], False),
    ("a shared sidecar judged as a third name", ENGINE + "import_move.rs",
     [("crate::file_access::check_place(&sc, crate::file_access::Access::Read)?;", "crate::file_access::check_read(&sc)?;")],
     "lightcraft-engine", ["a_confined_move_of_two_photos_with_one_sidecar_goes_through"], False),
    ("a link to another computer followed on Windows", ENGINE + "file_access.rs",
     [("if self.link_leads_to_network(&absolute, MAX_LINK_HOPS) {", "if false && self.link_leads_to_network(&absolute, MAX_LINK_HOPS) {")],
     "lightcraft-engine", ["a_link_to_another_computer_is_refused_without_following_it"], True),
    ("an availability probe run with no roots", ENGINE + "availability.rs",
     [("let ok = scope.run(|| probe(&path));", "let _ = &scope;\n                let ok = probe(&path);")],
     "lightcraft-engine", ["the_worker_probes_under_the_roots_of_the_thread_that_asked"], False),
    ("merge preview workers reading with no roots", ENGINE + "merge.rs",
     [("let bytes = roots.run(|| (self.read)(path))?;", "let _ = &roots;\n            let bytes = (self.read)(path)?;")],
     "lightcraft-engine", ["preview_workers_read_under_the_roots_of_the_caller"], False),
    ("a cached logo handed to a confined read", ENGINE + "export/watermark.rs",
     [("if crate::file_access::current().is_none()\n        && let Some((m, img)) = map.get(path)", "if let Some((m, img)) = map.get(path)")],
     "lightcraft-engine", ["a_confined_logo_read_does_not_take_a_decode_another_read_cached"], False),
    ("a profile elsewhere read while the library opens", ENGINE + "cmd/lut_profiles.rs",
     [("crate::file_access::check_read_process(file).is_ok()", "crate::file_access::check_read(file).is_ok()")],
     "lightcraft-engine::operator_library_roots", ["a_profile_elsewhere_is_judged_by_the_roots_while_the_library_opens"], False),
    ("the sync work run with no roots", ENGINE + "sync.rs",
     [("scope.run(move || self.run_in_scope(progress, send));", "let _ = &scope;\n        self.run_in_scope(progress, send);")],
     "lightcraft-engine", ["the_work_runs_under_the_roots_of_whoever_started_it"], False),
    ("a handoff that hands over nothing", ENGINE + "file_access.rs",
     [("        confine(self.0.as_ref(), f)\n    }\n}\n\n/// The roots in force on this thread now", "        f()\n    }\n}\n\n/// The roots in force on this thread now")],
     "lightcraft-engine", ["a_handoff_puts_the_roots_in_force_on_another_thread"], False),
    ("export lanes reading with no roots", ENGINE + "export/batch.rs",
     [("move || lane_roots.run(|| q.work())", "move || {\n                let _ = &lane_roots;\n                q.work()\n            }")],
     "lightcraft-engine", ["export_lanes_read_under_the_roots_of_the_caller"], False),
    ("a link taken for a placed second name", ENGINE + "file_access.rs",
     [("            for name in [path, placed] {\n                if std::fs::symlink_metadata(name).is_ok_and(|m| m.file_type().is_symlink()) {", "            for name in [path, placed] {\n                if false && std::fs::symlink_metadata(name).is_ok_and(|m| m.file_type().is_symlink()) {")],
     "lightcraft-engine", ["a_link_to_the_source_is_not_its_placed_second_name"], False),
    ("a link chain past the budget trusted on Windows", ENGINE + "file_access.rs",
     [("            if hops == 0 {\n                return true;\n            }", "            if hops == 0 {\n                return false;\n            }")],
     "lightcraft-engine", ["a_chain_of_links_longer_than_the_budget_is_refused"], True),
    ("an availability answer handed to another asker", ENGINE + "availability.rs",
     [("if st.known_for != asker {", "if false && st.known_for != asker {")],
     "lightcraft-engine", ["an_answer_is_not_handed_to_an_asker_with_other_roots"], False),
    ("a sidecar name asked about outside the roots", ENGINE + "sidecar.rs",
     [("            .filter(|p| crate::file_access::may_probe(p))", "            .filter(|_| true)")],
     "lightcraft-engine", ["a_sidecar_name_outside_the_roots_is_not_a_candidate"], False),
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
    package, _, target = package.partition("::")
    code, out = run(["cargo", "test", "--locked", "-p", package, *(["--test", target] if target else ["--lib"]), "--", *tests])
    run(["git", "checkout", "--", path])
    survived = [t for t in tests if f"{t} ... FAILED" not in out]
    verdict = "ok, its tests fail" if not survived else f"STILL PASSES: {survived}"
    print(f"{name}: {verdict}", flush=True)
    if survived:
        print(out[-3000:])
        failed_to_fail.append(name)
sys.exit(1 if failed_to_fail else 0)
