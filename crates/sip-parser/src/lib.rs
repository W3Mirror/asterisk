//! Bounded, non-panicking SIP/2.0 message parser and serializer.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    str,
};

use sip_types::{Headers, SipMessage, SipMethod, SipRequest, SipResponse};

#[derive(Clone, Copy, Debug)]
pub struct ParseConfig {
    pub max_message_bytes: usize,
    pub max_headers: usize,
    pub max_header_bytes: usize,
    pub max_uri_bytes: usize,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: 65_535,
            max_headers: 128,
            max_header_bytes: 8_192,
            max_uri_bytes: 4_096,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    EmptyMessage,
    MessageTooLarge { actual: usize, maximum: usize },
    MissingHeaderDelimiter,
    InvalidUtf8,
    InvalidStartLine,
    InvalidMethod,
    InvalidVersion,
    InvalidUri,
    TooManyHeaders { maximum: usize },
    HeaderTooLong { maximum: usize },
    InvalidHeader,
    InvalidHeaderValue,
    InvalidContentLength,
    BodyLengthMismatch { declared: usize, available: usize },
}

impl Display for ParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => formatter.write_str("SIP message is empty"),
            Self::MessageTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "SIP message is {actual} bytes, maximum is {maximum}"
                )
            }
            Self::MissingHeaderDelimiter => formatter.write_str("SIP header delimiter is missing"),
            Self::InvalidUtf8 => formatter.write_str("SIP headers are not valid UTF-8"),
            Self::InvalidStartLine => formatter.write_str("SIP start line is invalid"),
            Self::InvalidMethod => formatter.write_str("SIP method is invalid"),
            Self::InvalidVersion => formatter.write_str("SIP version is invalid"),
            Self::InvalidUri => formatter.write_str("SIP request URI is invalid"),
            Self::TooManyHeaders { maximum } => {
                write!(formatter, "SIP message exceeds the {maximum}-header limit")
            }
            Self::HeaderTooLong { maximum } => {
                write!(formatter, "SIP header exceeds the {maximum}-byte limit")
            }
            Self::InvalidHeader => formatter.write_str("SIP header is invalid"),
            Self::InvalidHeaderValue => {
                formatter.write_str("SIP header value contains a control byte")
            }
            Self::InvalidContentLength => formatter.write_str("SIP Content-Length is invalid"),
            Self::BodyLengthMismatch {
                declared,
                available,
            } => write!(
                formatter,
                "SIP body declares {declared} bytes but {available} bytes are available"
            ),
        }
    }
}

impl Error for ParseError {}

pub fn parse(input: &[u8]) -> Result<SipMessage, ParseError> {
    parse_with_config(input, ParseConfig::default())
}

pub fn parse_with_config(input: &[u8], config: ParseConfig) -> Result<SipMessage, ParseError> {
    if input.is_empty() {
        return Err(ParseError::EmptyMessage);
    }
    if input.len() > config.max_message_bytes {
        return Err(ParseError::MessageTooLarge {
            actual: input.len(),
            maximum: config.max_message_bytes,
        });
    }

    let delimiter = input
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(ParseError::MissingHeaderDelimiter)?;
    let header_bytes = &input[..delimiter];
    let body = &input[delimiter + 4..];
    let header_text = str::from_utf8(header_bytes).map_err(|_| ParseError::InvalidUtf8)?;
    let mut lines = header_text.split("\r\n");
    let start_line = lines.next().ok_or(ParseError::InvalidStartLine)?;
    if start_line.is_empty() {
        return Err(ParseError::InvalidStartLine);
    }

    let mut headers = Headers::new();
    for line in lines {
        if line.len() > config.max_header_bytes {
            return Err(ParseError::HeaderTooLong {
                maximum: config.max_header_bytes,
            });
        }
        let (name, value) = line.split_once(':').ok_or(ParseError::InvalidHeader)?;
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(ParseError::InvalidHeader);
        }
        let value = value.trim_matches(|character: char| character == ' ' || character == '\t');
        if value.bytes().any(is_forbidden_value_byte) {
            return Err(ParseError::InvalidHeaderValue);
        }
        if headers.len() >= config.max_headers {
            return Err(ParseError::TooManyHeaders {
                maximum: config.max_headers,
            });
        }
        headers.push(name, value);
    }

    let declared_length = content_length(&headers)?;
    if declared_length != body.len() {
        return Err(ParseError::BodyLengthMismatch {
            declared: declared_length,
            available: body.len(),
        });
    }

    let body = body.to_vec();
    if start_line.starts_with("SIP/") {
        let mut fields = start_line.splitn(3, ' ');
        let version = fields.next().unwrap_or_default();
        let code = fields
            .next()
            .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(ParseError::InvalidStartLine)?;
        if version != "SIP/2.0" {
            return Err(ParseError::InvalidVersion);
        }
        let reason = fields.next().unwrap_or_default().to_owned();
        return Ok(SipMessage::Response(SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: code,
            reason,
            headers,
            body,
        }));
    }

    let mut fields = start_line.splitn(3, ' ');
    let method = fields
        .next()
        .and_then(SipMethod::parse)
        .ok_or(ParseError::InvalidMethod)?;
    let request_uri = fields.next().ok_or(ParseError::InvalidUri)?;
    let version = fields.next().ok_or(ParseError::InvalidVersion)?;
    if request_uri.is_empty()
        || request_uri.len() > config.max_uri_bytes
        || request_uri
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(ParseError::InvalidUri);
    }
    if version != "SIP/2.0" {
        return Err(ParseError::InvalidVersion);
    }
    Ok(SipMessage::Request(SipRequest {
        method,
        request_uri: request_uri.to_owned(),
        version: version.to_owned(),
        headers,
        body,
    }))
}

fn content_length(headers: &Headers) -> Result<usize, ParseError> {
    let mut declared = None;
    for value in headers.get_all("Content-Length") {
        let parsed = value
            .trim()
            .parse::<usize>()
            .map_err(|_| ParseError::InvalidContentLength)?;
        if let Some(previous) = declared {
            if previous != parsed {
                return Err(ParseError::InvalidContentLength);
            }
        }
        declared = Some(parsed);
    }
    Ok(declared.unwrap_or(0))
}

pub fn serialize(message: &SipMessage) -> Vec<u8> {
    let (start_line, headers, body) = match message {
        SipMessage::Request(request) => (
            format!(
                "{} {} {}\r\n",
                request.method, request.request_uri, request.version
            ),
            &request.headers,
            request.body.as_slice(),
        ),
        SipMessage::Response(response) => (
            format!(
                "{} {} {}\r\n",
                response.version, response.status_code, response.reason
            ),
            &response.headers,
            response.body.as_slice(),
        ),
    };

    let mut output = Vec::with_capacity(start_line.len() + body.len() + 64);
    output.extend_from_slice(start_line.as_bytes());
    let mut wrote_content_length = false;
    for header in headers.iter() {
        if header.name.eq_ignore_ascii_case("Content-Length") {
            // Recompute framing from the owned body. This prevents a stale
            // caller-provided Content-Length from producing an invalid wire
            // message after the body is changed.
            if !wrote_content_length {
                output.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
                wrote_content_length = true;
            }
            continue;
        }
        output.extend_from_slice(header.name.as_bytes());
        output.extend_from_slice(b": ");
        output.extend_from_slice(header.value.as_bytes());
        output.extend_from_slice(b"\r\n");
    }
    if !wrote_content_length {
        output.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    output.extend_from_slice(b"\r\n");
    output.extend_from_slice(body);
    output
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'..=b'\'' | b'*'..=b'+' | b'-'..=b'.' | b'^' | b'_' | b'|' | b'~'
        )
        || byte == 96
}

fn is_forbidden_value_byte(byte: u8) -> bool {
    byte.is_ascii_control() && byte != b'\t'
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &[u8] = b"INVITE sip:alice@example.com SIP/2.0\r\nVia: SIP/2.0/UDP host;branch=z9\r\nContent-Length: 4\r\n\r\nping";

    #[test]
    fn parses_and_serializes_request_without_losing_unknown_headers() {
        let message = parse(INVITE).unwrap();
        let SipMessage::Request(request) = &message else {
            panic!("expected request");
        };
        assert_eq!(request.method, SipMethod::Invite);
        assert_eq!(
            request.headers.get("via"),
            Some("SIP/2.0/UDP host;branch=z9")
        );
        assert_eq!(request.body, b"ping");
        assert_eq!(parse(&serialize(&message)).unwrap(), message);
    }

    #[test]
    fn parses_response_and_rejects_body_mismatch() {
        let response = parse(b"SIP/2.0 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert!(matches!(response, SipMessage::Response(_)));
        assert!(matches!(
            parse(b"SIP/2.0 200 OK\r\nContent-Length: 3\r\n\r\nno"),
            Err(ParseError::BodyLengthMismatch { .. })
        ));
    }

    #[test]
    fn serialization_recomputes_stale_content_length() {
        let message = SipMessage::Request(SipRequest {
            method: SipMethod::Options,
            request_uri: "sip:example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers: {
                let mut headers = Headers::new();
                headers.push("Content-Length", "999");
                headers
            },
            body: b"ok".to_vec(),
        });
        let wire = serialize(&message);
        assert!(
            std::str::from_utf8(&wire)
                .unwrap()
                .contains("Content-Length: 2\r\n")
        );
        let parsed = parse(&wire).unwrap();
        let SipMessage::Request(parsed) = parsed else {
            panic!("expected request");
        };
        assert_eq!(parsed.body, b"ok");
        assert_eq!(parsed.headers.get("Content-Length"), Some("2"));
    }

    #[test]
    fn enforces_message_and_header_limits() {
        let config = ParseConfig {
            max_message_bytes: 10,
            ..ParseConfig::default()
        };
        assert!(matches!(
            parse_with_config(b"OPTIONS sip:x SIP/2.0\r\n\r\n", config),
            Err(ParseError::MessageTooLarge { .. })
        ));
        let config = ParseConfig {
            max_headers: 1,
            ..ParseConfig::default()
        };
        assert!(matches!(
            parse_with_config(
                b"OPTIONS sip:x SIP/2.0\r\nVia: a\r\nFrom: b\r\n\r\n",
                config
            ),
            Err(ParseError::TooManyHeaders { .. })
        ));
    }

    #[test]
    fn arbitrary_bytes_return_an_error_or_a_message_without_panicking() {
        for length in 0..512 {
            let input = (0..length)
                .map(|offset| (offset as u8).wrapping_mul(31))
                .collect::<Vec<_>>();
            let _ = parse(&input);
        }
    }
}
