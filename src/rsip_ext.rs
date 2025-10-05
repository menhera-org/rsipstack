use crate::rsip;
use crate::transport::SipConnection;
use crate::{Error, Result};
use rsip::{
    message::HasHeaders,
    prelude::{HeadersExt, UntypedHeader},
};
pub trait RsipResponseExt {
    fn reason_phrase(&self) -> Option<&str>;
    fn via_received(&self) -> Option<rsip::HostWithPort>;
    fn content_type(&self) -> Option<rsip::headers::ContentType>;
}

impl RsipResponseExt for rsip::Response {
    fn reason_phrase(&self) -> Option<&str> {
        let headers = self.headers();
        for header in headers.iter() {
            if let rsip::Header::Other(name, value) = header {
                if name.eq_ignore_ascii_case("reason") {
                    return Some(value);
                }
            }
            if let rsip::Header::ErrorInfo(reason) = header {
                return Some(reason.value());
            }
        }
        None
    }
    /// Parse the received address from the Via header
    ///
    /// This function extracts the received address from the Via header
    /// and returns it as a HostWithPort struct.
    fn via_received(&self) -> Option<rsip::HostWithPort> {
        let via = self.via_header().ok()?;
        SipConnection::parse_target_from_via(via)
            .map(|(_, host_with_port)| host_with_port)
            .ok()
    }
    fn content_type(&self) -> Option<rsip::headers::ContentType> {
        let headers = self.headers();
        for header in headers.iter() {
            if let rsip::Header::ContentType(content_type) = header {
                return Some(content_type.clone());
            }
        }
        None
    }
}

pub trait RsipHeadersExt {
    fn push_front(&mut self, header: rsip::Header);
}

impl RsipHeadersExt for rsip::Headers {
    fn push_front(&mut self, header: rsip::Header) {
        let mut headers = self.iter().cloned().collect::<Vec<_>>();
        headers.insert(0, header);
        *self = headers.into();
    }
}

#[macro_export]
macro_rules! header_pop {
    ($iter:expr, $header:path) => {
        let mut first = true;
        $iter.retain(|h| {
            if first && matches!(h, $header(_)) {
                first = false;
                false
            } else {
                true
            }
        });
    };
}

pub fn extract_uri_from_contact(line: &str) -> Result<rsip::Uri> {
    if let Ok(mut uri) = rsip::headers::Contact::from(line).uri() {
        uri.params
            .retain(|p| matches!(p, rsip::Param::Transport(_)));
        return Ok(uri);
    }

    match line.split('<').nth(1).and_then(|s| s.split('>').next()) {
        Some(uri) => rsip::Uri::try_from(uri).map_err(Into::into),
        None => Err(Error::Error("no uri found".to_string())),
    }
}

#[test]
fn test_rsip_headers_ext() {
    use rsip::{Header, Headers};
    let mut headers: Headers = vec![
        Header::Via("SIP/2.0/TCP".into()),
        Header::Via("SIP/2.0/UDP".into()),
        Header::Via("SIP/2.0/WSS".into()),
    ]
    .into();
    let via = Header::Via("SIP/2.0/TLS".into());
    headers.push_front(via);
    assert_eq!(headers.iter().count(), 4);

    header_pop!(headers, Header::Via);
    assert_eq!(headers.iter().count(), 3);

    assert_eq!(
        headers.iter().collect::<Vec<_>>(),
        vec![
            &Header::Via("SIP/2.0/TCP".into()),
            &Header::Via("SIP/2.0/UDP".into()),
            &Header::Via("SIP/2.0/WSS".into())
        ]
    );
}
