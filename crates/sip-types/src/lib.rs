//! Owned SIP message types.

use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SipMethod {
    Invite,
    Ack,
    Bye,
    Cancel,
    Options,
    Register,
    Refer,
    Notify,
    Info,
    Update,
    Prack,
    Other(String),
}

impl SipMethod {
    pub fn parse(token: &str) -> Option<Self> {
        if token.is_empty() || !token.bytes().all(is_token_byte) {
            return None;
        }
        Some(match token {
            "INVITE" => Self::Invite,
            "ACK" => Self::Ack,
            "BYE" => Self::Bye,
            "CANCEL" => Self::Cancel,
            "OPTIONS" => Self::Options,
            "REGISTER" => Self::Register,
            "REFER" => Self::Refer,
            "NOTIFY" => Self::Notify,
            "INFO" => Self::Info,
            "UPDATE" => Self::Update,
            "PRACK" => Self::Prack,
            other => Self::Other(other.to_owned()),
        })
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Invite => "INVITE",
            Self::Ack => "ACK",
            Self::Bye => "BYE",
            Self::Cancel => "CANCEL",
            Self::Options => "OPTIONS",
            Self::Register => "REGISTER",
            Self::Refer => "REFER",
            Self::Notify => "NOTIFY",
            Self::Info => "INFO",
            Self::Update => "UPDATE",
            Self::Prack => "PRACK",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl Display for SipMethod {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SipRequest {
    pub method: SipMethod,
    pub request_uri: String,
    pub version: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SipResponse {
    pub version: String,
    pub status_code: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SipMessage {
    Request(SipRequest),
    Response(SipResponse),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Headers(Vec<Header>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.push(Header {
            name: name.into(),
            value: value.into(),
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &Header> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'..=b'\'' | b'*'..=b'+' | b'-'..=b'.' | b'^' | b'_' | b'|' | b'~'
        )
        || byte == 96
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_extension_methods_round_trip() {
        assert_eq!(SipMethod::parse("INVITE"), Some(SipMethod::Invite));
        assert_eq!(SipMethod::parse("X-CUSTOM").unwrap().as_str(), "X-CUSTOM");
        assert!(SipMethod::parse("bad method").is_none());
    }

    #[test]
    fn headers_are_case_insensitive_and_preserve_duplicates() {
        let mut headers = Headers::new();
        headers.push("Via", "one");
        headers.push("vIa", "two");
        assert_eq!(headers.get("VIA"), Some("one"));
        assert_eq!(
            headers.get_all("via").collect::<Vec<_>>(),
            vec!["one", "two"]
        );
    }
}
