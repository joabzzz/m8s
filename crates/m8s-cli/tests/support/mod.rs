//! Retained owned CLI child with bounded regular-file output capture.
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
pub(super) struct Client(Child, File, File);
impl Client {
    pub(super) fn spawn(args: &[OsString], broken: bool) -> Self {
        let out = tempfile::tempfile().unwrap();
        let err = tempfile::tempfile().unwrap();
        let stdout = if broken {
            let (reader, writer) = UnixStream::pair().unwrap();
            drop(reader);
            let fd: OwnedFd = writer.into();
            Stdio::from(fd)
        } else {
            Stdio::from(out.try_clone().unwrap())
        };
        Self(
            Command::new(env!("CARGO_BIN_EXE_m8s"))
                .args(args)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(err.try_clone().unwrap())
                .spawn()
                .unwrap(),
            out,
            err,
        )
    }
    pub(super) fn output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "owned CLI deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        self.1.seek(SeekFrom::Start(0)).unwrap();
        self.2.seek(SeekFrom::Start(0)).unwrap();
        let (mut stdout, mut stderr) = (vec![], vec![]);
        (&self.1).take(65537).read_to_end(&mut stdout).unwrap();
        (&self.2).take(4096).read_to_end(&mut stderr).unwrap();
        assert!(stdout.len() <= 65536 && stderr.len() < 4096);
        Output {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
