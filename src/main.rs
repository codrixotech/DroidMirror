//! DroidMirror: mirror and control an Android device from macOS, using the
//! official scrcpy-server running on the device.

mod logger;

/// scrcpy-server version this client speaks, from `scrcpy-server.lock`
/// (verified by build.rs). The server refuses to start if the version
/// passed on its command line differs from its own.
pub const SERVER_VERSION: &str = env!("SCRCPY_SERVER_VERSION");

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "-V" | "--version" => {
                println!(
                    "droidmirror {} (scrcpy-server {SERVER_VERSION})",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            "-h" | "--help" => {
                println!("usage: droidmirror [--version]\n\nset DROIDMIRROR_LOG=debug for verbose logs");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}` (try --help)"),
        }
    }

    logger::init();
    log::info!(
        "droidmirror {} using scrcpy-server {SERVER_VERSION}",
        env!("CARGO_PKG_VERSION")
    );
    log::info!("device connection arrives in milestone 2");
    Ok(())
}
