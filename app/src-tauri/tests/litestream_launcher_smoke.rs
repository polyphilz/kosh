#![cfg(feature = "test-support")]

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

const LAUNCHER_ARG: &str = "--kosh-litestream-launcher";
const ACTIVATION_TOKEN: &[u8] = b"kosh-litestream-activate-v1\n";
const ACCESS_KEY_ENV: &str = "KOSH_LITESTREAM_R2_ACCESS_KEY_ID";
const SECRET_KEY_ENV: &str = "KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY";

#[cfg(unix)]
fn fake_litestream(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let binary = root.join("fake-litestream");
    let config = root.join("ls.yml");
    let marker = root.join("ls.yml.started");
    fs::write(
        &binary,
        b"#!/bin/sh\n[ \"$KOSH_LITESTREAM_R2_ACCESS_KEY_ID\" = test-access-key ] || exit 65\n[ \"$KOSH_LITESTREAM_R2_SECRET_ACCESS_KEY\" = test-secret-key ] || exit 65\nprintf '%s\\n%s\\n' \"$1\" \"$2\" > \"$3.started\"\n",
    )
    .expect("fake Litestream");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
        .expect("fake Litestream permissions");
    fs::write(&config, b"safe config").expect("fake Litestream config");
    (binary, config, marker)
}

fn spawn_launcher(binary: &Path, config: &Path) -> Child {
    let bytes = fs::read(binary).expect("fake Litestream bytes");
    Command::new(env!("CARGO_BIN_EXE_kosh"))
        .arg(LAUNCHER_ARG)
        .arg(binary)
        .arg(format!("{:x}", Sha256::digest(&bytes)))
        .arg(bytes.len().to_string())
        .arg("replicate")
        .arg("-config")
        .arg(config)
        .env_clear()
        .env(ACCESS_KEY_ENV, "test-access-key")
        .env(SECRET_KEY_ENV, "test-secret-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Kosh Litestream launcher")
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("launcher status") {
            return status;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("launcher did not exit before timeout");
}

#[cfg(unix)]
#[test]
fn actual_kosh_launcher_waits_for_durable_activation_before_exec() {
    let root = tempfile::tempdir().expect("temporary launcher root");
    let (binary, config, marker) = fake_litestream(root.path());
    let mut child = spawn_launcher(&binary, &config);

    thread::sleep(Duration::from_millis(100));
    assert!(
        !marker.exists(),
        "Litestream executed before ownership activation"
    );
    let mut activation = child.stdin.take().expect("launcher activation pipe");
    activation
        .write_all(ACTIVATION_TOKEN)
        .expect("activate Litestream");
    drop(activation);

    assert!(wait_for_exit(&mut child).success());
    assert_eq!(
        fs::read_to_string(marker).expect("fake Litestream evidence"),
        "replicate\n-config\n"
    );
}

#[cfg(unix)]
#[test]
fn actual_kosh_launcher_exits_on_parent_pipe_eof_without_exec() {
    let root = tempfile::tempdir().expect("temporary launcher root");
    let (binary, config, marker) = fake_litestream(root.path());
    let mut child = spawn_launcher(&binary, &config);

    drop(child.stdin.take());

    assert_eq!(wait_for_exit(&mut child).code(), Some(74));
    assert!(
        !marker.exists(),
        "Litestream executed after pre-activation parent death"
    );
}

#[cfg(unix)]
#[test]
fn actual_kosh_launcher_rejects_a_path_swap_before_activation() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("temporary launcher root");
    let (binary, config, marker) = fake_litestream(root.path());
    let mut child = spawn_launcher(&binary, &config);
    let replacement = root.path().join("replacement-litestream");
    let replacement_marker = root.path().join("replacement.started");
    fs::write(
        &replacement,
        format!(
            "#!/bin/sh\nprintf substituted > '{}'\n",
            replacement_marker.display()
        ),
    )
    .expect("replacement Litestream");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700))
        .expect("replacement permissions");
    fs::rename(&replacement, &binary).expect("swap Litestream pathname");

    let mut activation = child.stdin.take().expect("launcher activation pipe");
    activation
        .write_all(ACTIVATION_TOKEN)
        .expect("activate verified Litestream");
    drop(activation);

    assert_eq!(wait_for_exit(&mut child).code(), Some(74));
    assert!(
        !marker.exists(),
        "a replaced path cannot execute the formerly verified bytes"
    );
    assert!(
        !replacement_marker.exists(),
        "the substituted pathname must never execute"
    );
}
