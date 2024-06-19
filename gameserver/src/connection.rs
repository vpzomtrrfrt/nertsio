use futures_util::{Sink, Stream};
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
    type Handle: ConnectionHandle + Sync + Send + 'static;

    async fn accept_bi_stream(
        &mut self,
    ) -> Option<Result<(Self::BiOut, Self::BiIn), anyhow::Error>>;
    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), anyhow::Error>;
    async fn read_datagram(&self) -> Result<bytes::Bytes, anyhow::Error>;

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

fn add_write_framing<S: tokio::io::AsyncWrite>(
    dest: S,
) -> MapErrAnyhowSink<
    bytes::Bytes,
    std::io::Error,
    tokio_util::codec::FramedWrite<S, tokio_util::codec::LengthDelimitedCodec>,
> {
    MapErrAnyhowSink::new(tokio_util::codec::FramedWrite::new(
        dest,
        tokio_util::codec::LengthDelimitedCodec::new(),
    ))
}

fn add_read_framing<S: tokio::io::AsyncRead>(
    src: S,
) -> MapErrAnyhowStream<
    bytes::BytesMut,
    std::io::Error,
    tokio_util::codec::FramedRead<S, tokio_util::codec::LengthDelimitedCodec>,
> {
    MapErrAnyhowStream::new(tokio_util::codec::FramedRead::new(
        src,
        tokio_util::codec::LengthDelimitedCodec::new(),
    ))
}

#[async_trait::async_trait]
impl Connection for webtransport_quinn::Session {
    type BiOut = MapErrAnyhowSink<
        bytes::Bytes,
        std::io::Error,
        tokio_util::codec::FramedWrite<
            webtransport_quinn::SendStream,
            tokio_util::codec::LengthDelimitedCodec,
        >,
    >;
    type BiIn = MapErrAnyhowStream<
        bytes::BytesMut,
        std::io::Error,
        tokio_util::codec::FramedRead<
            webtransport_quinn::RecvStream,
            tokio_util::codec::LengthDelimitedCodec,
        >,
    >;
    type Handle = WebConnectionHandle;

    async fn accept_bi_stream(
        &mut self,
    ) -> Option<Result<(Self::BiOut, Self::BiIn), anyhow::Error>> {
        match self.accept_bi().await {
            Err(err) => Some(Err(err.into())),
            Ok((send, recv)) => Some(Ok((add_write_framing(send), add_read_framing(recv)))),
        }
    }

    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), anyhow::Error> {
        let (send, recv) = self.open_bi().await?;

        Ok((add_write_framing(send), add_read_framing(recv)))
    }

    async fn read_datagram(&self) -> Result<bytes::Bytes, anyhow::Error> {
        self.read_datagram().await.map_err(Into::into)
    }

    fn create_handle(&self) -> Self::Handle {
        let (datagrams_tx, mut datagrams_rx) = tokio::sync::mpsc::unbounded_channel();

        {
            let session = self.clone();
            tokio::spawn(async move {
                while let Some(data) = datagrams_rx.recv().await {
                    // it's not actually doing any async things but maybe they expect to in the future?
                    if let Err(err) = session.send_datagram(data).await {
                        eprintln!("Failed to send datagram: {:?}", err);
                    }
                }
            });
        }

        WebConnectionHandle {
            datagrams_tx,
            session: self.clone(),
        }
    }
}

#[derive(Clone)]
pub struct WebConnectionHandle {
    datagrams_tx: tokio::sync::mpsc::UnboundedSender<bytes::Bytes>,
    session: webtransport_quinn::Session,
}

#[async_trait::async_trait]
impl ConnectionHandle for WebConnectionHandle {
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), anyhow::Error> {
        self.datagrams_tx.send(data)?;
        Ok(())
    }

    fn close(&self, code: u8) {
        self.session.close(
            code.into(),
            ni_ty::protocol::get_close_message(code).as_bytes(),
        );
    }
}
