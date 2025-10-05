use super::{connection::TransportSender, sip_addr::SipAddr, tcp::TcpConnection, SipConnection};
use crate::rsip;
use crate::transaction::key::TransactionKey;
use crate::transport::connection::TransportReceiver;
use crate::{transport::TransportEvent, Result};
use std::sync::{Mutex, RwLock};
use std::{collections::HashMap, sync::Arc};
use tokio::select;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub struct TransportLayerInner {
    pub(crate) cancel_token: CancellationToken,
    listens: Arc<RwLock<Vec<SipConnection>>>, // listening transports
    connections: Arc<RwLock<HashMap<SipAddr, SipConnection>>>, // outbound/inbound connections
    pub(crate) transport_tx: TransportSender,
    pub(crate) transport_rx: Mutex<Option<TransportReceiver>>,
}
pub(crate) type TransportLayerInnerRef = Arc<TransportLayerInner>;

pub struct TransportLayer {
    pub outbound: Option<SipAddr>,
    pub inner: TransportLayerInnerRef,
}

impl TransportLayer {
    pub fn new(cancel_token: CancellationToken) -> Self {
        let (transport_tx, transport_rx) = mpsc::unbounded_channel();
        let inner = TransportLayerInner {
            cancel_token,
            listens: Arc::new(RwLock::new(Vec::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            transport_tx,
            transport_rx: Mutex::new(Some(transport_rx)),
        };
        Self {
            outbound: None,
            inner: Arc::new(inner),
        }
    }

    pub fn add_transport(&self, transport: SipConnection) {
        self.inner.add_listener(transport)
    }

    pub fn del_transport(&self, addr: &SipAddr) {
        self.inner.del_listener(addr)
    }

    pub fn add_connection(&self, connection: SipConnection) {
        self.inner.add_connection(connection);
    }

    pub fn del_connection(&self, addr: &SipAddr) {
        self.inner.del_connection(addr)
    }

    pub async fn lookup(
        &self,
        target: &SipAddr,
        key: Option<&TransactionKey>,
    ) -> Result<(SipConnection, SipAddr)> {
        self.inner.lookup(target, self.outbound.as_ref(), key).await
    }

    pub async fn serve_listens(&self) -> Result<()> {
        // Collect the listeners under the read lock and drop it before awaiting.
        let listens: Vec<SipConnection> = match self.inner.listens.read() {
            Ok(listens_guard) => listens_guard.iter().cloned().collect(),
            Err(e) => {
                return Err(crate::Error::Error(format!(
                    "Failed to read listens: {:?}",
                    e
                )));
            }
        };

        for transport in listens {
            let inner = self.inner.clone();
            let addr = transport.get_addr().clone();

            tokio::spawn(async move {
                if let Err(e) = TransportLayerInner::serve_listener(inner, transport).await {
                    warn!(?addr, "Failed to serve listener: {:?}", e);
                }
            });
        }

        Ok(())
    }

    pub fn get_addrs(&self) -> Vec<SipAddr> {
        match self.inner.listens.read() {
            Ok(listens) => listens.iter().map(|t| t.get_addr().to_owned()).collect(),
            Err(e) => {
                warn!("Failed to read listens: {:?}", e);
                Vec::new()
            }
        }
    }
}

impl TransportLayerInner {
    pub(super) fn add_listener(&self, connection: SipConnection) {
        match self.listens.write() {
            Ok(mut listens) => {
                listens.push(connection);
            }
            Err(e) => {
                warn!("Failed to write listens: {:?}", e);
            }
        }
    }

    pub(super) fn del_listener(&self, addr: &SipAddr) {
        match self.listens.write() {
            Ok(mut listens) => {
                listens.retain(|t| t.get_addr() != addr);
            }
            Err(e) => {
                warn!("Failed to write listens: {} {:?}", addr, e);
            }
        }
    }

    pub(super) fn add_connection(&self, connection: SipConnection) {
        match self.connections.write() {
            Ok(mut connections) => {
                connections.insert(connection.get_addr().to_owned(), connection.clone());
                self.serve_connection(connection);
            }
            Err(e) => {
                warn!("Failed to write connections: {:?}", e);
            }
        }
    }

    pub(super) fn del_connection(&self, addr: &SipAddr) {
        match self.connections.write() {
            Ok(mut connections) => {
                connections.remove(addr);
            }
            Err(e) => {
                warn!("Failed to write connections: {} {:?}", addr, e);
            }
        }
    }

    async fn lookup(
        &self,
        destination: &SipAddr,
        outbound: Option<&SipAddr>,
        key: Option<&TransactionKey>,
    ) -> Result<(SipConnection, SipAddr)> {
        let target = outbound.cloned().unwrap_or_else(|| destination.clone());

        debug!(?key, "lookup target: {} -> {}", destination, target);
        match self.connections.read() {
            Ok(connections) => {
                if let Some(transport) = connections.get(&target) {
                    return Ok((transport.clone(), target.clone()));
                }
            }
            Err(e) => {
                warn!("Failed to read connections: {:?}", e);
                return Err(crate::Error::Error(format!(
                    "Failed to read connections: {:?}",
                    e
                )));
            }
        }

        if let Some(transport) = target.r#type.clone() {
            match transport {
                rsip::transport::Transport::Tcp => {
                    let connection =
                        TcpConnection::connect(&target, Some(self.cancel_token.child_token()))
                            .await?;
                    let sip_connection = SipConnection::Tcp(connection);
                    self.add_connection(sip_connection.clone());
                    return Ok((sip_connection, target));
                }
                rsip::transport::Transport::Udp => {}
                other => {
                    return Err(crate::Error::TransportLayerError(
                        format!("unsupported transport type: {:?}", other),
                        target.clone(),
                    ));
                }
            }
        }

        let listens = match self.listens.read() {
            Ok(listens) => listens,
            Err(e) => {
                return Err(crate::Error::Error(format!(
                    "Failed to read listens: {:?}",
                    e
                )));
            }
        };
        let mut first_udp = None;
        for transport in listens.iter() {
            let addr = transport.get_addr();
            if addr.r#type == Some(rsip::transport::Transport::Udp) && first_udp.is_none() {
                first_udp = Some(transport.clone());
            }
            if addr == &target {
                return Ok((transport.clone(), target.clone()));
            }
        }
        if let Some(transport) = first_udp {
            return Ok((transport, target.clone()));
        }
        Err(crate::Error::TransportLayerError(
            format!("unsupported transport type: {:?}", target.r#type),
            target.to_owned(),
        ))
    }

    pub(super) async fn serve_listener(self: Arc<Self>, transport: SipConnection) -> Result<()> {
        let sender = self.transport_tx.clone();
        match transport {
            SipConnection::Udp(transport) => {
                tokio::spawn(async move { transport.serve_loop(sender).await });
                Ok(())
            }
            SipConnection::TcpListener(connection) => connection.serve_listener(self.clone()).await,
            _ => {
                warn!(
                    "serve_listener: unsupported transport type: {:?}",
                    transport.get_addr()
                );
                Ok(())
            }
        }
    }

    pub fn serve_connection(&self, transport: SipConnection) {
        let sub_token = self.cancel_token.child_token();
        let sender_clone = self.transport_tx.clone();
        tokio::spawn(async move {
            match sender_clone.send(TransportEvent::New(transport.clone())) {
                Ok(()) => {}
                Err(e) => {
                    warn!(addr=%transport.get_addr(), "Error sending new connection event: {:?}", e);
                    return;
                }
            }
            select! {
                _ = sub_token.cancelled() => { }
                _ = transport.serve_loop(sender_clone.clone()) => {
                }
            }
            info!(addr=%transport.get_addr(), "transport serve_loop exited");
            sender_clone.send(TransportEvent::Closed(transport)).ok();
        });
    }
}
impl Drop for TransportLayer {
    fn drop(&mut self) {
        self.inner.cancel_token.cancel();
    }
}
#[cfg(test)]
mod tests {
    use crate::rsip;
    use crate::{
        transport::{udp::UdpConnection, SipAddr},
        Result,
    };
    #[tokio::test]
    async fn test_lookup() -> Result<()> {
        let mut tl = super::TransportLayer::new(tokio_util::sync::CancellationToken::new());

        let first_uri = SipAddr {
            r#type: Some(rsip::transport::Transport::Udp),
            addr: rsip::HostWithPort {
                host: rsip::Host::IpAddr("127.0.0.1".parse()?),
                port: Some(5060.into()),
            },
        };
        assert!(tl.lookup(&first_uri, None).await.is_err());
        let udp_peer = UdpConnection::create_connection(
            "127.0.0.1:0".parse()?,
            None,
            Some(tl.inner.cancel_token.child_token()),
        )
        .await?;
        let udp_peer_addr = udp_peer.get_addr().to_owned();
        tl.add_transport(udp_peer.into());

        let (target, _) = tl.lookup(&first_uri, None).await?;
        assert_eq!(target.get_addr(), &udp_peer_addr);

        // test outbound
        let outbound_peer = UdpConnection::create_connection(
            "127.0.0.1:0".parse()?,
            None,
            Some(tl.inner.cancel_token.child_token()),
        )
        .await?;
        let outbound = outbound_peer.get_addr().to_owned();
        tl.add_transport(outbound_peer.into());
        tl.outbound = Some(outbound.clone());

        // must return the outbound transport
        let (target, _) = tl.lookup(&first_uri, None).await?;
        assert_eq!(target.get_addr(), &outbound);
        Ok(())
    }

    #[tokio::test]
    async fn test_serve_listens() -> Result<()> {
        let tl = super::TransportLayer::new(tokio_util::sync::CancellationToken::new());

        // Add a UDP connection first
        let udp_conn = UdpConnection::create_connection(
            "127.0.0.1:0".parse()?,
            None,
            Some(tl.inner.cancel_token.child_token()),
        )
        .await?;
        let addr = udp_conn.get_addr().clone();
        tl.add_transport(udp_conn.into());

        // Start serving listeners
        tl.serve_listens().await?;

        // Verify that the transport list is not empty
        let addrs = tl.get_addrs();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], addr);

        // Cancel to stop the spawned tasks
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        drop(tl);

        Ok(())
    }
}
