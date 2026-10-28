//! CTAPHID: the USB HID framing that carries CTAP2 messages.
//!
//! A CTAPHID message is split into 64-byte HID reports ("packets").  The
//! first (initialization) packet carries a 4-byte channel ID, the command
//! byte with its high bit set, a 2-byte big-endian payload length and up to
//! 57 payload bytes.  Continuation packets carry the channel ID, a sequence
//! number (0..=127) and up to 59 further payload bytes.
//!
//! While a command is running the key sends `KEEPALIVE` packets, for example
//! to say that it is waiting for a touch.
//!
//! See section 11.2, "USB Human Interface Device (USB HID)", of the CTAP 2.1
//! specification.

use std::io;
use std::time::{Duration, Instant};

use crate::error::{LpError, Result};

/// Size of a FIDO HID report.  Every FIDO2 USB key uses 64-byte reports.
pub(crate) const PACKET_SIZE: usize = 64;

/// One HID report, without the leading report-ID byte.
pub(crate) type Packet = [u8; PACKET_SIZE];

const INIT_HEADER: usize = 7;
const CONT_HEADER: usize = 5;
const INIT_DATA: usize = PACKET_SIZE - INIT_HEADER;
const CONT_DATA: usize = PACKET_SIZE - CONT_HEADER;
const MAX_SEQUENCE: usize = 128;
const MAX_MESSAGE_SIZE: usize = INIT_DATA + MAX_SEQUENCE * CONT_DATA;

const BROADCAST_CID: [u8; 4] = [0xff; 4];
const TYPE_INIT: u8 = 0x80;

const CMD_INIT: u8 = 0x06;
const CMD_CBOR: u8 = 0x10;
const CMD_CANCEL: u8 = 0x11;
const CMD_KEEPALIVE: u8 = 0x3b;
const CMD_ERROR: u8 = 0x3f;

const CAPABILITY_CBOR: u8 = 0x04;

/// Keepalive status: the key is waiting for the user to touch it.
pub(crate) const KEEPALIVE_UP_NEEDED: u8 = 2;

/// How long to wait for the key to answer `CTAPHID_INIT`.
const INIT_TIMEOUT: Duration = Duration::from_secs(3);

/// A raw HID device that exchanges 64-byte reports.
pub(crate) trait Transport {
    /// Send one report.
    fn write_packet(&mut self, packet: &Packet) -> io::Result<()>;

    /// Receive one report, or `None` if nothing arrived within `timeout`.
    fn read_packet(&mut self, timeout: Duration) -> io::Result<Option<Packet>>;
}

/// Placeholder transport for platforms without USB security-key support.
#[cfg(not(target_os = "linux"))]
pub(crate) enum Unsupported {}

#[cfg(not(target_os = "linux"))]
impl Transport for Unsupported {
    fn write_packet(&mut self, _packet: &Packet) -> io::Result<()> {
        match *self {}
    }

    fn read_packet(&mut self, _timeout: Duration) -> io::Result<Option<Packet>> {
        match *self {}
    }
}

/// Why a message could not be received.
enum ReceiveError {
    Timeout,
    Failed(LpError),
}

/// An allocated CTAPHID channel on one device.
pub(crate) struct Channel<T> {
    transport: T,
    cid: [u8; 4],
}

impl<T: Transport> Channel<T> {
    /// Allocate a channel on `transport` with `CTAPHID_INIT`.
    ///
    /// Fails if the key does not support CTAP2 (CBOR) messages, e.g. a
    /// U2F-only key.
    pub(crate) fn open(mut transport: T) -> Result<Self> {
        let nonce: [u8; 8] = rand::random();
        send(&mut transport, BROADCAST_CID, CMD_INIT, &nonce)?;

        let deadline = Instant::now() + INIT_TIMEOUT;
        loop {
            let (cmd, data) = match receive(&mut transport, BROADCAST_CID, deadline, &mut |_| {}) {
                Ok(message) => message,
                Err(ReceiveError::Timeout) => {
                    return Err(LpError::HardwareKey(
                        "The security key did not respond.".to_string(),
                    ));
                }
                Err(ReceiveError::Failed(e)) => return Err(e),
            };

            // Other applications share the broadcast channel, so only accept
            // the reply to our own nonce.
            if cmd != CMD_INIT || data.len() < 17 || data[..8] != nonce {
                continue;
            }
            if data[16] & CAPABILITY_CBOR == 0 {
                return Err(LpError::HardwareKey(
                    "This security key does not support FIDO2 (CTAP2).".to_string(),
                ));
            }

            let mut cid = [0u8; 4];
            cid.copy_from_slice(&data[8..12]);
            return Ok(Self { transport, cid });
        }
    }

    /// Send a `CTAPHID_CBOR` request and wait up to `timeout` for the reply.
    ///
    /// `on_keepalive` receives each keepalive status reported meanwhile.  On
    /// timeout the request is cancelled so the key stops waiting for a touch.
    pub(crate) fn cbor(
        &mut self,
        request: &[u8],
        timeout: Duration,
        on_keepalive: &mut dyn FnMut(u8),
    ) -> Result<Vec<u8>> {
        send(&mut self.transport, self.cid, CMD_CBOR, request)?;

        let deadline = Instant::now() + timeout;
        match receive(&mut self.transport, self.cid, deadline, on_keepalive) {
            Ok((CMD_CBOR, data)) => Ok(data),
            Ok((cmd, _)) => Err(LpError::HardwareKey(format!(
                "Unexpected CTAPHID response 0x{cmd:02x}."
            ))),
            Err(ReceiveError::Timeout) => {
                let _ = send(&mut self.transport, self.cid, CMD_CANCEL, &[]);
                Err(LpError::HardwareKeyTimeout)
            }
            Err(ReceiveError::Failed(e)) => Err(e),
        }
    }
}

/// Split a message into CTAPHID packets.
fn encode(cid: [u8; 4], cmd: u8, data: &[u8]) -> Result<Vec<Packet>> {
    let length = u16::try_from(data.len())
        .ok()
        .filter(|_| data.len() <= MAX_MESSAGE_SIZE)
        .ok_or_else(|| LpError::HardwareKey("The CTAPHID message is too large.".to_string()))?;

    let (head, rest) = data.split_at(data.len().min(INIT_DATA));
    let mut first = [0u8; PACKET_SIZE];
    first[..4].copy_from_slice(&cid);
    first[4] = TYPE_INIT | cmd;
    first[5..INIT_HEADER].copy_from_slice(&length.to_be_bytes());
    first[INIT_HEADER..INIT_HEADER + head.len()].copy_from_slice(head);

    let mut packets = Vec::with_capacity(1 + rest.len().div_ceil(CONT_DATA));
    packets.push(first);
    // The size check above bounds the sequence number to 0..MAX_SEQUENCE.
    for (sequence, chunk) in (0u8..).zip(rest.chunks(CONT_DATA)) {
        let mut packet = [0u8; PACKET_SIZE];
        packet[..4].copy_from_slice(&cid);
        packet[4] = sequence;
        packet[CONT_HEADER..CONT_HEADER + chunk.len()].copy_from_slice(chunk);
        packets.push(packet);
    }
    Ok(packets)
}

fn send<T: Transport>(transport: &mut T, cid: [u8; 4], cmd: u8, data: &[u8]) -> Result<()> {
    for packet in encode(cid, cmd, data)? {
        transport.write_packet(&packet).map_err(io_error)?;
    }
    Ok(())
}

/// Receive one complete message on `cid`, skipping keepalives and packets
/// for other channels.
fn receive<T: Transport>(
    transport: &mut T,
    cid: [u8; 4],
    deadline: Instant,
    on_keepalive: &mut dyn FnMut(u8),
) -> std::result::Result<(u8, Vec<u8>), ReceiveError> {
    let malformed =
        || ReceiveError::Failed(LpError::HardwareKey("Malformed CTAPHID response.".into()));

    loop {
        let packet = read_before(transport, deadline)?;
        if packet[..4] != cid || packet[4] & TYPE_INIT == 0 {
            continue;
        }

        let cmd = packet[4] & !TYPE_INIT;
        if cmd == CMD_KEEPALIVE {
            on_keepalive(packet[INIT_HEADER]);
            continue;
        }

        let length = usize::from(u16::from_be_bytes([packet[5], packet[6]]));
        if length > MAX_MESSAGE_SIZE {
            return Err(malformed());
        }
        let mut data = Vec::with_capacity(length);
        data.extend_from_slice(&packet[INIT_HEADER..INIT_HEADER + length.min(INIT_DATA)]);

        let mut expected_sequence = 0u8;
        while data.len() < length {
            let packet = read_before(transport, deadline)?;
            if packet[..4] != cid {
                continue;
            }
            if packet[4] != expected_sequence {
                return Err(malformed());
            }
            let take = (length - data.len()).min(CONT_DATA);
            data.extend_from_slice(&packet[CONT_HEADER..CONT_HEADER + take]);
            expected_sequence += 1;
        }

        if cmd == CMD_ERROR {
            let code = data.first().copied().unwrap_or(0x7f);
            return Err(ReceiveError::Failed(hid_error(code)));
        }
        return Ok((cmd, data));
    }
}

fn read_before<T: Transport>(
    transport: &mut T,
    deadline: Instant,
) -> std::result::Result<Packet, ReceiveError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ReceiveError::Timeout);
    }
    match transport.read_packet(remaining) {
        Ok(Some(packet)) => Ok(packet),
        Ok(None) => Err(ReceiveError::Timeout),
        Err(e) => Err(ReceiveError::Failed(io_error(e))),
    }
}

fn hid_error(code: u8) -> LpError {
    match code {
        0x06 => LpError::HardwareKey(
            "The security key is busy with another application; try again.".to_string(),
        ),
        _ => LpError::HardwareKey(format!(
            "The security key reported CTAPHID error 0x{code:02x}."
        )),
    }
}

/// Convert a device I/O error into an [`LpError`].
pub(crate) fn io_error(e: io::Error) -> LpError {
    LpError::HardwareKey(format!("Cannot communicate with the security key: {e}"))
}

// ---------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::VecDeque;

    use super::*;

    /// The channel ID the simulated device allocates.
    pub(crate) const TEST_CID: [u8; 4] = [0x01, 0x02, 0x03, 0x04];

    /// A partially received request.
    struct Incoming {
        cid: [u8; 4],
        cmd: u8,
        length: usize,
        data: Vec<u8>,
    }

    /// A simulated CTAPHID device.  Each complete `CTAPHID_CBOR` request is
    /// passed to `handler`; returning `None` simulates a key that never
    /// answers (for example, one that is never touched).
    pub(crate) struct SimulatedDevice<F> {
        handler: F,
        incoming: Option<Incoming>,
        outbox: VecDeque<Packet>,
        /// Keepalive statuses sent before each CBOR reply.
        pub(crate) keepalives: Vec<u8>,
        /// The `CTAPHID_INIT` capability flags.
        pub(crate) capabilities: u8,
        /// Whether a `CTAPHID_CANCEL` was received.
        pub(crate) cancelled: bool,
    }

    impl<F: FnMut(&[u8]) -> Option<Vec<u8>>> SimulatedDevice<F> {
        pub(crate) fn new(handler: F) -> Self {
            Self {
                handler,
                incoming: None,
                outbox: VecDeque::new(),
                keepalives: Vec::new(),
                capabilities: CAPABILITY_CBOR,
                cancelled: false,
            }
        }

        fn reply(&mut self, cid: [u8; 4], cmd: u8, data: &[u8]) {
            self.outbox
                .extend(encode(cid, cmd, data).expect("reply fits in a CTAPHID message"));
        }

        fn dispatch(&mut self, request: Incoming) {
            match request.cmd {
                CMD_INIT => {
                    let mut data = request.data.clone();
                    data.extend_from_slice(&TEST_CID);
                    data.extend_from_slice(&[2, 5, 4, 3, self.capabilities]);
                    self.reply(request.cid, CMD_INIT, &data);
                }
                CMD_CBOR => {
                    for status in self.keepalives.clone() {
                        let mut packet = [0u8; PACKET_SIZE];
                        packet[..4].copy_from_slice(&request.cid);
                        packet[4] = TYPE_INIT | CMD_KEEPALIVE;
                        packet[6] = 1;
                        packet[INIT_HEADER] = status;
                        self.outbox.push_back(packet);
                    }
                    if let Some(response) = (self.handler)(&request.data) {
                        self.reply(request.cid, CMD_CBOR, &response);
                    }
                }
                CMD_CANCEL => self.cancelled = true,
                other => panic!("unexpected CTAPHID command 0x{other:02x}"),
            }
        }
    }

    impl<F: FnMut(&[u8]) -> Option<Vec<u8>>> Transport for SimulatedDevice<F> {
        fn write_packet(&mut self, packet: &Packet) -> io::Result<()> {
            let mut cid = [0u8; 4];
            cid.copy_from_slice(&packet[..4]);

            if packet[4] & TYPE_INIT != 0 {
                let length = usize::from(u16::from_be_bytes([packet[5], packet[6]]));
                let take = length.min(INIT_DATA);
                self.incoming = Some(Incoming {
                    cid,
                    cmd: packet[4] & !TYPE_INIT,
                    length,
                    data: packet[INIT_HEADER..INIT_HEADER + take].to_vec(),
                });
            } else if let Some(incoming) = self.incoming.as_mut() {
                let take = (incoming.length - incoming.data.len()).min(CONT_DATA);
                incoming
                    .data
                    .extend_from_slice(&packet[CONT_HEADER..CONT_HEADER + take]);
            }

            if self
                .incoming
                .as_ref()
                .is_some_and(|incoming| incoming.data.len() == incoming.length)
                && let Some(request) = self.incoming.take()
            {
                self.dispatch(request);
            }
            Ok(())
        }

        fn read_packet(&mut self, _timeout: Duration) -> io::Result<Option<Packet>> {
            Ok(self.outbox.pop_front())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::testing::{SimulatedDevice, TEST_CID};
    use super::*;

    #[test]
    fn encode_splits_into_init_and_continuation_packets() {
        let data: Vec<u8> = (0..200u16).map(|n| n as u8).collect();
        let packets = encode(TEST_CID, CMD_CBOR, &data).unwrap();

        // 57 bytes in the first packet, then 59 + 59 + 25.
        assert_eq!(packets.len(), 4);
        assert_eq!(&packets[0][..4], &TEST_CID);
        assert_eq!(packets[0][4], TYPE_INIT | CMD_CBOR);
        assert_eq!(u16::from_be_bytes([packets[0][5], packets[0][6]]), 200);
        assert_eq!(&packets[0][INIT_HEADER..], &data[..INIT_DATA]);
        for (sequence, packet) in packets[1..].iter().enumerate() {
            assert_eq!(usize::from(packet[4]), sequence);
        }
        assert_eq!(packets[3][CONT_HEADER + 24], 199);
    }

    #[test]
    fn encode_rejects_oversized_messages() {
        assert!(encode(TEST_CID, CMD_CBOR, &vec![0u8; MAX_MESSAGE_SIZE + 1]).is_err());
        assert!(encode(TEST_CID, CMD_CBOR, &vec![0u8; MAX_MESSAGE_SIZE]).is_ok());
    }

    #[test]
    fn channel_round_trips_large_message_and_reports_keepalives() {
        let mut device =
            SimulatedDevice::new(|request: &[u8]| Some(request.iter().rev().copied().collect()));
        device.keepalives = vec![KEEPALIVE_UP_NEEDED, KEEPALIVE_UP_NEEDED];
        let mut channel = Channel::open(device).unwrap();
        assert_eq!(channel.cid, TEST_CID);

        let request: Vec<u8> = (0..300u16).map(|n| (n % 251) as u8).collect();
        let mut keepalives = Vec::new();
        let response = channel
            .cbor(&request, Duration::from_secs(1), &mut |status| {
                keepalives.push(status)
            })
            .unwrap();

        assert_eq!(response, request.iter().rev().copied().collect::<Vec<_>>());
        assert_eq!(keepalives, vec![KEEPALIVE_UP_NEEDED; 2]);
    }

    #[test]
    fn channel_rejects_keys_without_ctap2() {
        let mut device = SimulatedDevice::new(|_: &[u8]| None);
        device.capabilities = 0;
        let err = Channel::open(device).err().unwrap();
        assert!(err.to_string().contains("CTAP2"), "{err}");
    }

    #[test]
    fn unanswered_request_times_out_and_cancels() {
        let device = SimulatedDevice::new(|_: &[u8]| None);
        let mut channel = Channel::open(device).unwrap();
        let err = channel
            .cbor(&[0x04], Duration::from_millis(10), &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, LpError::HardwareKeyTimeout), "{err}");
        assert!(channel.transport.cancelled);
    }
}
