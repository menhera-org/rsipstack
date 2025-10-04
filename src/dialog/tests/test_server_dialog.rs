use crate::rsip;
use rsip::prelude::{HeadersExt, ToTypedHeader};
use tokio::sync::mpsc::unbounded_channel;

use crate::{
    dialog::{
        dialog::DialogInner,
        tests::test_dialog_states::{create_invite_request, create_test_endpoint},
        DialogId,
    },
    transaction::key::TransactionRole,
};

#[tokio::test]
async fn test_dialog_make_request() -> crate::Result<()> {
    // Create dialog ID
    let dialog_id = DialogId {
        call_id: "test-call-id-123".to_string(),
        from_tag: "alice-tag-456".to_string(),
        to_tag: "bob-tag-789".to_string(),
    };

    let endpoint = create_test_endpoint().await?;
    let (tu_sender, _tu_receiver) = unbounded_channel();
    let (state_sender, _state_receiver) = unbounded_channel();
    // Create INVITE request
    let invite_req = create_invite_request("alice-tag-456", "", "test-call-id-123");
    // Create dialog inner
    let dialog_inner = DialogInner::new(
        TransactionRole::Client,
        dialog_id.clone(),
        invite_req.clone(),
        endpoint.inner.clone(),
        state_sender,
        None,
        Some(rsip::Uri::try_from("sip:alice@alice.example.com:5060")?),
        tu_sender,
    )
    .expect("Failed to create dialog inner");

    let bye = dialog_inner
        .make_request_with_vias(
            rsip::Method::Bye,
            None,
            dialog_inner
                .build_vias_from_request()
                .expect("Failed to build vias"),
            None,
            None,
        )
        .expect("Failed to make request");
    assert_eq!(bye.method, rsip::Method::Bye);

    assert_eq!(
        bye.via_header()
            .expect("not via header")
            .typed()?
            .received()?,
        "172.0.0.1".parse().ok()
    );
    assert!(
        bye.via_header().expect("not via header").typed()?.branch()
            != invite_req
                .via_header()
                .expect("not via header")
                .typed()?
                .branch()
    );
    Ok(())
}
