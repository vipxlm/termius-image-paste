//! Exercise the shipped helper process without opening Tauri or touching an installed client.
#![cfg(windows)]
use sha2::{Digest, Sha256};
use std::os::windows::process::CommandExt;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}
fn hidden(path: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(path);
    c.creation_flags(0x08000000);
    c
}
#[test]
fn actual_helper_waits_replaces_restarts_and_rejects_tampering() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("更新 空格 ' 路径");
    fs::create_dir(&root).unwrap();
    let marker = root.join("restarted.txt");
    let source = root.join("fixture.rs");
    let candidate = root.join("candidate.exe");
    fs::write(
        &source,
        format!(
            "fn main() {{ std::fs::write({:?}, b\"updated\").unwrap(); }}",
            marker.to_str().unwrap()
        ),
    )
    .unwrap();
    assert!(hidden("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&candidate)
        .output()
        .unwrap()
        .status
        .success());
    let original = Path::new(env!("CARGO_BIN_EXE_termius-image-paste"));
    let id = "a".repeat(32);
    let target = root.join("客户端 ' 空格.exe");
    let staged = root.join(format!(".termius-update-{id}.bin"));
    let helper = root.join(format!(".termius-helper-{id}.exe"));
    let backup = root.join(format!(".termius-backup-{id}.exe"));
    let plan_path = target.with_extension("termius-update.json");
    fs::copy(original, &target).unwrap();
    fs::copy(original, &helper).unwrap();
    fs::copy(&candidate, &staged).unwrap();
    let old_hash = hash(&target);
    let new_hash = hash(&staged);
    let mut parent = hidden("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Milliseconds 1500",
        ])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut plan = serde_json::json!({"target":target,"staged":staged,"backup":backup,"helper":helper,"old_hash":old_hash,"new_hash":new_hash,"version":"3.0.0","owner_pid":parent.id(),"phase":"ready","failure":null});
    fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let mut child = hidden(&helper)
        .arg("--apply-update")
        .arg(&plan_path)
        .arg("restart")
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        hash(&target),
        old_hash,
        "must not replace while parent is running"
    );
    parent.wait().unwrap();
    assert!(child.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(fs::read(&marker).unwrap(), b"updated");
    assert_eq!(hash(&target), new_hash);
    assert_eq!(hash(&backup), old_hash);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&plan_path).unwrap()).unwrap()
            ["phase"],
        "applied"
    );
    // Tampered downloads cannot modify the original or overwrite its existing backup.
    let target2 = root.join("protected.exe");
    let plan2 = target2.with_extension("termius-update.json");
    fs::copy(original, &target2).unwrap();
    fs::write(&staged, b"tampered").unwrap();
    plan["target"] = serde_json::to_value(&target2).unwrap();
    plan["owner_pid"] = u32::MAX.into();
    fs::write(&plan2, serde_json::to_vec(&plan).unwrap()).unwrap();
    assert!(!hidden(&helper)
        .arg("--apply-update")
        .arg(&plan2)
        .arg("exit")
        .status()
        .unwrap()
        .success());
    assert_eq!(hash(&target2), old_hash);
    assert_eq!(hash(&backup), old_hash);
}
