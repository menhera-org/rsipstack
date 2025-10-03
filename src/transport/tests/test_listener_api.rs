use crate::{
    transport::{SipAddr, TcpListenerConnection},
    Result,
};

/// Test new TcpListenerConnection API
#[tokio::test]
async fn test_tcp_listener_connection_api() -> Result<()> {
    // Create TCP listener connection with a specific port to avoid conflicts
    let socket_addr: std::net::SocketAddr = "127.0.0.1:0".parse()?;
    let local_addr = SipAddr::new(rsip::transport::Transport::Tcp, socket_addr.into());
    let tcp_listener = TcpListenerConnection::new(local_addr, None).await?;

    // Get the address (should be the same as input since we don't bind in new())
    let bound_addr = tcp_listener.get_addr().clone();

    println!(
        "Successfully created TCP listener using new API: {:?}",
        bound_addr
    );

    // Test that we can get the address
    assert_eq!(bound_addr.r#type, Some(rsip::transport::Transport::Tcp));
    assert_eq!(bound_addr.addr.host.to_string(), "127.0.0.1");

    Ok(())
}
