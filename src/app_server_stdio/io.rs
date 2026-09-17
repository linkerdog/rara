use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use rara_app_server::stdio_protocol::{
    ClientFrame, MAX_FRAME_BYTES, ServerFrame, decode_client_frame, encode_server_frame,
};
use tokio::sync::{mpsc, oneshot};

use crate::runtime_control::RuntimeControlEvent;

pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) type Frame = ServerFrame<RuntimeControlEvent>;

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub(super) enum Failure {
    #[error("app-server input is malformed, truncated or oversized")]
    Framing,
    #[error("app-server input closed without semantic shutdown")]
    InputClosed,
    #[error("app-server output failed or stalled")]
    Output,
    #[error("app-server runtime operation failed")]
    Runtime,
    #[error("app-server session cleanup failed")]
    Cleanup,
    #[error("app-server event replay requires resynchronization")]
    ReplayGap,
}

type Packet = (Vec<u8>, Option<oneshot::Sender<()>>);

#[derive(Clone)]
pub(super) struct Output(mpsc::Sender<Packet>);

impl Output {
    pub(super) async fn send(&self, frame: Frame) -> Result<(), Failure> {
        let bytes = encode_server_frame(&frame).map_err(|_| Failure::Output)?;
        tokio::time::timeout(IO_TIMEOUT, self.0.send((bytes, None)))
            .await
            .map_err(|_| Failure::Output)?
            .map_err(|_| Failure::Output)
    }

    pub(super) async fn flush_handshake(&self, frame: Frame) -> Result<(), Failure> {
        let bytes = encode_server_frame(&frame).map_err(|_| Failure::Output)?;
        let (sender, receiver) = oneshot::channel();
        tokio::time::timeout(IO_TIMEOUT, self.0.send((bytes, Some(sender))))
            .await
            .map_err(|_| Failure::Output)?
            .map_err(|_| Failure::Output)?;
        tokio::time::timeout(IO_TIMEOUT, receiver)
            .await
            .map_err(|_| Failure::Output)?
            .map_err(|_| Failure::Output)
    }
}

pub(super) fn start_writer<W: Write + Send + 'static>(
    mut writer: W,
) -> Result<(Output, oneshot::Receiver<Result<(), Failure>>), Failure> {
    let (sender, mut receiver) = mpsc::channel::<Packet>(32);
    let (done, completion) = oneshot::channel();
    std::thread::Builder::new()
        .name("app-server-output".into())
        .spawn(move || {
            let result = (|| {
                while let Some((bytes, flushed)) = receiver.blocking_recv() {
                    writer.write_all(&bytes).map_err(|_| Failure::Output)?;
                    writer.flush().map_err(|_| Failure::Output)?;
                    if let Some(flushed) = flushed {
                        let _ = flushed.send(());
                    }
                }
                Ok(())
            })();
            let _ = done.send(result);
        })
        .map_err(|_| Failure::Output)?;
    Ok((Output(sender), completion))
}

pub(super) fn start_reader<R: Read + Send + 'static>(
    reader: R,
) -> Result<mpsc::Receiver<Result<ClientFrame, Failure>>, Failure> {
    let (sender, receiver) = mpsc::channel(8);
    std::thread::Builder::new()
        .name("app-server-input".into())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                let next = read_frame(&mut reader);
                match next {
                    Ok(Some(frame)) => {
                        if sender.blocking_send(Ok(frame)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = sender.blocking_send(Err(Failure::InputClosed));
                        return;
                    }
                    Err(error) => {
                        let _ = sender.blocking_send(Err(error));
                        return;
                    }
                }
            }
        })
        .map_err(|_| Failure::Framing)?;
    Ok(receiver)
}

pub(super) fn read_frame(reader: &mut impl BufRead) -> Result<Option<ClientFrame>, Failure> {
    // Include at most one CR delimiter byte; never grow an unbounded read_line buffer.
    let mut payload = Vec::with_capacity(MAX_FRAME_BYTES + 1);
    loop {
        let bytes = reader.fill_buf().map_err(|_| Failure::Framing)?;
        if bytes.is_empty() {
            return if payload.is_empty() {
                Ok(None)
            } else {
                Err(Failure::Framing)
            };
        }
        let delimiter = bytes.iter().position(|byte| *byte == b'\n');
        let take = delimiter.unwrap_or(bytes.len());
        if payload.len() + take > MAX_FRAME_BYTES + 1 {
            return Err(Failure::Framing);
        }
        payload.extend_from_slice(&bytes[..take]);
        reader.consume(take + usize::from(delimiter.is_some()));
        if delimiter.is_some() {
            if payload.last() == Some(&b'\r') {
                payload.pop();
            }
            return decode_client_frame(&payload)
                .map(Some)
                .map_err(|_| Failure::Framing);
        }
    }
}
