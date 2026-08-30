//! Safe SDP parsing, serialization, and audio codec intersection.

use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[derive(Clone, Copy, Debug)]
pub struct ParseConfig {
    pub max_message_bytes: usize,
    pub max_lines: usize,
    pub max_line_bytes: usize,
}

impl Default for ParseConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: 65_535,
            max_lines: 256,
            max_line_bytes: 4_096,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Direction {
    #[default]
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::SendRecv => "sendrecv",
            Self::SendOnly => "sendonly",
            Self::RecvOnly => "recvonly",
            Self::Inactive => "inactive",
        }
    }

    /// Computes the direction of the local media stream after applying the
    /// direction advertised by both endpoints.
    pub fn negotiate(local: Self, remote: Self) -> Self {
        let local_sends = matches!(local, Self::SendRecv | Self::SendOnly);
        let local_receives = matches!(local, Self::SendRecv | Self::RecvOnly);
        let remote_sends = matches!(remote, Self::SendRecv | Self::SendOnly);
        let remote_receives = matches!(remote, Self::SendRecv | Self::RecvOnly);
        match (
            local_sends && remote_receives,
            local_receives && remote_sends,
        ) {
            (true, true) => Self::SendRecv,
            (true, false) => Self::SendOnly,
            (false, true) => Self::RecvOnly,
            (false, false) => Self::Inactive,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Codec {
    pub payload_type: u8,
    pub name: String,
    pub clock_rate: u32,
    pub channels: u16,
    pub fmtp: Option<String>,
}

impl Codec {
    pub fn is_telephone_event(&self) -> bool {
        self.name.eq_ignore_ascii_case("telephone-event")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaDescription {
    pub media: String,
    pub port: u16,
    pub protocol: String,
    pub formats: Vec<u8>,
    pub connection: Option<String>,
    pub direction: Option<Direction>,
    pub codecs: Vec<Codec>,
    pub attributes: Vec<(String, Option<String>)>,
}

impl MediaDescription {
    pub fn effective_direction(&self, session_direction: Direction) -> Direction {
        self.direction.unwrap_or(session_direction)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionDescription {
    pub origin: String,
    pub session_name: String,
    pub connection: Option<String>,
    pub timing: String,
    pub direction: Direction,
    pub media: Vec<MediaDescription>,
    pub attributes: Vec<(String, Option<String>)>,
}

impl SessionDescription {
    pub fn new_audio(origin: impl Into<String>, connection: impl Into<String>, port: u16) -> Self {
        Self {
            origin: origin.into(),
            session_name: "-".to_owned(),
            connection: Some(connection.into()),
            timing: "0 0".to_owned(),
            direction: Direction::SendRecv,
            media: vec![MediaDescription {
                media: "audio".to_owned(),
                port,
                protocol: "RTP/AVP".to_owned(),
                formats: vec![0, 8],
                connection: None,
                direction: None,
                codecs: vec![
                    Codec {
                        payload_type: 0,
                        name: "PCMU".to_owned(),
                        clock_rate: 8_000,
                        channels: 1,
                        fmtp: None,
                    },
                    Codec {
                        payload_type: 8,
                        name: "PCMA".to_owned(),
                        clock_rate: 8_000,
                        channels: 1,
                        fmtp: None,
                    },
                ],
                attributes: Vec::new(),
            }],
            attributes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SdpError {
    Empty,
    TooLarge { actual: usize, maximum: usize },
    TooManyLines { maximum: usize },
    LineTooLong { maximum: usize },
    InvalidUtf8,
    InvalidLine,
    InvalidVersion,
    MissingField(&'static str),
    InvalidMedia,
    InvalidPort,
    InvalidPayloadType,
    InvalidCodec,
    InvalidAttribute,
}

impl Display for SdpError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("SDP is empty"),
            Self::TooLarge { actual, maximum } => {
                write!(formatter, "SDP is {actual} bytes, maximum is {maximum}")
            }
            Self::TooManyLines { maximum } => write!(formatter, "SDP exceeds {maximum} lines"),
            Self::LineTooLong { maximum } => write!(formatter, "SDP line exceeds {maximum} bytes"),
            Self::InvalidUtf8 => formatter.write_str("SDP is not valid UTF-8"),
            Self::InvalidLine => formatter.write_str("SDP line is invalid"),
            Self::InvalidVersion => formatter.write_str("SDP version must be 0"),
            Self::MissingField(field) => write!(formatter, "SDP is missing {field}"),
            Self::InvalidMedia => formatter.write_str("SDP media description is invalid"),
            Self::InvalidPort => formatter.write_str("SDP media port is invalid"),
            Self::InvalidPayloadType => formatter.write_str("SDP payload type is invalid"),
            Self::InvalidCodec => formatter.write_str("SDP codec mapping is invalid"),
            Self::InvalidAttribute => formatter.write_str("SDP attribute is invalid"),
        }
    }
}

impl Error for SdpError {}

pub fn parse(input: &[u8]) -> Result<SessionDescription, SdpError> {
    parse_with_config(input, ParseConfig::default())
}

pub fn parse_with_config(
    input: &[u8],
    config: ParseConfig,
) -> Result<SessionDescription, SdpError> {
    if input.is_empty() {
        return Err(SdpError::Empty);
    }
    if input.len() > config.max_message_bytes {
        return Err(SdpError::TooLarge {
            actual: input.len(),
            maximum: config.max_message_bytes,
        });
    }

    let text = std::str::from_utf8(input).map_err(|_| SdpError::InvalidUtf8)?;
    let mut version = None;
    let mut origin = None;
    let mut session_name = None;
    let mut connection = None;
    let mut timing = None;
    let mut direction = Direction::SendRecv;
    let mut media = Vec::new();
    let mut attributes = Vec::new();
    let mut current_media: Option<MediaDescription> = None;
    let mut line_count = 0;

    for raw_line in text.split(['\r', '\n']).filter(|line| !line.is_empty()) {
        line_count += 1;
        if line_count > config.max_lines {
            return Err(SdpError::TooManyLines {
                maximum: config.max_lines,
            });
        }
        if raw_line.len() > config.max_line_bytes {
            return Err(SdpError::LineTooLong {
                maximum: config.max_line_bytes,
            });
        }
        let (kind, value) = raw_line.split_once('=').ok_or(SdpError::InvalidLine)?;
        if kind.len() != 1 {
            return Err(SdpError::InvalidLine);
        }
        match kind.as_bytes()[0] {
            b'v' => version = Some(value.to_owned()),
            b'o' => origin = Some(value.to_owned()),
            b's' => session_name = Some(value.to_owned()),
            b'c' => {
                if let Some(current) = current_media.as_mut() {
                    current.connection = Some(value.to_owned());
                } else {
                    connection = Some(value.to_owned());
                }
            }
            b't' => timing = Some(value.to_owned()),
            b'm' => {
                if let Some(previous) = current_media.take() {
                    media.push(previous);
                }
                current_media = Some(parse_media(value)?);
            }
            b'a' => {
                let (name, argument) = value
                    .split_once(':')
                    .map_or((value, None), |(name, argument)| (name, Some(argument)));
                if name.is_empty() {
                    return Err(SdpError::InvalidAttribute);
                }
                if let Some(current) = current_media.as_mut() {
                    apply_media_attribute(current, name, argument)?;
                } else if let Some(parsed_direction) = parse_direction(name) {
                    direction = parsed_direction;
                } else {
                    attributes.push((name.to_owned(), argument.map(str::to_owned)));
                }
                if let Some(current) = current_media.as_mut() {
                    if let Some((payload, mapping)) = parse_rtpmap(name, argument)? {
                        upsert_codec(current, payload, mapping);
                    } else if let Some((payload, fmtp)) = parse_fmtp(name, argument)? {
                        if let Some(codec) = current
                            .codecs
                            .iter_mut()
                            .find(|codec| codec.payload_type == payload)
                        {
                            codec.fmtp = Some(fmtp);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(previous) = current_media {
        media.push(previous);
    }
    if version.as_deref() != Some("0") {
        return Err(SdpError::InvalidVersion);
    }
    Ok(SessionDescription {
        origin: origin.ok_or(SdpError::MissingField("origin"))?,
        session_name: session_name.ok_or(SdpError::MissingField("session name"))?,
        connection,
        timing: timing.ok_or(SdpError::MissingField("timing"))?,
        direction,
        media,
        attributes,
    })
}

fn parse_media(value: &str) -> Result<MediaDescription, SdpError> {
    let mut fields = value.split_whitespace();
    let media = fields.next().ok_or(SdpError::InvalidMedia)?;
    let port = fields
        .next()
        .ok_or(SdpError::InvalidPort)?
        .split('/')
        .next()
        .ok_or(SdpError::InvalidPort)?
        .parse()
        .map_err(|_| SdpError::InvalidPort)?;
    let protocol = fields.next().ok_or(SdpError::InvalidMedia)?;
    let formats = fields
        .map(|value| value.parse().map_err(|_| SdpError::InvalidPayloadType))
        .collect::<Result<Vec<u8>, SdpError>>()?;
    if formats.is_empty() {
        return Err(SdpError::InvalidPayloadType);
    }
    let codecs = formats
        .iter()
        .filter_map(|payload_type| static_codec(*payload_type))
        .collect();
    Ok(MediaDescription {
        media: media.to_owned(),
        port,
        protocol: protocol.to_owned(),
        formats,
        connection: None,
        direction: None,
        codecs,
        attributes: Vec::new(),
    })
}

fn static_codec(payload_type: u8) -> Option<Codec> {
    let (name, clock_rate) = match payload_type {
        0 => ("PCMU", 8_000),
        8 => ("PCMA", 8_000),
        _ => return None,
    };
    Some(Codec {
        payload_type,
        name: name.to_owned(),
        clock_rate,
        channels: 1,
        fmtp: None,
    })
}

fn parse_direction(name: &str) -> Option<Direction> {
    Some(match name {
        "sendrecv" => Direction::SendRecv,
        "sendonly" => Direction::SendOnly,
        "recvonly" => Direction::RecvOnly,
        "inactive" => Direction::Inactive,
        _ => return None,
    })
}

fn apply_media_attribute(
    media: &mut MediaDescription,
    name: &str,
    argument: Option<&str>,
) -> Result<(), SdpError> {
    if let Some(direction) = parse_direction(name) {
        media.direction = Some(direction);
    } else if name != "rtpmap" && name != "fmtp" {
        media
            .attributes
            .push((name.to_owned(), argument.map(str::to_owned)));
    }
    Ok(())
}

fn parse_rtpmap(name: &str, argument: Option<&str>) -> Result<Option<(u8, Codec)>, SdpError> {
    if name != "rtpmap" {
        return Ok(None);
    }
    let value = argument.ok_or(SdpError::InvalidCodec)?;
    let mut fields = value.split_whitespace();
    let payload = fields.next().ok_or(SdpError::InvalidCodec)?;
    let mapping = fields.next().ok_or(SdpError::InvalidCodec)?;
    if fields.next().is_some() {
        return Err(SdpError::InvalidCodec);
    }
    let payload_type = payload.parse().map_err(|_| SdpError::InvalidPayloadType)?;
    let mut parts = mapping.split('/');
    let codec_name = parts.next().ok_or(SdpError::InvalidCodec)?;
    let clock_rate = parts
        .next()
        .ok_or(SdpError::InvalidCodec)?
        .parse()
        .map_err(|_| SdpError::InvalidCodec)?;
    if clock_rate == 0 {
        return Err(SdpError::InvalidCodec);
    }
    let channels = parts.next().map_or(Ok(1), |value| {
        value.parse().map_err(|_| SdpError::InvalidCodec)
    })?;
    if channels == 0 || parts.next().is_some() {
        return Err(SdpError::InvalidCodec);
    }
    Ok(Some((
        payload_type,
        Codec {
            payload_type,
            name: codec_name.to_owned(),
            clock_rate,
            channels,
            fmtp: None,
        },
    )))
}

fn parse_fmtp(name: &str, argument: Option<&str>) -> Result<Option<(u8, String)>, SdpError> {
    if name != "fmtp" {
        return Ok(None);
    }
    let value = argument.ok_or(SdpError::InvalidCodec)?;
    let separator = value
        .find(|character: char| character.is_ascii_whitespace())
        .ok_or(SdpError::InvalidCodec)?;
    let payload = &value[..separator];
    let fmtp = value[separator..].trim();
    if fmtp.is_empty() {
        return Err(SdpError::InvalidCodec);
    }
    Ok(Some((
        payload.parse().map_err(|_| SdpError::InvalidPayloadType)?,
        fmtp.to_owned(),
    )))
}

fn upsert_codec(media: &mut MediaDescription, payload: u8, codec: Codec) {
    if let Some(existing) = media
        .codecs
        .iter_mut()
        .find(|existing| existing.payload_type == payload)
    {
        *existing = codec;
    } else {
        media.codecs.push(codec);
    }
}

pub fn serialize(description: &SessionDescription) -> Vec<u8> {
    let mut output = String::new();
    output.push_str("v=0\r\n");
    output.push_str("o=");
    output.push_str(&description.origin);
    output.push_str("\r\ns=");
    output.push_str(&description.session_name);
    output.push_str("\r\n");
    if let Some(connection) = &description.connection {
        output.push_str("c=");
        output.push_str(connection);
        output.push_str("\r\n");
    }
    output.push_str("t=");
    output.push_str(&description.timing);
    output.push_str("\r\n");
    output.push_str("a=");
    output.push_str(description.direction.as_str());
    output.push_str("\r\n");
    for (name, argument) in &description.attributes {
        push_attribute(&mut output, name, argument.as_deref());
    }
    for media in &description.media {
        output.push_str("m=");
        output.push_str(&media.media);
        output.push(' ');
        output.push_str(&media.port.to_string());
        output.push(' ');
        output.push_str(&media.protocol);
        for payload in &media.formats {
            output.push(' ');
            output.push_str(&payload.to_string());
        }
        output.push_str("\r\n");
        if let Some(connection) = &media.connection {
            output.push_str("c=");
            output.push_str(connection);
            output.push_str("\r\n");
        }
        if let Some(direction) = media.direction {
            output.push_str("a=");
            output.push_str(direction.as_str());
            output.push_str("\r\n");
        }
        for codec in &media.codecs {
            output.push_str("a=rtpmap:");
            output.push_str(&codec.payload_type.to_string());
            output.push(' ');
            output.push_str(&codec.name);
            output.push('/');
            output.push_str(&codec.clock_rate.to_string());
            if codec.channels != 1 {
                output.push('/');
                output.push_str(&codec.channels.to_string());
            }
            output.push_str("\r\n");
            if let Some(fmtp) = &codec.fmtp {
                output.push_str("a=fmtp:");
                output.push_str(&codec.payload_type.to_string());
                output.push(' ');
                output.push_str(fmtp);
                output.push_str("\r\n");
            }
        }
        for (name, argument) in &media.attributes {
            push_attribute(&mut output, name, argument.as_deref());
        }
    }
    output.into_bytes()
}

fn push_attribute(output: &mut String, name: &str, argument: Option<&str>) {
    output.push_str("a=");
    output.push_str(name);
    if let Some(argument) = argument {
        output.push(':');
        output.push_str(argument);
    }
    output.push_str("\r\n");
}

pub fn negotiate_audio<'a>(
    local: &'a SessionDescription,
    remote: &'a SessionDescription,
) -> Option<&'a Codec> {
    let local_audio = local.media.iter().find(|media| media.media == "audio")?;
    let remote_audio = remote.media.iter().find(|media| media.media == "audio")?;
    local_audio.codecs.iter().find(|local_codec| {
        !local_codec.is_telephone_event()
            && remote_audio.codecs.iter().any(|remote_codec| {
                local_codec.name.eq_ignore_ascii_case(&remote_codec.name)
                    && local_codec.clock_rate == remote_codec.clock_rate
                    && local_codec.channels == remote_codec.channels
            })
    })
}

/// Returns the local media direction resulting from an audio offer/answer.
pub fn negotiate_direction(
    local: &SessionDescription,
    remote: &SessionDescription,
) -> Option<Direction> {
    let local_audio = local.media.iter().find(|media| media.media == "audio")?;
    let remote_audio = remote.media.iter().find(|media| media.media == "audio")?;
    Some(Direction::negotiate(
        local_audio.effective_direction(local.direction),
        remote_audio.effective_direction(remote.direction),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &[u8] = b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\na=sendrecv\r\nm=audio 4000 RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\n";

    #[test]
    fn parses_direction_codecs_and_telephone_event() {
        let description = parse(OFFER).unwrap();
        assert_eq!(description.direction, Direction::SendRecv);
        let audio = &description.media[0];
        assert_eq!(audio.port, 4000);
        assert_eq!(audio.codecs[0].name, "PCMU");
        assert!(audio.codecs[1].is_telephone_event());
        assert_eq!(audio.codecs[1].fmtp.as_deref(), Some("0-16"));
    }

    #[test]
    fn supplies_static_audio_mappings_and_negotiates_direction() {
        let remote = parse(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=-\r\nt=0 0\r\na=sendonly\r\nm=audio 4000 RTP/AVP 0 8\r\n",
        )
        .unwrap();
        assert_eq!(remote.media[0].codecs[0].name, "PCMU");
        assert_eq!(remote.media[0].codecs[1].name, "PCMA");

        let local =
            SessionDescription::new_audio("- 2 2 IN IP4 192.0.2.20", "IN IP4 192.0.2.20", 5000);
        assert_eq!(
            negotiate_direction(&local, &remote),
            Some(Direction::RecvOnly)
        );
        assert_eq!(
            negotiate_direction(&remote, &local),
            Some(Direction::SendOnly)
        );
    }

    #[test]
    fn serializes_and_negotiates_audio() {
        let local =
            SessionDescription::new_audio("- 2 2 IN IP4 192.0.2.20", "IN IP4 192.0.2.20", 5000);
        let remote = parse(OFFER).unwrap();
        assert_eq!(negotiate_audio(&local, &remote).unwrap().name, "PCMU");
        assert_eq!(parse(&serialize(&local)).unwrap(), local);
    }

    #[test]
    fn malformed_input_is_rejected_with_limits() {
        assert!(matches!(parse(b"v=1\r\n"), Err(SdpError::InvalidVersion)));
        let config = ParseConfig {
            max_message_bytes: 2,
            ..ParseConfig::default()
        };
        assert!(matches!(
            parse_with_config(b"v=0\r\n", config),
            Err(SdpError::TooLarge { .. })
        ));
    }
}
