use bytes::BufMut;
use futures_util::{Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use nertsio_types as ni_ty;
use std::collections::HashMap;
use std::sync::Arc;

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

pin_project_lite::pin_project! {
    pub struct AddErrAnyhowStream<T, R: Stream<Item=T>> {
        #[pin]
        src: R,
    }
}

impl<T, R: Stream<Item = T>> AddErrAnyhowStream<T, R> {
    pub fn new(src: R) -> Self {
        AddErrAnyhowStream { src }
    }
}

impl<T, R: Stream<Item = T>> Stream for AddErrAnyhowStream<T, R> {
    type Item = Result<T, anyhow::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.project().src.poll_next(cx) {
            std::task::Poll::Ready(Some(item)) => std::task::Poll::Ready(Some(Ok(item))),
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

pub struct WSBiOut {
    dest: tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::protocol::Message>,
    id: i8,
}

impl Sink<bytes::Bytes> for WSBiOut {
    type Error = anyhow::Error;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: bytes::Bytes) -> Result<(), Self::Error> {
        let this = self.as_ref();

        let mut out = bytes::BytesMut::new();
        out.extend_from_slice(&item);
        out.put_i8(this.id);

        this.dest.send(out.as_ref().into())?;
        Ok(())
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

struct WSConnectionInner {
    bi_streams: HashMap<
        i8,
        (
            Option<tokio::sync::mpsc::Receiver<bytes::BytesMut>>,
            tokio::sync::mpsc::Sender<bytes::BytesMut>,
        ),
    >,
    out_send: tokio::sync::mpsc::UnboundedSender<tokio_tungstenite::tungstenite::protocol::Message>,
    my_last_stream_id: i8,
    their_last_stream_id: i8,
}

#[derive(Clone)]
pub struct WSConnectionHandle {
    inner: Arc<tokio::sync::Mutex<WSConnectionInner>>,
}

#[async_trait::async_trait]
impl ConnectionHandle for WSConnectionHandle {
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), anyhow::Error> {
        let mut dest = Vec::with_capacity(data.len() + 1);
        dest.extend_from_slice(&data);
        dest.push(0);

        let msg = dest.into();

        if let Ok(mut lock) = self.inner.try_lock() {
            let inner = &mut *lock;

            let _ = inner.out_send.send(msg); // drop if full
        }

        Ok(())
    }

    fn close(&self, code: u8) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut lock = inner.lock().await;
            let inner = &mut *lock;

            if let Err(err) = inner
                .out_send
                .send(tokio_tungstenite::tungstenite::Message::Close(Some(
                    tokio_tungstenite::tungstenite::protocol::frame::CloseFrame {
                        code: (u16::from(code) + 4000).into(),
                        reason: ni_ty::protocol::get_close_message(code).into(),
                    },
                )))
            {
                eprintln!("Failed to send close message: {:?}", err);
            }
        });
    }
}

pub struct WSConnection {
    inner: Arc<tokio::sync::Mutex<WSConnectionInner>>,
    datagrams: tokio_stream::wrappers::ReceiverStream<bytes::Bytes>,
}

impl WSConnection {
    pub fn init(
        stream: tokio_tungstenite::WebSocketStream<
            tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        >,
    ) -> Self {
        let (datagrams_recv_send, datagrams_recv_recv) =
            tokio::sync::mpsc::channel::<bytes::Bytes>(2);
        let (out_send, mut out_recv) = tokio::sync::mpsc::unbounded_channel();

        let inner = Arc::new(tokio::sync::Mutex::new(WSConnectionInner {
            bi_streams: HashMap::new(),
            my_last_stream_id: 0,
            their_last_stream_id: 0,
            out_send,
        }));

        tokio::spawn({
            let inner = inner.clone();
            async move {
                let (mut stream_send, stream_recv) = stream.split();

                if let Err(err) = futures_util::try_join!(
                    async move {
                        while let Some(msg) = out_recv.recv().await {
                            stream_send.send(msg).await?;
                        }
                        anyhow::Result::Ok(())
                    },
                    stream_recv
                        .map_err(anyhow::Error::from)
                        .try_for_each(|msg| {
                            let inner = inner.clone();
                            let datagrams_recv_send = datagrams_recv_send.clone();
                            async move {
                                if let tokio_tungstenite::tungstenite::protocol::Message::Binary(
                                    mut data,
                                ) = msg
                                {
                                    let id = data.pop();
                                    match id {
                                        Some(0) => {
                                            let _ = datagrams_recv_send.try_send(data.into());
                                            //drop if full
                                        }
                                        Some(id) => {
                                            let id = i8::from_ne_bytes([id]);
                                            let mut out = bytes::BytesMut::new();
                                            out.extend_from_slice(&data);

                                            let mut lock = inner.lock().await;
                                            let inner = &mut *lock;

                                            if let Some((_, send)) = inner.bi_streams.get_mut(&id) {
                                                send.send(out).await?;
                                            }
                                        }
                                        None => {
                                            eprintln!("received empty message");
                                        }
                                    }
                                }

                                Ok(())
                            }
                        }),
                ) {
                    eprintln!("Error in connection handling: {:?}", err);
                }
            }
        });

        Self {
            inner,
            datagrams: tokio_stream::wrappers::ReceiverStream::new(datagrams_recv_recv),
        }
    }
}

const WS_BI_CHANNEL_CAPACITY: usize = 8;

#[async_trait::async_trait]
impl Connection for WSConnection {
    type BiIn = AddErrAnyhowStream<
        bytes::BytesMut,
        tokio_stream::wrappers::ReceiverStream<bytes::BytesMut>,
    >;
    type BiOut = WSBiOut;
    type DatagramsIn =
        AddErrAnyhowStream<bytes::Bytes, tokio_stream::wrappers::ReceiverStream<bytes::Bytes>>;
    type Handle = WSConnectionHandle;

    async fn accept_bi_stream(
        &mut self,
    ) -> Option<Result<(Self::BiOut, Self::BiIn), anyhow::Error>> {
        let (send, recv) = {
            let mut lock = self.inner.lock().await;
            let inner = &mut *lock;

            inner.their_last_stream_id += 1;
            let id = inner.their_last_stream_id;

            let recv_recv = if let Some((ref mut recv_recv, _)) = inner.bi_streams.get_mut(&id) {
                recv_recv.take().unwrap() // shouldn't be possible to hit more than once
            } else {
                let (recv_send, recv_recv) = tokio::sync::mpsc::channel(WS_BI_CHANNEL_CAPACITY);
                inner.bi_streams.insert(id, (None, recv_send));
                recv_recv
            };

            let send_send = WSBiOut {
                dest: inner.out_send.clone(),
                id,
            };

            (send_send, recv_recv)
        };

        Some(Ok((
            send,
            AddErrAnyhowStream::new(tokio_stream::wrappers::ReceiverStream::new(recv)),
        )))
    }

    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), anyhow::Error> {
        let (send, recv) = {
            let mut lock = self.inner.lock().await;
            let inner = &mut *lock;

            inner.my_last_stream_id -= 1;
            let id = inner.my_last_stream_id;

            let recv_recv = if let Some((ref mut recv_recv, _)) = inner.bi_streams.get_mut(&id) {
                recv_recv.take().unwrap() // shouldn't be possible to hit more than once
            } else {
                let (recv_send, recv_recv) = tokio::sync::mpsc::channel(WS_BI_CHANNEL_CAPACITY);
                inner.bi_streams.insert(id, (None, recv_send));
                recv_recv
            };

            let send_send = WSBiOut {
                dest: inner.out_send.clone(),
                id,
            };

            (send_send, recv_recv)
        };

        Ok((
            send,
            AddErrAnyhowStream::new(tokio_stream::wrappers::ReceiverStream::new(recv)),
        ))
    }

    fn into_datagrams(self) -> Self::DatagramsIn {
        AddErrAnyhowStream::new(self.datagrams)
    }

    fn create_handle(&self) -> Self::Handle {
        WSConnectionHandle {
            inner: self.inner.clone(),
        }
    }
}
