//! Bounded SIP dialog identity, routing, sequence, and state handling.

use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use sip_types::{Headers, SipMethod, SipRequest, SipResponse};

const DEFAULT_MAX_FIELD_BYTES: usize = 4_096;
const DEFAULT_MAX_ROUTE_ENTRIES: usize = 32;

/// The side of a SIP dialog represented by a [`Dialog`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogRole {
    /// User-agent client, which creates the initial request.
    Uac,
    /// User-agent server, which receives the initial request.
    Uas,
}

/// High-level dialog lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogState {
    /// A dialog has a remote tag but has not received a final success response.
    Early,
    /// A final 2xx response and/or an in-dialog ACK established the dialog.
    Confirmed,
    /// The dialog has been closed and cannot accept further messages.
    Terminated,
}

/// Bounds applied while extracting dialog metadata from SIP headers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DialogConfig {
    /// Maximum bytes in a Call-ID, tag, or URI-like field.
    pub max_field_bytes: usize,
    /// Maximum number of Record-Route entries retained by a dialog.
    pub max_route_entries: usize,
}

impl Default for DialogConfig {
    fn default() -> Self {
        Self {
            max_field_bytes: DEFAULT_MAX_FIELD_BYTES,
            max_route_entries: DEFAULT_MAX_ROUTE_ENTRIES,
        }
    }
}

impl DialogConfig {
    fn validate(self) -> Result<Self, DialogError> {
        if self.max_field_bytes == 0 || self.max_route_entries == 0 {
            return Err(DialogError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Stable dialog identity. Call-ID alone is insufficient; the tags complete
/// the identity once a remote tag has been learned.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DialogId {
    call_id: String,
    local_tag: String,
    remote_tag: Option<String>,
}

impl DialogId {
    /// Returns the SIP Call-ID component.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the local From/To tag component.
    #[must_use]
    pub fn local_tag(&self) -> &str {
        &self.local_tag
    }

    /// Returns the remote tag, if the early/confirmed dialog has one.
    #[must_use]
    pub fn remote_tag(&self) -> Option<&str> {
        self.remote_tag.as_deref()
    }
}

/// Actions emitted while applying a SIP response or in-dialog request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialogAction {
    /// The dialog moved between lifecycle states.
    StateChanged {
        /// Previous state.
        from: DialogState,
        /// New state.
        to: DialogState,
    },
    /// A previously unknown remote tag completed the dialog identity.
    RemoteTagSet,
    /// The Contact/remote target changed.
    RemoteTargetChanged {
        /// New remote target URI.
        target: String,
    },
    /// The route set was learned from Record-Route headers.
    RouteSetChanged {
        /// Ordered route URIs, ready for subsequent requests.
        routes: Vec<String>,
    },
    /// An in-dialog request was accepted.
    RequestAccepted,
    /// An ACK established or confirmed the dialog.
    AckAccepted,
    /// A BYE closed the dialog.
    ByeReceived,
    /// A duplicate in-dialog request was observed.
    Retransmission,
    /// The dialog entered its terminal state.
    Terminated,
}

/// Errors returned when dialog metadata or transitions are invalid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialogError {
    /// A configurable bound was zero.
    InvalidConfig,
    /// A required SIP header was absent.
    MissingHeader(&'static str),
    /// A field was empty or contained invalid bytes.
    InvalidField(&'static str),
    /// A field exceeded the configured bound.
    FieldTooLong {
        /// Field name.
        field: &'static str,
        /// Configured maximum.
        maximum: usize,
    },
    /// The tag parameter was malformed.
    InvalidTag,
    /// The URI-like value was malformed.
    InvalidUri,
    /// A `CSeq` value was malformed.
    InvalidCSeq,
    /// The response/request method did not match the dialog transaction.
    MethodMismatch,
    /// A sequence number did not match or advance the dialog sequence.
    SequenceMismatch,
    /// A remote sequence number moved backwards or changed on a duplicate.
    SequenceOutOfOrder,
    /// A response that establishes a dialog did not carry a To tag.
    MissingRemoteTag,
    /// A message belonged to a different Call-ID.
    CallIdMismatch,
    /// A message carried a different local or remote tag.
    TagMismatch,
    /// The message is not valid for the dialog's role or state.
    InvalidState,
    /// The Record-Route set exceeded the configured bound.
    TooManyRoutes {
        /// Configured route limit.
        maximum: usize,
    },
    /// A newly learned route set conflicted with the established one.
    RouteSetMismatch,
    /// A local `CSeq` could not be incremented.
    SequenceOverflow,
    /// The initial request cannot create a dialog.
    InvalidInitialRequest,
    /// The request method is handled by the transaction layer instead.
    InvalidMethod,
}

impl Display for DialogError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("SIP dialog bounds must be non-zero"),
            Self::MissingHeader(name) => write!(formatter, "SIP dialog requires {name}"),
            Self::InvalidField(name) => write!(formatter, "SIP dialog field {name} is invalid"),
            Self::FieldTooLong { field, maximum } => {
                write!(
                    formatter,
                    "SIP dialog field {field} exceeds {maximum} bytes"
                )
            }
            Self::InvalidTag => formatter.write_str("SIP dialog tag is invalid"),
            Self::InvalidUri => formatter.write_str("SIP dialog URI is invalid"),
            Self::InvalidCSeq => formatter.write_str("SIP dialog CSeq is invalid"),
            Self::MethodMismatch => formatter.write_str("SIP dialog CSeq method mismatches"),
            Self::SequenceMismatch => formatter.write_str("SIP dialog sequence number mismatches"),
            Self::SequenceOutOfOrder => formatter.write_str("SIP dialog sequence is out of order"),
            Self::MissingRemoteTag => formatter.write_str("SIP response is missing its remote tag"),
            Self::CallIdMismatch => formatter.write_str("SIP message Call-ID mismatches dialog"),
            Self::TagMismatch => formatter.write_str("SIP message tag mismatches dialog"),
            Self::InvalidState => formatter.write_str("SIP dialog event is invalid in this state"),
            Self::TooManyRoutes { maximum } => {
                write!(formatter, "SIP dialog exceeds the {maximum}-route limit")
            }
            Self::RouteSetMismatch => {
                formatter.write_str("SIP dialog route set changed unexpectedly")
            }
            Self::SequenceOverflow => formatter.write_str("SIP dialog local CSeq overflowed"),
            Self::InvalidInitialRequest => {
                formatter.write_str("SIP request cannot establish an initial dialog")
            }
            Self::InvalidMethod => formatter.write_str("SIP method is not valid in a dialog"),
        }
    }
}

impl Error for DialogError {}

/// A bounded SIP dialog independent from transaction and call state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dialog {
    role: DialogRole,
    state: DialogState,
    call_id: String,
    local_tag: String,
    remote_tag: Option<String>,
    local_seq: u32,
    initial_sequence: u32,
    remote_seq: Option<u32>,
    remote_method: Option<SipMethod>,
    route_set: Vec<String>,
    remote_target: String,
    local_method: Option<SipMethod>,
    initial_method: SipMethod,
    config: DialogConfig,
}

impl Dialog {
    /// Creates a UAC dialog from its initial request.
    pub fn from_uac_request(
        request: &SipRequest,
        config: DialogConfig,
    ) -> Result<Self, DialogError> {
        let config = config.validate()?;
        if !is_dialog_forming_method(&request.method) {
            return Err(DialogError::InvalidInitialRequest);
        }
        let call_id = required_value(&request.headers, &["Call-ID", "i"], "Call-ID")?;
        let from = required_value(&request.headers, &["From", "f"], "From")?;
        let local_tag = required_tag(from, config.max_field_bytes)?;
        let (local_seq, cseq_method) = required_cseq(&request.headers)?;
        if cseq_method != request.method {
            return Err(DialogError::MethodMismatch);
        }
        let to = required_value(&request.headers, &["To", "t"], "To")?;
        let remote_tag = optional_tag(Some(to), config.max_field_bytes)?;
        let remote_target = validate_uri(&request.request_uri, config.max_field_bytes)?;
        let call_id = validate_field(call_id, "Call-ID", config.max_field_bytes)?;
        Ok(Self {
            role: DialogRole::Uac,
            state: DialogState::Early,
            call_id,
            local_tag,
            remote_tag,
            local_seq,
            initial_sequence: local_seq,
            remote_seq: None,
            remote_method: None,
            route_set: Vec::new(),
            remote_target,
            local_method: Some(request.method.clone()),
            initial_method: request.method.clone(),
            config,
        })
    }

    /// Creates a UAS dialog from an incoming initial INVITE.
    pub fn from_uas_invite(
        request: &SipRequest,
        local_tag: impl AsRef<str>,
        config: DialogConfig,
    ) -> Result<Self, DialogError> {
        if request.method != SipMethod::Invite {
            return Err(DialogError::InvalidInitialRequest);
        }
        Self::from_uas_request(request, local_tag, config)
    }

    /// Creates a UAS dialog from an initial dialog-forming request.
    pub fn from_uas_request(
        request: &SipRequest,
        local_tag: impl AsRef<str>,
        config: DialogConfig,
    ) -> Result<Self, DialogError> {
        let config = config.validate()?;
        if !is_dialog_forming_method(&request.method) {
            return Err(DialogError::InvalidInitialRequest);
        }
        let call_id = required_value(&request.headers, &["Call-ID", "i"], "Call-ID")?;
        let from = required_value(&request.headers, &["From", "f"], "From")?;
        let remote_tag = required_tag(from, config.max_field_bytes)?;
        let to = required_value(&request.headers, &["To", "t"], "To")?;
        if optional_tag(Some(to), config.max_field_bytes)?.is_some() {
            return Err(DialogError::InvalidInitialRequest);
        }
        let (remote_seq, cseq_method) = required_cseq(&request.headers)?;
        if cseq_method != request.method {
            return Err(DialogError::MethodMismatch);
        }
        let local_tag = validate_tag(local_tag.as_ref(), config.max_field_bytes)?;
        let remote_target = contact_or_request_uri(request, config.max_field_bytes)?;
        let route_set = parse_routes(
            &request.headers,
            config.max_field_bytes,
            config.max_route_entries,
            false,
        )?;
        let call_id = validate_field(call_id, "Call-ID", config.max_field_bytes)?;
        Ok(Self {
            role: DialogRole::Uas,
            state: DialogState::Early,
            call_id,
            local_tag,
            remote_tag: Some(remote_tag),
            local_seq: 0,
            initial_sequence: remote_seq,
            remote_seq: Some(remote_seq),
            remote_method: Some(cseq_method),
            route_set,
            remote_target,
            local_method: None,
            initial_method: request.method.clone(),
            config,
        })
    }

    /// Returns the dialog's role.
    #[must_use]
    pub fn role(&self) -> DialogRole {
        self.role
    }

    /// Returns the dialog lifecycle state.
    #[must_use]
    pub fn state(&self) -> DialogState {
        self.state
    }

    /// Returns the full tag-qualified dialog identity.
    #[must_use]
    pub fn id(&self) -> DialogId {
        DialogId {
            call_id: self.call_id.clone(),
            local_tag: self.local_tag.clone(),
            remote_tag: self.remote_tag.clone(),
        }
    }

    /// Returns the SIP Call-ID.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the local tag.
    #[must_use]
    pub fn local_tag(&self) -> &str {
        &self.local_tag
    }

    /// Returns the remote tag, if learned.
    #[must_use]
    pub fn remote_tag(&self) -> Option<&str> {
        self.remote_tag.as_deref()
    }

    /// Returns the next local sequence number without mutating the dialog.
    #[must_use]
    pub fn local_sequence(&self) -> u32 {
        self.local_seq
    }

    /// Returns the most recently accepted remote sequence number.
    #[must_use]
    pub fn remote_sequence(&self) -> Option<u32> {
        self.remote_seq
    }

    /// Returns the ordered route set.
    #[must_use]
    pub fn route_set(&self) -> &[String] {
        &self.route_set
    }

    /// Returns the current remote target URI.
    #[must_use]
    pub fn remote_target(&self) -> &str {
        &self.remote_target
    }

    /// Allocates the next local in-dialog `CSeq` using the initial method.
    pub fn next_local_sequence(&mut self) -> Result<u32, DialogError> {
        self.next_local_sequence_for(self.initial_method.clone())
    }

    /// Allocates the next local in-dialog `CSeq` for `method`.
    pub fn next_local_sequence_for(&mut self, method: SipMethod) -> Result<u32, DialogError> {
        if self.state != DialogState::Confirmed {
            return Err(DialogError::InvalidState);
        }
        self.local_seq = self
            .local_seq
            .checked_add(1)
            .ok_or(DialogError::SequenceOverflow)?;
        self.local_method = Some(method);
        Ok(self.local_seq)
    }

    /// Applies a response to the most recent local dialog request.
    pub fn receive_response(
        &mut self,
        response: &SipResponse,
    ) -> Result<Vec<DialogAction>, DialogError> {
        if self.state == DialogState::Terminated {
            return Err(DialogError::InvalidState);
        }
        validate_message_call_id(&response.headers, &self.call_id)?;
        let local_method = self.local_method.clone().ok_or(DialogError::InvalidState)?;
        validate_response_cseq(&response.headers, self.local_seq, &local_method)?;
        let status = response.status_code;

        let mut actions = Vec::new();
        if let Some(tag) = self.response_remote_tag(&response.headers)? {
            self.set_remote_tag(tag, &mut actions)?;
        }

        let is_initial_transaction = self.role == DialogRole::Uac
            && self.local_seq == self.initial_sequence
            && local_method == self.initial_method;
        if !is_initial_transaction {
            if status >= 200 && local_method == SipMethod::Bye {
                self.transition(DialogState::Terminated, &mut actions);
            } else if status < 300 {
                self.apply_response_metadata(&response.headers, &mut actions, false)?;
            }
            return Ok(actions);
        }

        if status < 200 {
            if self.remote_tag.is_some() {
                self.apply_response_metadata(&response.headers, &mut actions, true)?;
            }
            return Ok(actions);
        }
        if status < 300 {
            if self.remote_tag.is_none() {
                return Err(DialogError::MissingRemoteTag);
            }
            self.apply_response_metadata(&response.headers, &mut actions, true)?;
            self.transition(DialogState::Confirmed, &mut actions);
        } else {
            self.transition(DialogState::Terminated, &mut actions);
        }
        Ok(actions)
    }

    /// Applies an in-dialog request from the remote endpoint.
    pub fn receive_request(
        &mut self,
        request: &SipRequest,
    ) -> Result<Vec<DialogAction>, DialogError> {
        if self.state == DialogState::Terminated {
            return Err(DialogError::InvalidState);
        }
        validate_message_call_id(&request.headers, &self.call_id)?;
        let from = required_value(&request.headers, &["From", "f"], "From")?;
        let to = required_value(&request.headers, &["To", "t"], "To")?;
        let from_tag = required_tag(from, self.config.max_field_bytes)?;
        let to_tag = required_tag(to, self.config.max_field_bytes)?;
        if self.remote_tag.as_deref() != Some(from_tag.as_str()) || to_tag != self.local_tag {
            return Err(DialogError::TagMismatch);
        }
        let (sequence, cseq_method) = required_cseq(&request.headers)?;
        if cseq_method != request.method {
            return Err(DialogError::MethodMismatch);
        }
        if request.method == SipMethod::Ack {
            if self.initial_method != SipMethod::Invite || sequence != self.initial_sequence {
                return Err(DialogError::SequenceMismatch);
            }
            if self.state != DialogState::Early && self.state != DialogState::Confirmed {
                return Err(DialogError::InvalidState);
            }
            let mut actions = Vec::new();
            self.transition(DialogState::Confirmed, &mut actions);
            actions.push(DialogAction::AckAccepted);
            return Ok(actions);
        }
        if request.method == SipMethod::Cancel {
            return Err(DialogError::InvalidMethod);
        }
        if self.state != DialogState::Confirmed {
            return Err(DialogError::InvalidState);
        }

        let duplicate = self.observe_remote_sequence(sequence, &cseq_method)?;
        if duplicate {
            return Ok(vec![DialogAction::Retransmission]);
        }
        let mut actions = Vec::new();
        self.apply_request_metadata(&request.headers, &mut actions)?;
        if request.method == SipMethod::Bye {
            self.transition(DialogState::Terminated, &mut actions);
            actions.push(DialogAction::ByeReceived);
        } else {
            actions.push(DialogAction::RequestAccepted);
        }
        Ok(actions)
    }

    /// Terminates the dialog locally and returns the resulting actions.
    pub fn terminate(&mut self) -> Vec<DialogAction> {
        let mut actions = Vec::new();
        self.transition(DialogState::Terminated, &mut actions);
        actions
    }

    fn set_remote_tag(
        &mut self,
        tag: String,
        actions: &mut Vec<DialogAction>,
    ) -> Result<(), DialogError> {
        if let Some(previous) = self.remote_tag.as_deref() {
            if previous != tag {
                return Err(DialogError::TagMismatch);
            }
        } else {
            self.remote_tag = Some(tag);
            actions.push(DialogAction::RemoteTagSet);
        }
        Ok(())
    }

    fn response_remote_tag(&self, headers: &Headers) -> Result<Option<String>, DialogError> {
        let (local_names, local_display_name, remote_value) = match self.role {
            DialogRole::Uac => (
                &["From", "f"][..],
                "From",
                optional_value(headers, &["To", "t"]),
            ),
            DialogRole::Uas => (
                &["To", "t"][..],
                "To",
                optional_value(headers, &["From", "f"]),
            ),
        };
        let local = required_value(headers, local_names, local_display_name)?;
        if required_tag(local, self.config.max_field_bytes)? != self.local_tag {
            return Err(DialogError::TagMismatch);
        }
        optional_tag(remote_value, self.config.max_field_bytes)
    }

    fn apply_response_metadata(
        &mut self,
        headers: &Headers,
        actions: &mut Vec<DialogAction>,
        reverse_routes: bool,
    ) -> Result<(), DialogError> {
        if let Some(contact) = optional_value(headers, &["Contact", "m"]) {
            self.update_remote_target(contact, actions)?;
        }
        let routes = parse_routes(
            headers,
            self.config.max_field_bytes,
            self.config.max_route_entries,
            reverse_routes,
        )?;
        self.update_route_set(routes, actions)
    }

    fn apply_request_metadata(
        &mut self,
        headers: &Headers,
        actions: &mut Vec<DialogAction>,
    ) -> Result<(), DialogError> {
        if let Some(contact) = optional_value(headers, &["Contact", "m"]) {
            self.update_remote_target(contact, actions)?;
        }
        Ok(())
    }

    fn update_remote_target(
        &mut self,
        contact: &str,
        actions: &mut Vec<DialogAction>,
    ) -> Result<(), DialogError> {
        let target = extract_uri(contact, self.config.max_field_bytes)?;
        if self.remote_target != target {
            self.remote_target.clone_from(&target);
            actions.push(DialogAction::RemoteTargetChanged { target });
        }
        Ok(())
    }

    fn update_route_set(
        &mut self,
        routes: Vec<String>,
        actions: &mut Vec<DialogAction>,
    ) -> Result<(), DialogError> {
        if routes.is_empty() {
            return Ok(());
        }
        if self.route_set.is_empty() {
            self.route_set.clone_from(&routes);
            actions.push(DialogAction::RouteSetChanged { routes });
        } else if self.route_set != routes {
            return Err(DialogError::RouteSetMismatch);
        }
        Ok(())
    }

    fn observe_remote_sequence(
        &mut self,
        sequence: u32,
        method: &SipMethod,
    ) -> Result<bool, DialogError> {
        if let Some(previous) = self.remote_seq {
            if sequence < previous {
                return Err(DialogError::SequenceOutOfOrder);
            }
            if sequence == previous {
                if self.remote_method.as_ref() == Some(method) {
                    return Ok(true);
                }
                return Err(DialogError::SequenceOutOfOrder);
            }
        }
        self.remote_seq = Some(sequence);
        self.remote_method = Some(method.clone());
        Ok(false)
    }

    fn transition(&mut self, next: DialogState, actions: &mut Vec<DialogAction>) {
        if self.state == next {
            return;
        }
        let from = self.state;
        self.state = next;
        actions.push(DialogAction::StateChanged { from, to: next });
        if next == DialogState::Terminated {
            actions.push(DialogAction::Terminated);
        }
    }
}

fn required_value<'a>(
    headers: &'a Headers,
    names: &[&str],
    display_name: &'static str,
) -> Result<&'a str, DialogError> {
    optional_value(headers, names).ok_or(DialogError::MissingHeader(display_name))
}

fn optional_value<'a>(headers: &'a Headers, names: &[&str]) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| {
            names
                .iter()
                .any(|name| header.name.eq_ignore_ascii_case(name))
        })
        .map(|header| header.value.as_str())
}

fn validate_field(value: &str, field: &'static str, maximum: usize) -> Result<String, DialogError> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(DialogError::InvalidField(field));
    }
    if value.len() > maximum {
        return Err(DialogError::FieldTooLong { field, maximum });
    }
    Ok(value.to_owned())
}

fn is_dialog_forming_method(method: &SipMethod) -> bool {
    matches!(method, SipMethod::Invite | SipMethod::Refer)
        || matches!(method, SipMethod::Other(value) if value.eq_ignore_ascii_case("SUBSCRIBE"))
}

fn validate_tag(value: &str, maximum: usize) -> Result<String, DialogError> {
    if value.is_empty() || value.len() > maximum || !value.bytes().all(is_token_byte) {
        return if value.len() > maximum {
            Err(DialogError::FieldTooLong {
                field: "tag",
                maximum,
            })
        } else {
            Err(DialogError::InvalidTag)
        };
    }
    Ok(value.to_owned())
}

fn validate_uri(value: &str, maximum: usize) -> Result<String, DialogError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > maximum
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(if value.len() > maximum {
            DialogError::FieldTooLong {
                field: "URI",
                maximum,
            }
        } else {
            DialogError::InvalidUri
        });
    }
    Ok(value.to_owned())
}

fn required_tag(value: &str, maximum: usize) -> Result<String, DialogError> {
    optional_tag(Some(value), maximum)?.ok_or(DialogError::InvalidTag)
}

fn optional_tag(value: Option<&str>, maximum: usize) -> Result<Option<String>, DialogError> {
    let Some(value) = value else {
        return Ok(None);
    };
    for parameter in value.split(';').skip(1) {
        let Some((name, tag)) = parameter.trim().split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("tag") {
            return Ok(Some(validate_tag(tag.trim(), maximum)?));
        }
    }
    Ok(None)
}

fn required_cseq(headers: &Headers) -> Result<(u32, SipMethod), DialogError> {
    let value = required_value(headers, &["CSeq", "c"], "CSeq")?;
    let mut fields = value.split_whitespace();
    let sequence = fields
        .next()
        .ok_or(DialogError::InvalidCSeq)?
        .parse::<u32>()
        .map_err(|_| DialogError::InvalidCSeq)?;
    let method = fields
        .next()
        .and_then(SipMethod::parse)
        .ok_or(DialogError::InvalidCSeq)?;
    if fields.next().is_some() {
        return Err(DialogError::InvalidCSeq);
    }
    Ok((sequence, method))
}

fn validate_response_cseq(
    headers: &Headers,
    expected_sequence: u32,
    expected_method: &SipMethod,
) -> Result<(), DialogError> {
    let (sequence, method) = required_cseq(headers)?;
    if sequence != expected_sequence {
        return Err(DialogError::SequenceMismatch);
    }
    if &method != expected_method {
        return Err(DialogError::MethodMismatch);
    }
    Ok(())
}

fn validate_message_call_id(headers: &Headers, expected: &str) -> Result<(), DialogError> {
    let value = required_value(headers, &["Call-ID", "i"], "Call-ID")?;
    if value.trim() != expected {
        return Err(DialogError::CallIdMismatch);
    }
    Ok(())
}

fn contact_or_request_uri(request: &SipRequest, maximum: usize) -> Result<String, DialogError> {
    optional_value(&request.headers, &["Contact", "m"]).map_or_else(
        || validate_uri(&request.request_uri, maximum),
        |value| extract_uri(value, maximum),
    )
}

fn extract_uri(value: &str, maximum: usize) -> Result<String, DialogError> {
    let value = value.trim();
    if let Some(start) = value.find('<') {
        let end = value[start + 1..]
            .find('>')
            .map(|offset| start + 1 + offset)
            .ok_or(DialogError::InvalidUri)?;
        return validate_uri(&value[start + 1..end], maximum);
    }
    validate_uri(value.split(';').next().unwrap_or_default(), maximum)
}

fn parse_routes(
    headers: &Headers,
    maximum_field: usize,
    maximum_routes: usize,
    reverse: bool,
) -> Result<Vec<String>, DialogError> {
    let mut routes = Vec::new();
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("Record-Route"))
    {
        for route in header.value.split(',') {
            if routes.len() >= maximum_routes {
                return Err(DialogError::TooManyRoutes {
                    maximum: maximum_routes,
                });
            }
            routes.push(extract_uri(route, maximum_field)?);
        }
    }
    if reverse {
        routes.reverse();
    }
    Ok(routes)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'..=b'\'' | b'*'..=b'+' | b'-'..=b'.' | b'^' | b'_' | b'|' | b'~'
        )
        || byte == b'`'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite_request(to: &str, cseq: &str, contact: Option<&str>) -> SipRequest {
        let mut headers = Headers::new();
        headers.push("Call-ID", "call-123@example.com");
        headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        headers.push("To", to);
        headers.push("CSeq", cseq);
        if let Some(contact) = contact {
            headers.push("Contact", contact);
        }
        headers.push(
            "Record-Route",
            "<sip:proxy-a.example;lr>, <sip:proxy-b.example;lr>",
        );
        SipRequest {
            method: SipMethod::Invite,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    fn response(status: u16, to: &str, cseq: &str, contact: Option<&str>) -> SipResponse {
        let mut headers = Headers::new();
        headers.push("Call-ID", "call-123@example.com");
        headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        headers.push("To", to);
        headers.push("CSeq", cseq);
        if let Some(contact) = contact {
            headers.push("Contact", contact);
        }
        headers.push(
            "Record-Route",
            "<sip:proxy-a.example;lr>, <sip:proxy-b.example;lr>",
        );
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code: status,
            reason: "OK".to_owned(),
            headers,
            body: Vec::new(),
        }
    }

    #[test]
    fn uac_learns_tag_target_and_reversed_routes_then_allocates_sequence() {
        let request = invite_request("Bob <sip:bob@example.com>", "42 INVITE", None);
        let mut dialog = Dialog::from_uac_request(&request, DialogConfig::default()).unwrap();
        assert_eq!(dialog.id().remote_tag(), None);
        dialog
            .receive_response(&response(
                180,
                "Bob <sip:bob@example.com>;tag=remote-1",
                "42 INVITE",
                Some("<sip:bob@198.51.100.10>"),
            ))
            .unwrap();
        assert_eq!(dialog.state(), DialogState::Early);
        assert_eq!(dialog.remote_tag(), Some("remote-1"));
        assert_eq!(dialog.remote_target(), "sip:bob@198.51.100.10");
        assert_eq!(
            dialog.route_set(),
            ["sip:proxy-b.example;lr", "sip:proxy-a.example;lr"]
        );
        dialog
            .receive_response(&response(
                200,
                "Bob <sip:bob@example.com>;tag=remote-1",
                "42 INVITE",
                Some("<sip:bob@198.51.100.11>"),
            ))
            .unwrap();
        assert_eq!(dialog.state(), DialogState::Confirmed);
        assert_eq!(dialog.next_local_sequence().unwrap(), 43);
    }

    #[test]
    fn uas_confirms_on_ack_tracks_remote_sequence_and_accepts_bye() {
        let request = invite_request(
            "Bob <sip:bob@example.com>",
            "42 INVITE",
            Some("<sip:alice@192.0.2.10>"),
        );
        let mut dialog =
            Dialog::from_uas_invite(&request, "server-1", DialogConfig::default()).unwrap();
        assert_eq!(
            dialog.route_set(),
            ["sip:proxy-a.example;lr", "sip:proxy-b.example;lr"]
        );

        let mut ack_headers = Headers::new();
        ack_headers.push("Call-ID", "call-123@example.com");
        ack_headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        ack_headers.push("To", "Bob <sip:bob@example.com>;tag=server-1");
        ack_headers.push("CSeq", "42 ACK");
        let ack = SipRequest {
            method: SipMethod::Ack,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers: ack_headers,
            body: Vec::new(),
        };
        dialog.receive_request(&ack).unwrap();
        assert_eq!(dialog.state(), DialogState::Confirmed);

        let mut bye_headers = Headers::new();
        bye_headers.push("Call-ID", "call-123@example.com");
        bye_headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        bye_headers.push("To", "Bob <sip:bob@example.com>;tag=server-1");
        bye_headers.push("CSeq", "43 BYE");
        let bye = SipRequest {
            method: SipMethod::Bye,
            request_uri: "sip:bob@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers: bye_headers,
            body: Vec::new(),
        };
        let actions = dialog.receive_request(&bye).unwrap();
        assert!(actions.contains(&DialogAction::ByeReceived));
        assert_eq!(dialog.state(), DialogState::Terminated);
    }

    #[test]
    fn mismatched_identity_and_sequences_are_rejected() {
        let request = invite_request("Bob <sip:bob@example.com>", "42 INVITE", None);
        let mut dialog = Dialog::from_uac_request(&request, DialogConfig::default()).unwrap();
        let mut bad = response(
            200,
            "Bob <sip:bob@example.com>;tag=remote-1",
            "41 INVITE",
            None,
        );
        bad.headers.push("Call-ID", "other@example.com");
        assert!(matches!(
            dialog.receive_response(&bad),
            Err(DialogError::SequenceMismatch)
        ));

        let mut missing_tag = response(200, "Bob <sip:bob@example.com>", "42 INVITE", None);
        missing_tag.headers = {
            let mut headers = Headers::new();
            headers.push("Call-ID", "call-123@example.com");
            headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
            headers.push("To", "Bob <sip:bob@example.com>");
            headers.push("CSeq", "42 INVITE");
            headers
        };
        assert!(matches!(
            dialog.receive_response(&missing_tag),
            Err(DialogError::MissingRemoteTag)
        ));
    }

    #[test]
    fn uas_can_originate_in_dialog_requests_and_match_role_oriented_responses() {
        let request = invite_request(
            "Bob <sip:bob@example.com>",
            "42 INVITE",
            Some("<sip:alice@192.0.2.10>"),
        );
        let mut dialog =
            Dialog::from_uas_invite(&request, "server-1", DialogConfig::default()).unwrap();
        let mut ack_headers = Headers::new();
        ack_headers.push("Call-ID", "call-123@example.com");
        ack_headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        ack_headers.push("To", "Bob <sip:bob@example.com>;tag=server-1");
        ack_headers.push("CSeq", "42 ACK");
        dialog
            .receive_request(&SipRequest {
                method: SipMethod::Ack,
                request_uri: "sip:bob@example.com".to_owned(),
                version: "SIP/2.0".to_owned(),
                headers: ack_headers,
                body: Vec::new(),
            })
            .unwrap();

        assert_eq!(
            dialog.next_local_sequence_for(SipMethod::Update).unwrap(),
            1
        );
        let mut response_headers = Headers::new();
        response_headers.push("Call-ID", "call-123@example.com");
        response_headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        response_headers.push("To", "Bob <sip:bob@example.com>;tag=server-1");
        response_headers.push("CSeq", "1 UPDATE");
        response_headers.push("Contact", "<sip:alice@192.0.2.11>");
        dialog
            .receive_response(&SipResponse {
                version: "SIP/2.0".to_owned(),
                status_code: 200,
                reason: "OK".to_owned(),
                headers: response_headers,
                body: Vec::new(),
            })
            .unwrap();
        assert_eq!(dialog.state(), DialogState::Confirmed);
        assert_eq!(dialog.remote_target(), "sip:alice@192.0.2.11");

        assert_eq!(dialog.next_local_sequence_for(SipMethod::Bye).unwrap(), 2);
        let mut bye_response_headers = Headers::new();
        bye_response_headers.push("Call-ID", "call-123@example.com");
        bye_response_headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
        bye_response_headers.push("To", "Bob <sip:bob@example.com>;tag=server-1");
        bye_response_headers.push("CSeq", "2 BYE");
        dialog
            .receive_response(&SipResponse {
                version: "SIP/2.0".to_owned(),
                status_code: 200,
                reason: "OK".to_owned(),
                headers: bye_response_headers,
                body: Vec::new(),
            })
            .unwrap();
        assert_eq!(dialog.state(), DialogState::Terminated);
    }

    #[test]
    fn limits_and_duplicate_requests_are_deterministic() {
        let request = invite_request("Bob <sip:bob@example.com>", "42 INVITE", None);
        assert!(matches!(
            Dialog::from_uac_request(
                &request,
                DialogConfig {
                    max_field_bytes: 0,
                    ..DialogConfig::default()
                }
            ),
            Err(DialogError::InvalidConfig)
        ));

        let mut dialog = Dialog::from_uac_request(&request, DialogConfig::default()).unwrap();
        dialog
            .receive_response(&response(
                200,
                "Bob <sip:bob@example.com>;tag=remote-1",
                "42 INVITE",
                None,
            ))
            .unwrap();
        let mut info = request.clone();
        info.method = SipMethod::Info;
        let mut headers = Headers::new();
        headers.push("Call-ID", "call-123@example.com");
        headers.push("From", "Bob <sip:bob@example.com>;tag=remote-1");
        headers.push("To", "Alice <sip:alice@example.com>;tag=wrong");
        headers.push("CSeq", "7 INFO");
        info.headers = headers;
        assert!(matches!(
            dialog.receive_request(&info),
            Err(DialogError::TagMismatch)
        ));

        let mut options = request.clone();
        options.method = SipMethod::Options;
        options.headers = {
            let mut headers = Headers::new();
            headers.push("Call-ID", "call-123@example.com");
            headers.push("From", "Alice <sip:alice@example.com>;tag=local-1");
            headers.push("To", "Bob <sip:bob@example.com>");
            headers.push("CSeq", "42 OPTIONS");
            headers
        };
        assert!(matches!(
            Dialog::from_uac_request(&options, DialogConfig::default()),
            Err(DialogError::InvalidInitialRequest)
        ));
    }
}
