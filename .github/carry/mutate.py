"""Undo the case and junction fixes on this checkout and confirm that their tests fail."""
import re
import subprocess
import sys

F = "crates/engine/src/file_access.rs"
s = open(F, encoding="utf-8").read()

exact = "root.components().all(|r| path.next() == Some(r))"
assert s.count(exact) == 1
s = s.replace(
    exact,
    "root.components().all(|r| path.next().is_some_and(|p| p == r || "
    "matches!((p.as_os_str().to_str(), r.as_os_str().to_str()), "
    "(Some(a), Some(b)) if a.to_lowercase() == b.to_lowercase())))",
)
s, links = re.subn(r"if allowed\s*&& access == Access::Write\s*&& let Some\(folder\)", "if false && let Some(folder)", s)
assert links == 1
open(F, "w", encoding="utf-8").write(s)

r = subprocess.run(
    "cargo test --locked -p lightcraft-engine --lib file_access -- --nocapture --test-threads=1",
    shell=True, capture_output=True, text=True, encoding="utf-8", errors="replace",
)
out = r.stdout + r.stderr
print(out[-6000:])
want = ["names_are_compared_exactly", "a_junction_is_written_where_it_stands"]
missing = [t for t in want if f"{t} ... FAILED" not in out and f"file_access::tests::{t} ... FAILED" not in out]
print("expected to fail without the fixes:", want)
print("did not fail:", missing)
sys.exit(1 if missing or r.returncode == 0 else 0)
