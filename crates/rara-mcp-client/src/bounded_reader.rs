use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};

const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// Enforce the JSONL frame limit before the SDK accumulates or decodes a line.
pub(crate) struct BoundedReader<R> {
    inner: R,
    frame_bytes: usize,
    failed: bool,
}

impl<R> BoundedReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            frame_bytes: 0,
            failed: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP frame exceeds the byte limit",
            )));
        }
        let filled = output.filled().len();
        std::task::ready!(Pin::new(&mut this.inner).poll_read(cx, output))?;
        for byte in &output.filled()[filled..] {
            if *byte == b'\n' {
                this.frame_bytes = 0;
            } else {
                this.frame_bytes += 1;
                if this.frame_bytes > MAX_FRAME_BYTES {
                    this.failed = true;
                    // AsyncRead errors must not report newly filled bytes.
                    output.set_filled(filled);
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "MCP frame exceeds the byte limit",
                    )));
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // Boundary assertions keep fixture failures explicit.
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    #[tokio::test]
    async fn frame_budget_resets_at_each_delimiter_across_partial_reads() {
        let mut input = vec![b'a'; MAX_FRAME_BYTES];
        input.push(b'\n');
        input.extend(std::iter::repeat_n(b'b', MAX_FRAME_BYTES));
        input.push(b'\n');
        let mut reader = BoundedReader::new(input.as_slice());
        let mut received = 0;
        let mut buffer = [0_u8; 511];
        loop {
            let count = reader.read(&mut buffer).await.expect("in-budget frame");
            if count == 0 {
                break;
            }
            received += count;
        }
        assert_eq!(received, input.len());
    }

    #[tokio::test]
    async fn an_oversized_frame_cannot_resume_after_a_later_delimiter() {
        let mut input = vec![b'a'; MAX_FRAME_BYTES + 1];
        input.extend_from_slice(b"\n{}\n");
        let mut reader = BoundedReader::new(input.as_slice());
        let mut output = Vec::new();
        let error = reader
            .read_to_end(&mut output)
            .await
            .expect_err("oversized frame");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let mut byte = [0];
        assert_eq!(
            reader
                .read(&mut byte)
                .await
                .expect_err("closed reader")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
