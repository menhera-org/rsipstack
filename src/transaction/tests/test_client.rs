use crate::transaction::key::{TransactionKey, TransactionRole};
use crate::transaction::transaction::Transaction;
use crate::transport::udp::UdpConnection;
use crate::{transport::TransportEvent, Result};
use crate::rsip;
use rsip::{headers::*, SipMessage};
use std::time::Duration;
use tokio::{select, sync::mpsc::unbounded_channel, time::sleep};
use tracing::info;

#[tokio::test]
async fn test_client_transaction() -> Result<()> {
    let endpoint = super::create_test_endpoint(Some("127.0.0.1:0")).await?;
    let server_addr = endpoint
        .get_addrs()
        .get(0)
        .expect("must has connection")
        .to_owned();
    info!("server addr: {}", server_addr);

    let peer_server = UdpConnection::create_connection("127.0.0.1:0".parse()?, None, None).await?;
    let peer_server_loop = async {
        let (sender, mut recevier) = unbounded_channel();
        select! {
            _ = async {
                if let Some(event) = recevier.recv().await {
                    match event {
                        TransportEvent::Incoming(msg, connection, _) => {
                            info!("recv request: {}", msg);
                            assert!(msg.is_request());
                            match msg {
                                SipMessage::Request(req) => {
                                    let headers = req.headers.clone();
                                    let response = SipMessage::Response(rsip::message::Response {
                                        version: rsip::Version::V2,
                                        status_code:rsip::StatusCode::Trying,
                                        headers: headers.clone(),
                                        body: Default::default(),
                                    });
                                    connection.send(response, None).await.expect("send trying");
                                    sleep(Duration::from_millis(100)).await;

                                    let response = SipMessage::Response(rsip::message::Response {
                                        version: rsip::Version::V2,
                                        status_code:rsip::StatusCode::OK,
                                        headers,
                                        body: Default::default(),
                                    });
                                    connection.send(response, None).await.expect("send Ok");
                                    sleep(Duration::from_secs(1)).await;
                                }
                                _ => {
                                    assert!(false, "must not reach here");
                                }
                            }
                        }
                        _ => {}
                    }
                } else {
                    assert!(false, "must not reach here");
                }
            } => {}
            _ = peer_server.serve_loop(sender) => {
                assert!(false, "must not reach here");
            }
        }
    };

    let recv_loop = async {
        let register_req = rsip::message::Request {
            method: rsip::method::Method::Register,
            uri: rsip::Uri {
                scheme: Some(rsip::Scheme::Sip),
                host_with_port: peer_server.get_addr().addr.clone(),
                ..Default::default()
            },
            headers: vec![
                Via::new("SIP/2.0/TLS restsend.com:5061;branch=z9hG4bKnashd92").into(),
                CSeq::new("1 REGISTER").into(),
                From::new("Bob <sips:bob@restsend.com>;tag=ja743ks76zlflH").into(),
                CallId::new("1j9FpLxk3uxtm8tn@restsend.com").into(),
            ]
            .into(),
            version: rsip::Version::V2,
            body: Default::default(),
        };

        let key = TransactionKey::from_request(&register_req, TransactionRole::Client)
            .expect("client_transaction");
        let mut tx = Transaction::new_client(key, register_req, endpoint.inner.clone(), None);
        tx.send().await.expect("send request");

        while let Some(resp) = tx.receive().await {
            info!("Received response: {:?}", resp);
        }
    };

    select! {
        _ = recv_loop => {}
        _ = peer_server_loop => {
            assert!(false, "must not reach here");
        }
        _ = endpoint.serve() => {
            assert!(false, "must not reach here");
        }
        _ = sleep(Duration::from_secs(1)) => {
            assert!(false, "timeout waiting");
        }
    }
    Ok(())
}
