//! Verifies the pinned scrcpy-server before every build and passes its
//! version and path to the crate, so the client can never be built against
//! a server it was not written for.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lock_path = root.join("scrcpy-server.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", lock_path.display())));
    let field = |key: &str| {
        lock.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
            .unwrap_or_else(|| fail(&format!("`{key}=` missing in scrcpy-server.lock")))
            .to_owned()
    };
    let version = field("version");
    let expected = field("sha256");

    let server = root.join("assets").join(format!("scrcpy-server-v{version}"));
    println!("cargo:rerun-if-changed={}", server.display());
    if !server.is_file() {
        fail(&format!(
            "{} not found; run ./scripts/fetch-server.sh",
            server.display()
        ));
    }

    let actual = sha256(&server);
    if actual != expected {
        fail(&format!(
            "checksum mismatch for {}\n  expected {expected}\n  actual   {actual}\n\
             delete it and run ./scripts/fetch-server.sh",
            server.display()
        ));
    }

    println!("cargo:rustc-env=SCRCPY_SERVER_VERSION={version}");
    println!("cargo:rustc-env=SCRCPY_SERVER_PATH={}", server.display());
}

/// Hashes with the system tool (`shasum` ships with macOS) to avoid a
/// build dependency.
fn sha256(path: &std::path::Path) -> String {
    let attempts: [(&str, &[&str]); 2] = [("shasum", &["-a", "256"]), ("sha256sum", &[])];
    for (tool, args) in attempts {
        if let Ok(out) = Command::new(tool).args(args).arg(path).output()
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = text.split_whitespace().next() {
                return hash.to_ascii_lowercase();
            }
        }
    }
    fail("neither `shasum` nor `sha256sum` is available to verify the server")
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}
