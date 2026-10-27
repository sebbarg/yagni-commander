//! A blocking SFTP client (protocol version 3, the one OpenSSH speaks) over
//! any byte stream: in the app the standard input and output of
//! `ssh -s host sftp` (see [`crate::remote`]), in tests a local
//! `sftp-server`. One request at a time, except [`Client::download`], which
//! keeps several reads in flight.
//!
//! Paths are bytes, as on the wire: a remote name need not be UTF-8.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt;

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_LSTAT: u8 = 7;
const FXP_OPENDIR: u8 = 11;
const FXP_READDIR: u8 = 12;
const FXP_REALPATH: u8 = 16;
const FXP_STAT: u8 = 17;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_NAME: u8 = 104;
const FXP_ATTRS: u8 = 105;

const FX_EOF: u32 = 1;
const FX_NO_SUCH_FILE: u32 = 2;
const FX_PERMISSION_DENIED: u32 = 3;

const ATTR_SIZE: u32 = 0x1;
const ATTR_UIDGID: u32 = 0x2;
const ATTR_PERMISSIONS: u32 = 0x4;
const ATTR_ACMODTIME: u32 = 0x8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

const OPEN_READ: u32 = 0x1;

/// Larger packets are refused: OpenSSH sends at most 256 KiB, so anything
/// much bigger is a broken or hostile server.
const MAX_PACKET: u32 = 1 << 20;
/// Bytes asked for by each read of a download. OpenSSH's own client uses
/// 32 KiB; every server must accept it.
const READ_SIZE: u32 = 32 * 1024;
/// Reads of a download in flight at once (1 MiB), so a long round trip
/// doesn't stall the transfer.
const IN_FLIGHT: usize = 32;

/// A file's attributes; a server may leave any of them out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid_gid: Option<(u32, u32)>,
    /// `st_mode`: the file type bits and the permissions.
    pub permissions: Option<u32>,
    /// Seconds since the epoch.
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.permissions
            .is_some_and(|mode| mode & 0o170_000 == 0o040_000)
    }

    pub fn is_symlink(&self) -> bool {
        self.permissions
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
    }

    pub fn is_file(&self) -> bool {
        self.permissions
            .is_some_and(|mode| mode & 0o170_000 == 0o100_000)
    }
}

/// One entry of a folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub name: Vec<u8>,
    /// The server's `ls -l` line for the entry (OpenSSH: owner and group
    /// by name).
    pub long_name: Vec<u8>,
    pub attrs: Attrs,
}

/// An open file or folder on the server.
#[derive(Debug, PartialEq, Eq)]
pub struct Handle(Vec<u8>);

pub struct Client {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    next_id: u32,
    /// Set once the stream failed or went out of step: nothing more can be
    /// sent on it.
    broken: bool,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("broken", &self.broken)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Starts a session on the stream: sends `INIT` and reads the
    /// server's `VERSION`.
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> io::Result<Self> {
        let mut client = Self {
            reader,
            writer,
            next_id: 0,
            broken: false,
        };
        let mut init = Packet::new(FXP_INIT);
        init.u32(3);
        client.send(init)?;
        let (kind, mut body) = client.receive()?;
        if kind != FXP_VERSION {
            client.broken = true;
            return Err(protocol("the server did not start an SFTP session"));
        }
        let version = body.u32()?;
        if version < 3 {
            client.broken = true;
            return Err(protocol(&format!(
                "the server speaks SFTP version {version}, version 3 is needed"
            )));
        }
        Ok(client)
    }

    /// False once the stream failed: the session is over.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// The absolute, canonical form of `path` (`.` is the login folder).
    pub fn realpath(&mut self, path: &[u8]) -> io::Result<Vec<u8>> {
        let mut body = self
            .request(FXP_REALPATH, |p| p.bytes(path))?
            .expect(FXP_NAME)?;
        if body.u32()? == 0 {
            return Err(protocol("the server gave no name"));
        }
        body.bytes()
    }

    /// The attributes of `path`, following a symlink.
    pub fn stat(&mut self, path: &[u8]) -> io::Result<Attrs> {
        self.request(FXP_STAT, |p| p.bytes(path))?
            .expect(FXP_ATTRS)?
            .attrs()
    }

    /// The attributes of `path` itself, not a symlink's target.
    pub fn lstat(&mut self, path: &[u8]) -> io::Result<Attrs> {
        self.request(FXP_LSTAT, |p| p.bytes(path))?
            .expect(FXP_ATTRS)?
            .attrs()
    }

    /// Calls `each` with every entry of folder `path`, `.` and `..`
    /// included, as the server sends them.
    pub fn read_dir(&mut self, path: &[u8], mut each: impl FnMut(Name)) -> io::Result<()> {
        let handle = self.open_dir(path)?;
        let result = (|| loop {
            let reply = self.request(FXP_READDIR, |p| p.bytes(&handle.0))?;
            if reply.is_eof() {
                return Ok(());
            }
            let mut body = reply.expect(FXP_NAME)?;
            for _ in 0..body.u32()? {
                let name = body.bytes()?;
                let long_name = body.bytes()?;
                let attrs = body.attrs()?;
                each(Name {
                    name,
                    long_name,
                    attrs,
                });
            }
        })();
        let closed = self.close(handle);
        result.and(closed)
    }

    fn open_dir(&mut self, path: &[u8]) -> io::Result<Handle> {
        let mut body = self
            .request(FXP_OPENDIR, |p| p.bytes(path))?
            .expect(FXP_HANDLE)?;
        Ok(Handle(body.bytes()?))
    }

    /// Opens file `path` for reading.
    pub fn open_read(&mut self, path: &[u8]) -> io::Result<Handle> {
        let mut body = self
            .request(FXP_OPEN, |p| {
                p.bytes(path);
                p.u32(OPEN_READ);
                p.u32(0); // no attributes
            })?
            .expect(FXP_HANDLE)?;
        Ok(Handle(body.bytes()?))
    }

    pub fn close(&mut self, handle: Handle) -> io::Result<()> {
        self.request(FXP_CLOSE, |p| p.bytes(&handle.0))?.expect_ok()
    }

    /// Copies the open file into `out` from its start, with several reads
    /// in flight. `progress` gets the bytes of each piece written and
    /// returns false to stop: then the result is `Ok(false)`, and `out`
    /// holds part of the file.
    pub fn download(
        &mut self,
        handle: &Handle,
        out: &File,
        mut progress: impl FnMut(u64) -> bool,
    ) -> io::Result<bool> {
        // Reads in flight, by request id: offset and length asked for.
        let mut pending: HashMap<u32, (u64, u32)> = HashMap::new();
        // Where the next new read starts.
        let mut next = 0u64;
        let mut at_end = false;
        let mut failure = None;
        let mut stopped = false;
        loop {
            while failure.is_none() && !stopped && !at_end && pending.len() < IN_FLIGHT {
                let id = self.send_request(FXP_READ, |p| {
                    p.bytes(&handle.0);
                    p.u64(next);
                    p.u32(READ_SIZE);
                })?;
                pending.insert(id, (next, READ_SIZE));
                next += u64::from(READ_SIZE);
            }
            if pending.is_empty() {
                break;
            }
            // Every reply is read, even after a failure or a stop, so none
            // is left on the stream for the next request.
            let (id, reply) = self.receive_reply()?;
            let Some((offset, asked)) = pending.remove(&id) else {
                self.broken = true;
                return Err(protocol("the server answered a request it was never sent"));
            };
            if failure.is_some() || stopped {
                continue;
            }
            if reply.is_eof() {
                at_end = true;
                continue;
            }
            let data = match reply.expect(FXP_DATA).and_then(|mut body| body.bytes()) {
                Ok(data) => data,
                Err(e) => {
                    failure = Some(e);
                    continue;
                }
            };
            if data.is_empty() {
                at_end = true;
                continue;
            }
            if let Err(e) = out.write_all_at(&data, offset) {
                failure = Some(e);
                continue;
            }
            let got = data.len() as u32;
            if got < asked {
                // A short read: ask again for the rest of the piece.
                let rest = offset + u64::from(got);
                let id = self.send_request(FXP_READ, |p| {
                    p.bytes(&handle.0);
                    p.u64(rest);
                    p.u32(asked - got);
                })?;
                pending.insert(id, (rest, asked - got));
            }
            if !progress(u64::from(got)) {
                stopped = true;
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(!stopped),
        }
    }

    /// Sends a request and reads its reply.
    fn request(&mut self, kind: u8, fill: impl FnOnce(&mut Packet)) -> io::Result<Reply> {
        let id = self.send_request(kind, fill)?;
        let (got, reply) = self.receive_reply()?;
        if got != id {
            self.broken = true;
            return Err(protocol("the server's answer is out of step"));
        }
        Ok(reply)
    }

    /// Sends a request with a new id, which it returns.
    fn send_request(&mut self, kind: u8, fill: impl FnOnce(&mut Packet)) -> io::Result<u32> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut packet = Packet::new(kind);
        packet.u32(id);
        fill(&mut packet);
        self.send(packet)?;
        Ok(id)
    }

    fn receive_reply(&mut self) -> io::Result<(u32, Reply)> {
        let (kind, mut body) = self.receive()?;
        let id = body.u32()?;
        Ok((id, Reply { kind, body }))
    }

    fn send(&mut self, packet: Packet) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the connection is closed",
            ));
        }
        let result = self
            .writer
            .write_all(&packet.finish())
            .and_then(|()| self.writer.flush());
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    fn receive(&mut self) -> io::Result<(u8, Body)> {
        if self.broken {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the connection is closed",
            ));
        }
        let result = (|| {
            let mut len = [0; 4];
            self.reader.read_exact(&mut len)?;
            let len = u32::from_be_bytes(len);
            if len == 0 || len > MAX_PACKET {
                return Err(protocol("the server sent a malformed packet"));
            }
            let mut data = vec![0; len as usize];
            self.reader.read_exact(&mut data)?;
            let kind = data[0];
            Ok((kind, Body { data, at: 1 }))
        })();
        if result.is_err() {
            self.broken = true;
        }
        result
    }
}

/// A reply to one request.
struct Reply {
    kind: u8,
    body: Body,
}

impl Reply {
    fn is_eof(&self) -> bool {
        self.kind == FXP_STATUS && self.body.peek_u32() == Some(FX_EOF)
    }

    /// The body of a reply of `kind`; a status reply becomes its error.
    fn expect(self, kind: u8) -> io::Result<Body> {
        if self.kind == kind {
            return Ok(self.body);
        }
        if self.kind == FXP_STATUS {
            return Err(self.status_error());
        }
        Err(protocol("the server sent an unexpected answer"))
    }

    /// A status reply: `Ok` for `SSH_FX_OK`, else its error.
    fn expect_ok(self) -> io::Result<()> {
        if self.kind != FXP_STATUS {
            return Err(protocol("the server sent an unexpected answer"));
        }
        if self.body.peek_u32() == Some(0) {
            return Ok(());
        }
        Err(self.status_error())
    }

    fn status_error(mut self) -> io::Error {
        let Ok(code) = self.body.u32() else {
            return protocol("the server sent a malformed status");
        };
        let message = self
            .body
            .bytes()
            .map(|m| String::from_utf8_lossy(&m).into_owned())
            .unwrap_or_default();
        let kind = match code {
            FX_NO_SUCH_FILE => io::ErrorKind::NotFound,
            FX_PERMISSION_DENIED => io::ErrorKind::PermissionDenied,
            FX_EOF => io::ErrorKind::UnexpectedEof,
            _ => io::ErrorKind::Other,
        };
        let message = match (kind, message.is_empty()) {
            (io::ErrorKind::NotFound, _) => "No such file or folder".to_owned(),
            (io::ErrorKind::PermissionDenied, _) => "Permission denied".to_owned(),
            (_, true) => format!("the server failed (status {code})"),
            (_, false) => message,
        };
        io::Error::new(kind, message)
    }
}

/// A packet being built: length, type, then the fields.
struct Packet(Vec<u8>);

impl Packet {
    fn new(kind: u8) -> Self {
        Self(vec![0, 0, 0, 0, kind])
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
    }

    fn finish(mut self) -> Vec<u8> {
        let len = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&len.to_be_bytes());
        self.0
    }
}

/// A received packet's fields, read in order.
struct Body {
    data: Vec<u8>,
    at: usize,
}

impl Body {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| protocol("the server sent a truncated packet"))?;
        let field = &self.data[self.at..end];
        self.at = end;
        Ok(field)
    }

    fn peek_u32(&self) -> Option<u32> {
        let field = self.data.get(self.at..self.at + 4)?;
        Some(u32::from_be_bytes(field.try_into().ok()?))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut attrs = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            attrs.uid_gid = Some((self.u32()?, self.u32()?));
        }
        if flags & ATTR_PERMISSIONS != 0 {
            attrs.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            let _atime = self.u32()?;
            attrs.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(attrs)
    }
}

fn protocol(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    /// OpenSSH's SFTP server program, run directly over pipes (no ssh).
    pub(crate) fn server_program() -> PathBuf {
        ["/usr/lib/openssh/sftp-server", "/usr/libexec/sftp-server"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .expect("OpenSSH's sftp-server is needed (Ubuntu: openssh-sftp-server)")
    }

    /// A client talking to a local `sftp-server`.
    pub(crate) fn local_client() -> Client {
        let mut child = Command::new(server_program())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let reader = child.stdout.take().unwrap();
        let writer = child.stdin.take().unwrap();
        // The server ends when its input closes, with the client.
        std::thread::spawn(move || child.wait());
        Client::new(Box::new(reader), Box::new(writer)).unwrap()
    }

    fn bytes(path: &Path) -> &[u8] {
        path.as_os_str().as_bytes()
    }

    #[test]
    fn lists_a_folder_with_attributes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::os::unix::fs::symlink("a.txt", tmp.path().join("link")).unwrap();
        let mut client = local_client();
        let mut names = Vec::new();
        client
            .read_dir(bytes(tmp.path()), |n| names.push(n))
            .unwrap();
        names.sort_by(|a, b| a.name.cmp(&b.name));
        let listed: Vec<_> = names.iter().map(|n| n.name.as_slice()).collect();
        assert_eq!(listed, [&b"."[..], b"..", b"a.txt", b"link", b"sub"]);
        let file = &names[2].attrs;
        assert_eq!(file.size, Some(5));
        assert!(file.is_file() && file.mtime.is_some() && file.uid_gid.is_some());
        assert!(names[3].attrs.is_symlink(), "listed without following");
        assert!(names[4].attrs.is_dir());
        assert!(
            String::from_utf8_lossy(&names[2].long_name).starts_with("-rw"),
            "{:?}",
            String::from_utf8_lossy(&names[2].long_name)
        );
    }

    #[test]
    fn stat_follows_a_link_and_lstat_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("dir")).unwrap();
        std::os::unix::fs::symlink("dir", tmp.path().join("link")).unwrap();
        let mut client = local_client();
        let link = tmp.path().join("link");
        assert!(client.stat(bytes(&link)).unwrap().is_dir());
        assert!(client.lstat(bytes(&link)).unwrap().is_symlink());
    }

    #[test]
    fn errors_carry_their_kind_and_the_session_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let mut client = local_client();
        let gone = tmp.path().join("gone");
        let err = client.stat(bytes(&gone)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(err.to_string(), "No such file or folder");
        let err = client.read_dir(bytes(&gone), |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!client.is_broken());
        assert!(client.stat(bytes(tmp.path())).unwrap().is_dir());
    }

    #[test]
    fn realpath_resolves_the_login_folder_and_dot_dot() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        let mut client = local_client();
        let here = std::env::current_dir().unwrap();
        assert_eq!(client.realpath(b".").unwrap(), bytes(&here));
        let up = tmp.path().join("a/..");
        let real = tmp.path().canonicalize().unwrap();
        assert_eq!(client.realpath(bytes(&up)).unwrap(), bytes(&real));
    }

    #[test]
    fn downloads_a_file_larger_than_the_reads_in_flight() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("big");
        // Not a multiple of the read size, and more than all reads in flight.
        let data: Vec<u8> = (0..(3 << 20) + 12345)
            .map(|i: u32| (i % 251) as u8)
            .collect();
        std::fs::write(&source, &data).unwrap();
        let mut client = local_client();
        let handle = client.open_read(bytes(&source)).unwrap();
        let out = File::create(tmp.path().join("copy")).unwrap();
        let mut total = 0;
        let done = client
            .download(&handle, &out, |n| {
                total += n;
                true
            })
            .unwrap();
        client.close(handle).unwrap();
        assert!(done);
        assert_eq!(total, data.len() as u64);
        assert_eq!(std::fs::read(tmp.path().join("copy")).unwrap(), data);
    }

    #[test]
    fn a_stopped_download_leaves_the_session_in_step() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("big");
        std::fs::write(&source, vec![7; 4 << 20]).unwrap();
        let mut client = local_client();
        let handle = client.open_read(bytes(&source)).unwrap();
        let out = File::create(tmp.path().join("copy")).unwrap();
        let done = client.download(&handle, &out, |_| false).unwrap();
        assert!(!done);
        client.close(handle).unwrap();
        assert!(client.stat(bytes(&source)).unwrap().is_file());
    }

    #[test]
    fn downloads_an_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("empty");
        std::fs::write(&source, "").unwrap();
        let mut client = local_client();
        let handle = client.open_read(bytes(&source)).unwrap();
        let out = File::create(tmp.path().join("copy")).unwrap();
        assert!(client.download(&handle, &out, |_| true).unwrap());
        assert_eq!(std::fs::metadata(tmp.path().join("copy")).unwrap().len(), 0);
    }

    #[test]
    fn a_closed_stream_breaks_the_client() {
        let mut child = Command::new(server_program())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let reader = child.stdout.take().unwrap();
        let writer = child.stdin.take().unwrap();
        let mut client = Client::new(Box::new(reader), Box::new(writer)).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(client.stat(b"/").is_err());
        assert!(client.is_broken());
        let err = client.stat(b"/").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_stream_that_is_not_sftp_is_refused() {
        let reader = io::Cursor::new(b"\0\0\0\x05hello".to_vec());
        let err = Client::new(Box::new(reader), Box::new(io::sink())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let huge = io::Cursor::new(b"\xff\xff\xff\xff".to_vec());
        let err = Client::new(Box::new(huge), Box::new(io::sink())).unwrap_err();
        assert_eq!(err.to_string(), "the server sent a malformed packet");
    }
}
