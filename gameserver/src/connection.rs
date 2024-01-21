use futures_util::{Sink, Stream, StreamExt};
use nertsio_types as ni_ty;

#[async_trait::async_trait]
pub trait ConnectionHandle {
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), anyhow::Error>;
    fn close(&self, code: u8);
}

#[async_trait::async_trait]
pub trait Connection {
    type BiOut: Sink<bytes::Bytes, Error = anyhow::Error> + Send + Unpin;
    type BiIn: Stream<Item = Result<bytes::BytesMut, anyhow::Error>> + Send + Unpin;
    type DatagramsIn: Stream<Item = Result<bytes::Bytes, anyhow::Error>> + Send;
    type Handle: ConnectionHandle + Sync + Send + 'static;

    async fn accept_bi_stream(
        &mut self,
    ) -> Option<Result<(Self::BiOut, Self::BiIn), anyhow::Error>>;
    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), anyhow::Error>;

    fn into_datagrams(self) -> Self::DatagramsIn;
    fn create_handle(&self) -> Self::Handle;
}

pin_project_lite::pin_project! {
    pub struct MapErrAnyhowSink<T, E: Into<anyhow::Error>, W: Sink<T, Error=E>> {
        #[pin]
        dest: W,
        _p: std::marker::PhantomData<(T, E)>,
    }
}

impl<T, E: Into<anyhow::Error>, W: Sink<T, Error = E>> MapErrAnyhowSink<T, E, W> {
    pub fn new(dest: W) -> Self {
        Self {
            dest,
            _p: Default::default(),
        }
    }
}

impl<T, E: Into<anyhow::Error>, W: Sink<T, Error = E>> Sink<T> for MapErrAnyhowSink<T, E, W> {
    type Error = anyhow::Error;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.project().dest.poll_ready(cx).map_err(Into::into)
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.project().dest.start_send(item).map_err(Into::into)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.project().dest.poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.project().dest.poll_close(cx).map_err(Into::into)
    }
}

pin_project_lite::pin_project! {
    pub struct MapErrAnyhowStream<T, E: Into<anyhow::Error>, R: Stream<Item=Result<T, E>>> {
        #[pin]
        src: R,
    }
}

impl<T, E: Into<anyhow::Error>, R: Stream<Item = Result<T, E>>> MapErrAnyhowStream<T, E, R> {
    pub fn new(src: R) -> Self {
        MapErrAnyhowStream { src }
    }
}

impl<T, E: Into<anyhow::Error>, R: Stream<Item = Result<T, E>>> Stream
    for MapErrAnyhowStream<T, E, R>
{
    type Item = Result<T, anyhow::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.project().src.poll_next(cx) {
            std::task::Poll::Ready(Some(item)) => {
                std::task::Poll::Ready(Some(item.map_err(Into::into)))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

fn add_write_framing(dest: quinn::SendStream) -> <quinn::NewConnection as Connection>::BiOut {
    MapErrAnyhowSink::new(tokio_util::codec::FramedWrite::new(
        dest,
        tokio_util::codec::LengthDelimitedCodec::new(),
    ))
}

fn add_read_framing(src: quinn::RecvStream) -> <quinn::NewConnection as Connection>::BiIn {
    MapErrAnyhowStream::new(tokio_util::codec::FramedRead::new(
        src,
        tokio_util::codec::LengthDelimitedCodec::new(),
    ))
}

#[async_trait::async_trait]
impl Connection for quinn::NewConnection {
    type BiOut = MapErrAnyhowSink<
        bytes::Bytes,
        std::io::Error,
        tokio_util::codec::FramedWrite<quinn::SendStream, tokio_util::codec::LengthDelimitedCodec>,
    >;
    type BiIn = MapErrAnyhowStream<
        bytes::BytesMut,
        std::io::Error,
        tokio_util::codec::FramedRead<quinn::RecvStream, tokio_util::codec::LengthDelimitedCodec>,
    >;
    type DatagramsIn = MapErrAnyhowStream<bytes::Bytes, quinn::ConnectionError, quinn::Datagrams>;
    type Handle = quinn::Connection;

    async fn accept_bi_stream(
        &mut self,
    ) -> Option<Result<(Self::BiOut, Self::BiIn), anyhow::Error>> {
        match self.bi_streams.next().await {
            None => None,
            Some(Err(err)) => Some(Err(err.into())),
            Some(Ok((send, recv))) => Some(Ok((add_write_framing(send), add_read_framing(recv)))),
        }
    }

    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), anyhow::Error> {
        let (send, recv) = self.connection.open_bi().await?;

        Ok((add_write_framing(send), add_read_framing(recv)))
    }

    fn into_datagrams(self) -> Self::DatagramsIn {
        MapErrAnyhowStream::new(self.datagrams)
    }

    fn create_handle(&self) -> Self::Handle {
        self.connection.clone()
    }
}

#[async_trait::async_trait]
impl ConnectionHandle for quinn::Connection {
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), anyhow::Error> {
        self.send_datagram(data).map_err(Into::into)
    }

    fn close(&self, code: u8) {
        self.close(
            code.into(),
            ni_ty::protocol::get_close_message(code).as_bytes(),
        );
    }
}
