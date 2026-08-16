//! End-to-end integration tests that spawn the real `cursdel` binary
//! against real temporary directory trees. Unit tests inside each crate
//! cover the internals in isolation; these tests exist to catch anything
//! that only breaks when the pieces are wired together as a real process
//! (argument parsing, exit codes as actually observed by a caller,
//! process-level Ctrl+C handling).
//!
//! Every test operates inside its own `tempfile::tempdir()`, never a real
//! filesystem location, per the product's destructive-testing safety
//! rule.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;

fn cursdel() -> Command {
    Command::cargo_bin("cursdel").expect("cursdel binary should build")
}

fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn deletes_a_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("solo.txt");
    write_file(&file, b"x");

    cursdel().arg(&file).assert().success();
    assert!(!file.exists());
}

#[test]
fn deletes_an_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("empty");
    fs::create_dir(&target).unwrap();

    cursdel().arg(&target).assert().success();
    assert!(!target.exists());
}

#[test]
fn deletes_a_nested_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"a");
    write_file(&root.join("sub/b.txt"), b"bb");
    write_file(&root.join("sub/deeper/c.txt"), b"ccc");
    fs::create_dir_all(root.join("sub/empty")).unwrap();

    cursdel()
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Files:          3"))
        .stdout(predicate::str::contains("Failures:       0"));
    assert!(!root.exists());
}

#[test]
fn deletes_a_deep_tree_without_stack_overflow() {
    // Depth is deliberately kept well under the host OS's PATH_MAX (macOS:
    // ~1024 bytes total path length) -- that is an OS limit unrelated to
    // what this test proves, which is that cursdel_core::walk's explicit-
    // stack traversal doesn't recurse (and therefore can't stack-overflow)
    // as depth grows. 120 levels is already far deeper than any recursive
    // implementation would tolerate without a dedicated large-stack
    // thread, while safely fitting under typical path-length limits.
    let dir = tempfile::tempdir().unwrap();
    let mut path = dir.path().join("deep");
    for i in 0..120 {
        path = path.join(format!("d{i}"));
    }
    write_file(&path.join("leaf.txt"), b"leaf");
    let root = dir.path().join("deep");

    cursdel().arg(&root).assert().success();
    assert!(!root.exists());
}

#[test]
fn deletes_a_wide_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wide");
    for i in 0..500 {
        write_file(&root.join(format!("file{i}.txt")), b"x");
    }

    cursdel()
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Files:          500"));
    assert!(!root.exists());
}

#[test]
fn handles_unicode_filenames() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("unicode");
    write_file(&root.join("caf\u{e9}.txt"), b"x");
    write_file(&root.join("\u{1f600}emoji.txt"), b"x");
    write_file(&root.join("\u{4e2d}\u{6587}.txt"), b"x"); // Chinese characters

    cursdel()
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Files:          3"));
    assert!(!root.exists());
}

#[test]
fn dry_run_does_not_modify_anything() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"a");
    write_file(&root.join("sub/b.txt"), b"bb");

    cursdel()
        .arg(&root)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("No files were modified."));

    assert!(root.join("a.txt").exists());
    assert!(root.join("sub/b.txt").exists());
}

#[test]
fn json_output_has_expected_schema_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"a");

    let output = cursdel()
        .arg(&root)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["exitCode"], 0);
    assert_eq!(value["metrics"]["filesDeleted"], 1);
    assert_eq!(value["dryRun"], false);
    assert!(value["failures"].as_array().unwrap().is_empty());
}

#[test]
fn exit_code_is_one_for_filesystem_root() {
    cursdel()
        .arg("/")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("filesystem root"));
}

#[test]
fn exit_code_is_usage_error_when_target_missing() {
    cursdel().assert().failure().code(64);
}

/// clap's own parse-error handling defaults to exit code 2 for any usage
/// error, which would collide with this product's own frozen, documented
/// meaning of exit code 2 ("completed with one or more deletion
/// failures"). A script checking `$? -eq 2` to mean "some files failed to
/// delete" must never be misled by an unrelated CLI usage error --
/// regression coverage for every clap-level error path, not just this
/// crate's own "no target given" case above.
#[test]
fn clap_level_usage_errors_use_cli_usage_error_code_not_claps_default() {
    cursdel()
        .arg("--this-flag-does-not-exist")
        .assert()
        .failure()
        .code(64);
}

#[test]
fn missing_required_license_subcommand_is_a_usage_error_not_success() {
    // `cursdel license` with no action given is, in this product's own
    // design, exactly as much a usage error as bare `cursdel` with no
    // target -- both must be treated consistently, even though clap
    // itself classifies a missing-subcommand error as "display help"
    // (which would otherwise map to exit 0).
    cursdel().arg("license").assert().failure().code(64);
}

#[test]
fn help_and_version_exit_zero_despite_using_claps_error_path() {
    cursdel().arg("--help").assert().success();
    cursdel().arg("--version").assert().success();
    cursdel().args(["license", "--help"]).assert().success();
}

#[test]
fn include_and_exclude_filters_are_respected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.log"), b"x");
    write_file(&root.join("b.keep.log"), b"x");
    write_file(&root.join("c.txt"), b"x");

    cursdel()
        .arg(&root)
        .arg("--include")
        .arg("*.log")
        .arg("--exclude")
        .arg("*.keep.log")
        .assert()
        .success();

    assert!(!root.join("a.log").exists(), "a.log should be deleted");
    assert!(
        root.join("b.keep.log").exists(),
        "b.keep.log should be excluded"
    );
    assert!(
        root.join("c.txt").exists(),
        "c.txt should not match --include"
    );
}

#[test]
fn min_and_max_size_filters_are_respected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("tiny.bin"), &[0u8; 10]);
    write_file(&root.join("mid.bin"), &[0u8; 500]);
    write_file(&root.join("huge.bin"), &[0u8; 5000]);

    cursdel()
        .arg(&root)
        .arg("--min-size")
        .arg("100")
        .arg("--max-size")
        .arg("1000")
        .assert()
        .success();

    assert!(
        root.join("tiny.bin").exists(),
        "below min-size should be retained"
    );
    assert!(
        !root.join("mid.bin").exists(),
        "in-range file should be deleted"
    );
    assert!(
        root.join("huge.bin").exists(),
        "above max-size should be retained"
    );
}

#[test]
fn age_rejects_bare_number_as_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"x");

    cursdel()
        .arg(&root)
        .arg("--age")
        .arg("2")
        .assert()
        .failure()
        .code(64)
        .stderr(predicate::str::contains("unit"));
}

#[test]
fn age_retention_preserves_root_and_newer_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    let old_file = root.join("old.log");
    let new_file = root.join("new.log");
    write_file(&old_file, b"old");
    write_file(&new_file, b"new");

    // Backdate old.log well past the threshold using the `touch` utility,
    // which is simpler and more portable across test environments than
    // hand-rolling a filetime-setting syscall wrapper for a test.
    let status = StdCommand::new("touch")
        .arg("-t")
        .arg("202001010000")
        .arg(&old_file)
        .status()
        .unwrap();
    assert!(status.success());

    cursdel()
        .arg(&root)
        .arg("--age")
        .arg("1d")
        .assert()
        .success();

    assert!(!old_file.exists(), "old file should be deleted");
    assert!(new_file.exists(), "new file should be retained");
    assert!(
        root.exists(),
        "target root must be preserved in retention mode"
    );
}

#[test]
fn age_retention_removes_directories_that_become_empty() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    let ancient = root.join("ancient/old.log");
    write_file(&ancient, b"old");
    let status = StdCommand::new("touch")
        .arg("-t")
        .arg("202001010000")
        .arg(&ancient)
        .status()
        .unwrap();
    assert!(status.success());

    cursdel()
        .arg(&root)
        .arg("--age")
        .arg("1d")
        .assert()
        .success();

    assert!(
        !root.join("ancient").exists(),
        "ancient/ became empty and should be removed"
    );
    assert!(root.exists(), "root itself must still be preserved");
}

#[test]
fn manual_workers_flag_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    for i in 0..20 {
        write_file(&root.join(format!("f{i}.txt")), b"x");
    }

    cursdel()
        .arg(&root)
        .arg("--workers")
        .arg("4")
        .assert()
        .success()
        .stdout(predicate::str::contains("Workers:      4"));
}

#[test]
fn rejects_zero_workers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"x");

    cursdel()
        .arg(&root)
        .arg("--workers")
        .arg("0")
        .assert()
        .failure()
        .code(64);
}

#[cfg(unix)]
#[test]
fn symlink_target_outside_tree_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    write_file(&outside.join("keep-me.txt"), b"important");

    let root = dir.path().join("tree");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

    cursdel().arg(&root).assert().success();

    assert!(
        !root.exists(),
        "the tree (including the link itself) should be gone"
    );
    assert!(outside.exists(), "the link target must never be touched");
    assert!(outside.join("keep-me.txt").exists());
}

#[cfg(unix)]
#[test]
fn force_mode_recovers_from_permission_denied_parent_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("locked");
    let file = root.join("f.txt");
    write_file(&file, b"x");

    // Remove write permission on the parent directory: deletion on POSIX
    // is governed by the parent's write bit, so this makes a normal
    // delete fail with EACCES.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();

    // Sanity check: without --force, this must actually fail.
    let normal_result = cursdel().arg(&file).assert();
    // Restore permissions immediately regardless of outcome so cleanup
    // doesn't leak a non-writable directory.
    let restore = fs::set_permissions(&root, fs::Permissions::from_mode(0o755));

    normal_result.failure();
    restore.unwrap();

    // Re-lock and retry with --force, which should remediate and succeed.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    let force_result = cursdel().arg(&file).arg("--force").assert();
    let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o755));
    force_result.success();
    assert!(!file.exists());
}

#[test]
fn quiet_mode_suppresses_summary_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"x");

    cursdel()
        .arg(&root)
        .arg("--quiet")
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn log_flag_writes_report_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"x");
    let log_path = dir.path().join("report.log");

    cursdel()
        .arg(&root)
        .arg("--log")
        .arg(&log_path)
        .assert()
        .success();

    let logged = fs::read_to_string(&log_path).unwrap();
    assert!(logged.contains("Complete."));
}

#[test]
fn close_remote_locks_without_license_is_rejected_before_touching_anything() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    write_file(&root.join("a.txt"), b"x");

    // No license is activated in this test's isolated HOME/config, so this
    // must be rejected as a license requirement rather than attempted.
    cursdel()
        .arg(&root)
        .arg("--close-remote-locks")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .assert()
        .failure()
        .code(7)
        .stderr(predicate::str::contains("Business or Enterprise"));

    assert!(
        root.join("a.txt").exists(),
        "must not have touched the target"
    );
}

#[test]
fn help_lists_all_documented_flags() {
    cursdel()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--force"))
        .stdout(predicate::str::contains("--destroy"))
        .stdout(predicate::str::contains("--workers"))
        .stdout(predicate::str::contains("--age"))
        .stdout(predicate::str::contains("--kill-locks"))
        .stdout(predicate::str::contains("--close-remote-locks"))
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--json"));
}

#[test]
fn license_status_reports_community_when_unlicensed() {
    let dir = tempfile::tempdir().unwrap();
    cursdel()
        .arg("license")
        .arg("status")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".config"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Community capabilities"));
}

/// Interrupting a large operation partway through must exit with the
/// documented Interrupted code (6) and never hang. The tree is sized
/// generously so the operation cannot plausibly complete in the brief
/// window before SIGINT is delivered, on any reasonable machine.
#[cfg(unix)]
#[test]
fn interrupted_operation_reports_interrupted_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("big");
    fs::create_dir(&root).unwrap();
    for i in 0..20_000 {
        write_file(&root.join(format!("f{i}.txt")), b"x");
    }

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("cursdel"))
        .arg(&root)
        .spawn()
        .expect("cursdel should spawn");

    std::thread::sleep(std::time::Duration::from_millis(30));
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    let status = child.wait().expect("cursdel should exit after SIGINT");
    assert_eq!(
        status.code(),
        Some(6),
        "expected the documented Interrupted exit code"
    );
}
