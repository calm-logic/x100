//! A body that passes through a transform chunk by chunk, so nothing is buffered whole.

use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame};

use crate::redact::RedactStream;

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) trait Transform: Send + Unpin + 'static {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8>;
    fn finish(&mut self) -> Vec<u8>;
}

impl Transform for RedactStream {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        RedactStream::push(self, chunk)
    }

    fn finish(&mut self) -> Vec<u8> {
        RedactStream::finish(self)
    }
}

pub(crate) struct Transformed<B, T> {
    inner: B,
    transform: T,
    trailers: Option<HeaderMap>,
    done: bool,
}

impl<B, T> Transformed<B, T> {
    pub(crate) fn new(inner: B, transform: T) -> Self {
        Self {
            inner,
            transform,
            trailers: None,
            done: false,
        }
    }
}

impl<B, T> Body for Transformed<B, T>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
    T: Transform,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        if let Some(trailers) = this.trailers.take() {
            this.done = true;
            return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
        }
        if this.done {
            return Poll::Ready(None);
        }
        loop {
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => {
                        let out = this.transform.push(&data);
                        if !out.is_empty() {
                            return Poll::Ready(Some(Ok(Frame::data(out.into()))));
                        }
                    }
                    Err(frame) => {
                        // Trailers end the body: release what was held back first.
                        this.trailers = frame.into_trailers().ok();
                        let out = this.transform.finish();
                        if !out.is_empty() {
                            return Poll::Ready(Some(Ok(Frame::data(out.into()))));
                        }
                        this.done = true;
                        return Poll::Ready(this.trailers.take().map(|t| Ok(Frame::trailers(t))));
                    }
                },
                Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                None => {
                    this.done = true;
                    let out = this.transform.finish();
                    return Poll::Ready((!out.is_empty()).then(|| Ok(Frame::data(out.into()))));
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done && self.trailers.is_none()
    }
}
