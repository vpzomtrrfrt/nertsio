use futures_util::{Stream, StreamExt};
use nertsio_types as ni_ty;

pub trait ConnectionHandle {
    type SendDatagramError: Into<anyhow::Error> + Sync + Send;
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), Self::SendDatagramError>;
    fn close(&self, code: u8);
}

pub struct AnyhowErrorConnectionHandleWrapper<
    E: Into<anyhow::Error> + Sync + Send + 'static,
    T: ConnectionHandle<SendDatagramError = E>,
>(pub T);

impl<
        E: Into<anyhow::Error> + Sync + Send + 'static,
        T: ConnectionHandle<SendDatagramError = E>,
    > ConnectionHandle for AnyhowErrorConnectionHandleWrapper<E, T>
{
    type SendDatagramError = anyhow::Error;

    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), Self::SendDatagramError> {
        self.0.send_datagram(data).map_err(Into::into)
    }

    fn close(&self, code: u8) {
        self.0.close(code);
    }
}

#[async_trait::async_trait]
pub trait Connection {
    type Error: std::error::Error + Sync + Send + 'static;
    type SendDatagramError: std::error::Error + Sync + Send + 'static;

    type BiOut: tokio::io::AsyncWrite + Unpin + Send;
    type BiIn: tokio::io::AsyncRead + Unpin + Send;
    type DatagramsIn: Stream<Item = Result<bytes::Bytes, Self::Error>> + Send;
    type Handle: ConnectionHandle<SendDatagramError = Self::SendDatagramError>
        + Sync
        + Send
        + 'static;

    async fn accept_bi_stream(&mut self) -> Option<Result<(Self::BiOut, Self::BiIn), Self::Error>>;
    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), Self::Error>;

    fn into_datagrams(self) -> Self::DatagramsIn;
    fn create_handle(&self) -> Self::Handle;
}

#[async_trait::async_trait]
impl Connection for quinn::NewConnection {
    type Error = quinn::ConnectionError;
    type SendDatagramError = quinn::SendDatagramError;

    type BiOut = quinn::SendStream;
    type BiIn = quinn::RecvStream;
    type DatagramsIn = quinn::Datagrams;
    type Handle = quinn::Connection;

    async fn accept_bi_stream(&mut self) -> Option<Result<(Self::BiOut, Self::BiIn), Self::Error>> {
        self.bi_streams.next().await
    }

    async fn start_bi_stream(&mut self) -> Result<(Self::BiOut, Self::BiIn), Self::Error> {
        self.connection.open_bi().await
    }

    fn into_datagrams(self) -> Self::DatagramsIn {
        self.datagrams
    }

    fn create_handle(&self) -> Self::Handle {
        self.connection.clone()
    }
}

impl ConnectionHandle for quinn::Connection {
    type SendDatagramError = quinn::SendDatagramError;

    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), Self::SendDatagramError> {
        self.send_datagram(data)
    }

    fn close(&self, code: u8) {
        self.close(
            code.into(),
            ni_ty::protocol::get_close_message(code).as_bytes(),
        );
    }
}
