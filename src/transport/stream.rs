use crate::rsip;
use crate::{
    transport::{
        connection::{TransportSender, KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE},
        SipAddr, SipConnection, TransportEvent,
    },
    Result,
};
use bytes::{Buf, BytesMut};
use rsip::SipMessage;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::Mutex,
};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, info, warn};

pub(super) const MAX_SIP_MESSAGE_SIZE: usize = 65535;

pub struct SipCodec {}

impl SipCodec {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for SipCodec {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum SipCodecType {
    Message(SipMessage),
    KeepaliveRequest,
    KeepaliveResponse,
}

impl std::fmt::Display for SipCodecType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SipCodecType::Message(msg) => write!(f, "{}", msg),
            SipCodecType::KeepaliveRequest => write!(f, "Keepalive Request"),
            SipCodecType::KeepaliveResponse => write!(f, "Keepalive Response"),
        }
    }
}

impl Decoder for SipCodec {
    type Item = SipCodecType;
    type Error = crate::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if src.len() >= 4 && &src[0..4] == KEEPALIVE_REQUEST {
            src.advance(4);
            return Ok(Some(SipCodecType::KeepaliveRequest));
        }

        if src.len() >= 2 && &src[0..2] == KEEPALIVE_RESPONSE {
            src.advance(2);
            return Ok(Some(SipCodecType::KeepaliveResponse));
        }

        if let Some(end_pos) = src
            .windows(KEEPALIVE_REQUEST.len())
            .position(|window| window == KEEPALIVE_REQUEST)
        {
            let msg_end = end_pos + KEEPALIVE_REQUEST.len();
            let msg_data = &src[..msg_end];
            match SipMessage::try_from(msg_data) {
                Ok(msg) => {
                    src.advance(msg_end);
                    Ok(Some(SipCodecType::Message(msg)))
                }
                Err(e) => {
                    src.advance(msg_end);
                    Err(crate::Error::Error(format!(
                        "Failed to parse SIP message: {}",
                        e
                    )))
                }
            }
        } else {
            if src.len() > MAX_SIP_MESSAGE_SIZE {
                return Err(crate::Error::Error("SIP message too large".to_string()));
            }
            Ok(None)
        }
    }
}

impl Encoder<SipMessage> for SipCodec {
    type Error = crate::Error;

    fn encode(&mut self, item: SipMessage, dst: &mut BytesMut) -> Result<()> {
        let data = item.to_string();
        dst.extend_from_slice(data.as_bytes());
        Ok(())
    }
}

pub struct StreamConnectionInner<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub local_addr: SipAddr,
    pub remote_addr: SipAddr,
    pub read_half: Mutex<Option<R>>,
    pub write_half: Mutex<W>,
}

impl<R, W> StreamConnectionInner<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub fn new(local_addr: SipAddr, remote_addr: SipAddr, read_half: R, write_half: W) -> Self {
        Self {
            local_addr,
            remote_addr,
            read_half: Mutex::new(Some(read_half)),
            write_half: Mutex::new(write_half),
        }
    }

    pub async fn send_message(&self, msg: SipMessage) -> Result<()> {
        send_to_stream(&self.write_half, msg).await
    }

    pub async fn send_raw(&self, data: &[u8]) -> Result<()> {
        send_raw_to_stream(&self.write_half, data).await
    }

    pub async fn serve_loop(
        &self,
        sender: TransportSender,
        connection: SipConnection,
    ) -> Result<()> {
        let mut read_half = match self.read_half.lock().await.take() {
            Some(read_half) => read_half,
            None => {
                warn!("Connection closed");
                return Ok(());
            }
        };

        let remote_addr = self.remote_addr.clone();

        let mut codec = SipCodec::new();
        let mut buffer = BytesMut::with_capacity(MAX_SIP_MESSAGE_SIZE);
        let mut read_buf = [0u8; MAX_SIP_MESSAGE_SIZE];

        loop {
            use tokio::io::AsyncReadExt;
            match read_half.read(&mut read_buf).await {
                Ok(0) => {
                    info!("Connection closed: {}", self.local_addr);
                    break;
                }
                Ok(n) => {
                    buffer.extend_from_slice(&read_buf[0..n]);

                    loop {
                        match codec.decode(&mut buffer) {
                            Ok(Some(msg)) => match msg {
                                SipCodecType::Message(sip_msg) => {
                                    debug!("Received message from {}: {}", remote_addr, sip_msg);
                                    let remote_socket_addr = remote_addr.get_socketaddr()?;
                                    let sip_msg = SipConnection::update_msg_received(
                                        sip_msg,
                                        remote_socket_addr,
                                        remote_addr.r#type.unwrap_or_default(),
                                    )?;

                                    if let Err(e) = sender.send(TransportEvent::Incoming(
                                        sip_msg,
                                        connection.clone(),
                                        remote_addr.clone(),
                                    )) {
                                        warn!("Error sending incoming message: {:?}", e);
                                        return Err(e.into());
                                    }
                                }
                                SipCodecType::KeepaliveRequest => {
                                    self.send_raw(KEEPALIVE_RESPONSE).await?;
                                }
                                SipCodecType::KeepaliveResponse => {}
                            },
                            Ok(None) => {
                                // Need more data
                                break;
                            }
                            Err(e) => {
                                warn!("Error decoding message from {}: {:?}", remote_addr, e);
                                // Continue processing despite decode errors
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Error reading from stream: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        let mut write_half = self.write_half.lock().await;
        write_half
            .shutdown()
            .await
            .map_err(|e| crate::Error::Error(format!("Failed to shutdown write half: {}", e)))?;
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait StreamConnection: Send + Sync + 'static {
    fn get_addr(&self) -> &SipAddr;
    async fn send_message(&self, msg: SipMessage) -> Result<()>;
    async fn send_raw(&self, data: &[u8]) -> Result<()>;
    async fn serve_loop(&self, sender: TransportSender) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub async fn send_to_stream<W>(write_half: &Mutex<W>, msg: SipMessage) -> Result<()>
where
    W: AsyncWrite + Unpin + Send,
{
    send_raw_to_stream(write_half, msg.to_string().as_bytes()).await
}

pub async fn send_raw_to_stream<W>(write_half: &Mutex<W>, data: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin + Send,
{
    let mut lock = write_half.lock().await;
    lock.write_all(data).await?;
    lock.flush().await?;
    Ok(())
}
