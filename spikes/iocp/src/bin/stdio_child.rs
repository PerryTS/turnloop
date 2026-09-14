#![deny(unsafe_op_in_unsafe_fn)]
use std::io::{Read, Write};
fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--tree") {
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .arg("--linger")
            .spawn()?;
        std::io::stdout().write_all(&child.id().to_le_bytes())?;
        std::io::stdout().flush()?;
        // Job Object test terminates both processes. Reap if child exits unexpectedly.
        child.wait()?;
        return Err(std::io::Error::other(
            "grandchild exited before job termination",
        ));
    }
    if std::env::args().nth(1).as_deref() == Some("--linger") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    let mut data = [0u8; 8];
    std::io::stdin().read_exact(&mut data)?;
    if &data != b"windlass" {
        return Err(std::io::Error::other("wrong child input"));
    }
    std::io::stdout().write_all(b"child-out:windlass\n")?;
    std::io::stdout().flush()?;
    std::io::stderr().write_all(b"child-err")?;
    std::io::stderr().flush()?;
    std::process::exit(23);
}
