use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let repository = manifest_dir.join("../..");
    watch_git_identity(&repository);
    println!(
        "cargo:rustc-env=KOSH_BUILD_GIT_SHA={}",
        build_git_sha(&repository)
    );
    tauri_build::build()
}

fn build_git_sha(repository: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository)
        .output()
        .expect("git is required to bind Kosh builds to their source commit");
    assert!(
        output.status.success(),
        "git rev-parse HEAD failed while binding the Kosh build"
    );
    validated_sha(String::from_utf8_lossy(&output.stdout).trim())
}

fn validated_sha(value: &str) -> String {
    assert!(
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Kosh build commit must be a full lowercase Git SHA"
    );
    value.to_owned()
}

fn watch_git_identity(repository: &Path) {
    let git_directory = git_path(repository, &["rev-parse", "--absolute-git-dir"]);
    let common_directory = git_path(repository, &["rev-parse", "--git-common-dir"]);
    let head = git_directory.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    println!(
        "cargo:rerun-if-changed={}",
        common_directory.join("packed-refs").display()
    );
    let Ok(contents) = fs::read_to_string(head) else {
        return;
    };
    let Some(reference) = contents.trim().strip_prefix("ref: ") else {
        return;
    };
    assert!(
        !reference.contains("..") && !reference.starts_with('/'),
        "Git HEAD contained an unsafe reference"
    );
    println!(
        "cargo:rerun-if-changed={}",
        common_directory.join(reference).display()
    );
}

fn git_path(repository: &Path, arguments: &[&str]) -> PathBuf {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("git is required to locate Kosh build metadata");
    assert!(
        output.status.success(),
        "git metadata lookup failed while binding the Kosh build"
    );
    let value = String::from_utf8_lossy(&output.stdout);
    let path = PathBuf::from(value.trim());
    if path.is_absolute() {
        path
    } else {
        repository.join(path)
    }
}
