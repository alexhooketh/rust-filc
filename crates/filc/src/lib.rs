//! Safe process transport for generated Rust-to-Fil-C bindings.
//!
//! This crate contains no C FFI and forbids unsafe Rust. Generated clients use
//! [`Connection`] to exchange bounded, copied values with a whole-program
//! Fil-C helper.

#![forbid(unsafe_code)]

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

pub use filc_macros::bridge;

const MAGIC: [u8; 4] = *b"FILC";
const VERSION: u16 = 2;
const REQUEST_KIND: u8 = 1;
const RESPONSE_KIND: u8 = 2;
const HEADER_LEN: usize = 20;

/// Result type used by the bridge runtime and generated bindings.
pub type Result<T> = std::result::Result<T, Error>;

/// A transport, codec, helper, or remote-dispatch failure.
#[derive(Debug)]
pub enum Error {
    /// The operating system rejected an I/O operation.
    Io(io::Error),
    /// A peer sent a malformed or incompatible frame.
    Protocol(String),
    /// The generated helper rejected a valid request.
    Remote(String),
    /// The Fil-C helper terminated or closed its protocol stream.
    HelperExited(Option<ExitStatus>),
    /// A generated handle belongs to a different helper connection.
    WrongConnection,
    /// A generated handle has already been explicitly released.
    ReleasedHandle,
    /// A returned string was not valid UTF-8.
    InvalidUtf8(std::string::FromUtf8Error),
    /// Another thread panicked while it owned the connection lock.
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "bridge I/O failed: {error}"),
            Self::Protocol(message) => write!(formatter, "bridge protocol error: {message}"),
            Self::Remote(message) => write!(formatter, "Fil-C helper rejected the call: {message}"),
            Self::HelperExited(Some(status)) => {
                write!(formatter, "Fil-C helper exited with status {status}")
            }
            Self::HelperExited(None) => {
                write!(formatter, "Fil-C helper closed its protocol stream")
            }
            Self::WrongConnection => write!(formatter, "opaque handle belongs to another client"),
            Self::ReleasedHandle => write!(formatter, "opaque handle was already released"),
            Self::InvalidUtf8(error) => {
                write!(formatter, "Fil-C helper returned invalid UTF-8: {error}")
            }
            Self::Poisoned => write!(formatter, "bridge connection lock is poisoned"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::Protocol(_)
            | Self::Remote(_)
            | Self::HelperExited(_)
            | Self::WrongConnection
            | Self::ReleasedHandle
            | Self::Poisoned => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}

/// An append-only encoder for protocol values.
#[derive(Debug, Default)]
pub struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    /// Creates an empty encoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Appends an unsigned byte.
    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Appends a Boolean as zero or one.
    pub fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    /// Appends a signed 32-bit integer.
    pub fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends an unsigned 32-bit integer.
    pub fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a signed 64-bit integer.
    pub fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends an unsigned 64-bit integer.
    pub fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends an IEEE-754 32-bit float.
    pub fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    /// Appends an IEEE-754 64-bit float.
    pub fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    /// Appends a length-prefixed byte slice.
    pub fn bytes(&mut self, value: &[u8]) -> Result<()> {
        let length = u32::try_from(value.len())
            .map_err(|_| Error::Protocol("value exceeds the 32-bit wire length".into()))?;
        self.u32(length);
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    /// Appends a length-prefixed UTF-8 string.
    pub fn string(&mut self, value: &str) -> Result<()> {
        self.bytes(value.as_bytes())
    }

    /// Returns the completed value bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// A bounds-checking decoder for one response payload.
#[derive(Debug)]
pub struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    /// Creates a decoder over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| Error::Protocol("decoder offset overflow".into()))?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| Error::Protocol("truncated response value".into()))?;
        self.cursor = end;
        Ok(value)
    }

    /// Decodes an unsigned byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Decodes a Boolean encoded as zero or one.
    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Protocol("invalid Boolean value".into())),
        }
    }

    /// Decodes a signed 32-bit integer.
    pub fn i32(&mut self) -> Result<i32> {
        let mut bytes = [0_u8; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(i32::from_le_bytes(bytes))
    }

    /// Decodes an unsigned 32-bit integer.
    pub fn u32(&mut self) -> Result<u32> {
        let mut bytes = [0_u8; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(bytes))
    }

    /// Decodes a signed 64-bit integer.
    pub fn i64(&mut self) -> Result<i64> {
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(i64::from_le_bytes(bytes))
    }

    /// Decodes an unsigned 64-bit integer.
    pub fn u64(&mut self) -> Result<u64> {
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(bytes))
    }

    /// Decodes an IEEE-754 32-bit float.
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// Decodes an IEEE-754 64-bit float.
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// Decodes a length-prefixed byte vector.
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let length = usize::try_from(self.u32()?).map_err(|_| {
            Error::Protocol("32-bit lengths require a 32-bit or wider target".into())
        })?;
        Ok(self.take(length)?.to_vec())
    }

    /// Decodes a length-prefixed owned UTF-8 string.
    pub fn string(&mut self) -> Result<String> {
        Ok(String::from_utf8(self.bytes()?)?)
    }

    /// Rejects unexpected trailing data.
    pub fn finish(self) -> Result<()> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::Protocol("response contains trailing bytes".into()))
        }
    }
}

/// One synchronized connection to a persistent Fil-C helper process.
#[derive(Debug)]
pub struct Connection {
    process: Mutex<Process>,
    max_frame_bytes: u32,
}

impl Connection {
    /// Starts a helper and connects its stdin/stdout protocol pipes.
    pub fn spawn(program: impl AsRef<Path>, max_frame_bytes: u32) -> Result<Self> {
        if max_frame_bytes < 40 {
            return Err(Error::Protocol("maximum frame size is too small".into()));
        }

        let mut child = Command::new(program.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Protocol("helper stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Protocol("helper stdout was not piped".into()))?;

        Ok(Self {
            process: Mutex::new(Process {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                next_request_id: 1,
            }),
            max_frame_bytes,
        })
    }

    /// Sends one generated operation and returns its encoded result.
    pub fn call(&self, schema_hash: [u8; 32], operation: u32, arguments: &[u8]) -> Result<Vec<u8>> {
        let mut payload = Vec::with_capacity(36_usize.saturating_add(arguments.len()));
        payload.extend_from_slice(&schema_hash);
        payload.extend_from_slice(&operation.to_le_bytes());
        payload.extend_from_slice(arguments);
        let payload_length = u32::try_from(payload.len())
            .map_err(|_| Error::Protocol("request exceeds the 32-bit frame length".into()))?;
        if payload_length > self.max_frame_bytes {
            return Err(Error::Protocol(
                "request exceeds configured maximum frame size".into(),
            ));
        }

        let mut process = self.process.lock().map_err(|_| Error::Poisoned)?;
        process.call(&payload, payload_length, self.max_frame_bytes)
    }
}

/// Lazily owns the current connection for one generated bridge module.
///
/// Generated free functions use this type to hide helper startup. When a
/// helper terminates or corrupts its transport, the failed connection is
/// discarded so the next independent call can start a clean helper.
#[derive(Debug)]
pub struct Bridge {
    program_environment: &'static str,
    default_program: &'static str,
    max_frame_bytes: u32,
    connection: Mutex<Option<Arc<Connection>>>,
}

impl Bridge {
    /// Creates an initially disconnected generated bridge runtime.
    #[must_use]
    pub const fn new(
        program_environment: &'static str,
        default_program: &'static str,
        max_frame_bytes: u32,
    ) -> Self {
        Self {
            program_environment,
            default_program,
            max_frame_bytes,
            connection: Mutex::new(None),
        }
    }

    /// Returns the current helper connection, starting it on first use.
    pub fn connection(&self) -> Result<Arc<Connection>> {
        let mut current = self.connection.lock().map_err(|_| Error::Poisoned)?;
        if let Some(connection) = current.as_ref() {
            return Ok(connection.clone());
        }
        let program = std::env::var_os(self.program_environment)
            .unwrap_or_else(|| self.default_program.into());
        let connection = Arc::new(Connection::spawn(program, self.max_frame_bytes)?);
        *current = Some(connection.clone());
        Ok(connection)
    }

    /// Calls through `connection` and retires it after a transport failure.
    pub fn call(
        &self,
        connection: &Arc<Connection>,
        schema_hash: [u8; 32],
        operation: u32,
        arguments: &[u8],
    ) -> Result<Vec<u8>> {
        let result = connection.call(schema_hash, operation, arguments);
        if result.as_ref().is_err_and(Error::breaks_connection) {
            self.retire(connection)?;
        }
        result
    }

    fn retire(&self, failed: &Arc<Connection>) -> Result<()> {
        let mut current = self.connection.lock().map_err(|_| Error::Poisoned)?;
        if current
            .as_ref()
            .is_some_and(|connection| Arc::ptr_eq(connection, failed))
        {
            *current = None;
        }
        Ok(())
    }
}

impl Error {
    const fn breaks_connection(&self) -> bool {
        matches!(self, Self::Io(_) | Self::HelperExited(_) | Self::Poisoned)
    }
}

#[derive(Debug)]
struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
}

impl Process {
    fn call(
        &mut self,
        payload: &[u8],
        payload_length: u32,
        max_frame_bytes: u32,
    ) -> Result<Vec<u8>> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| Error::Protocol("request ID space exhausted".into()))?;

        let mut header = [0_u8; HEADER_LEN];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..6].copy_from_slice(&VERSION.to_le_bytes());
        header[6] = REQUEST_KIND;
        header[7] = 0;
        header[8..16].copy_from_slice(&request_id.to_le_bytes());
        header[16..20].copy_from_slice(&payload_length.to_le_bytes());

        self.stdin.write_all(&header)?;
        self.stdin.write_all(payload)?;
        self.stdin.flush()?;

        let mut response_header = [0_u8; HEADER_LEN];
        if let Err(error) = self.stdout.read_exact(&mut response_header) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                return Err(Error::HelperExited(self.child.try_wait()?));
            }
            return Err(Error::Io(error));
        }
        validate_response_header(&response_header, request_id, max_frame_bytes)?;
        let status = response_header[7];
        let mut length_bytes = [0_u8; 4];
        length_bytes.copy_from_slice(&response_header[16..20]);
        let response_length = usize::try_from(u32::from_le_bytes(length_bytes)).map_err(|_| {
            Error::Protocol("32-bit lengths require a 32-bit or wider target".into())
        })?;
        let mut response = vec![0_u8; response_length];
        self.stdout.read_exact(&mut response)?;

        match status {
            0 => Ok(response),
            1 => Err(Error::Remote(
                String::from_utf8_lossy(&response).into_owned(),
            )),
            value => Err(Error::Protocol(format!("unknown response status {value}"))),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn validate_response_header(header: &[u8; HEADER_LEN], request_id: u64, max: u32) -> Result<()> {
    if header[0..4] != MAGIC {
        return Err(Error::Protocol("invalid response magic".into()));
    }
    let mut version_bytes = [0_u8; 2];
    version_bytes.copy_from_slice(&header[4..6]);
    if u16::from_le_bytes(version_bytes) != VERSION {
        return Err(Error::Protocol(
            "unsupported response protocol version".into(),
        ));
    }
    if header[6] != RESPONSE_KIND {
        return Err(Error::Protocol("expected a response frame".into()));
    }
    let mut request_bytes = [0_u8; 8];
    request_bytes.copy_from_slice(&header[8..16]);
    if u64::from_le_bytes(request_bytes) != request_id {
        return Err(Error::Protocol("response request ID mismatch".into()));
    }
    let mut length_bytes = [0_u8; 4];
    length_bytes.copy_from_slice(&header[16..20]);
    let length = u32::from_le_bytes(length_bytes);
    if length > max {
        return Err(Error::Protocol(
            "response exceeds configured maximum frame size".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Decoder, Encoder, Error, HEADER_LEN, MAGIC, RESPONSE_KIND, VERSION,
        validate_response_header,
    };

    #[test]
    fn codecs_round_trip_every_value_class() {
        let mut encoder = Encoder::new();
        encoder.bool(true);
        encoder.i32(-7);
        encoder.u64(42);
        encoder.f64(1.25);
        encoder.string("hello").unwrap();
        encoder.bytes(&[0, 1, 2]).unwrap();

        let wire = encoder.finish();
        let mut decoder = Decoder::new(&wire);
        assert!(decoder.bool().unwrap());
        assert_eq!(decoder.i32().unwrap(), -7);
        assert_eq!(decoder.u64().unwrap(), 42);
        assert_eq!(decoder.f64().unwrap().to_bits(), 1.25_f64.to_bits());
        assert_eq!(decoder.string().unwrap(), "hello");
        assert_eq!(decoder.bytes().unwrap(), [0, 1, 2]);
        decoder.finish().unwrap();
    }

    #[test]
    fn decoder_rejects_truncation_and_trailing_data() {
        let mut decoder = Decoder::new(&[4, 0, 0, 0, 1]);
        assert!(matches!(decoder.bytes(), Err(Error::Protocol(_))));

        let mut decoder = Decoder::new(&[1, 2]);
        assert_eq!(decoder.u8().unwrap(), 1);
        assert!(matches!(decoder.finish(), Err(Error::Protocol(_))));
    }

    #[test]
    fn response_header_is_strictly_validated() {
        let request_id = 9_u64;
        let mut header = [0_u8; HEADER_LEN];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..6].copy_from_slice(&VERSION.to_le_bytes());
        header[6] = RESPONSE_KIND;
        header[8..16].copy_from_slice(&request_id.to_le_bytes());
        header[16..20].copy_from_slice(&3_u32.to_le_bytes());
        validate_response_header(&header, request_id, 3).unwrap();

        header[6] = 99;
        assert!(matches!(
            validate_response_header(&header, request_id, 3),
            Err(Error::Protocol(_))
        ));
    }
}
