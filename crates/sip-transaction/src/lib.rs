//! Deterministic SIP transaction state machines and timer handling.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    time::Duration,
};

use sip_types::{SipMethod, SipRequest, SipResponse};

/// Whether a transaction uses a reliable stream or an unreliable datagram.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportReliability {
    Reliable,
    Unreliable,
}

/// The INVITE/non-INVITE transaction split from RFC 3261.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionKind {
    Invite,
    NonInvite,
}

impl TransactionKind {
    fn for_method(method: &SipMethod) -> Result<Self, TransactionError> {
        match method {
            SipMethod::Ack => Err(TransactionError::InvalidMethod),
            SipMethod::Invite => Ok(Self::Invite),
            _ => Ok(Self::NonInvite),
        }
    }
}

/// The protocol timers used by the state machines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerKind {
    A,
    B,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
}

/// Base timer values. Timer B/F/H use 64*T1; retransmit timers are capped at T2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerConfig {
    pub t1: Duration,
    pub t2: Duration,
    pub t4: Duration,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            t1: Duration::from_millis(500),
            t2: Duration::from_secs(4),
            t4: Duration::from_secs(5),
        }
    }
}

impl TimerConfig {
    fn validate(self) -> Result<Self, TransactionError> {
        if self.t1.is_zero() || self.t2 < self.t1 || self.t4.is_zero() {
            return Err(TransactionError::InvalidTimerConfig);
        }
        Ok(self)
    }

    fn sixty_four_t1(self) -> Duration {
        self.t1.checked_mul(64).unwrap_or(Duration::MAX)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimerEntry {
    kind: TimerKind,
    deadline: Duration,
    interval: Option<Duration>,
}

fn schedule(
    timers: &mut Vec<TimerEntry>,
    kind: TimerKind,
    now: Duration,
    delay: Duration,
    interval: Option<Duration>,
) {
    timers.retain(|entry| entry.kind != kind);
    timers.push(TimerEntry {
        kind,
        deadline: now.saturating_add(delay),
        interval,
    });
}

fn cancel(timers: &mut Vec<TimerEntry>, kind: TimerKind) {
    timers.retain(|entry| entry.kind != kind);
}

fn double_duration(value: Duration) -> Duration {
    value.checked_mul(2).unwrap_or(Duration::MAX)
}

/// Client transaction states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientState {
    Calling,
    Trying,
    Proceeding,
    Completed,
    Terminated,
}

/// Actions emitted by a client transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientAction {
    StateChanged { from: ClientState, to: ClientState },
    RetransmitRequest,
    AckRequired,
    TimedOut,
    Terminated,
}

/// Errors returned for invalid transaction input or state transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionError {
    InvalidMethod,
    InvalidStatusCode,
    InvalidState,
    InvalidTimerConfig,
}

impl Display for TransactionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMethod => "ACK cannot start a SIP transaction",
            Self::InvalidStatusCode => "SIP response status code must be between 100 and 699",
            Self::InvalidState => "SIP transaction event is invalid in the current state",
            Self::InvalidTimerConfig => "SIP timer configuration is invalid",
        })
    }
}

impl Error for TransactionError {}

fn validate_status(status_code: u16) -> Result<(), TransactionError> {
    if (100..=699).contains(&status_code) {
        Ok(())
    } else {
        Err(TransactionError::InvalidStatusCode)
    }
}

/// A client-side SIP transaction with deterministic timer polling.
#[derive(Clone, Debug)]
pub struct ClientTransaction {
    kind: TransactionKind,
    request: SipRequest,
    state: ClientState,
    reliability: TransportReliability,
    timers_config: TimerConfig,
    timers: Vec<TimerEntry>,
}

impl ClientTransaction {
    /// Starts a transaction at the supplied monotonic time.
    pub fn new(
        request: SipRequest,
        now: Duration,
        reliability: TransportReliability,
        timers_config: TimerConfig,
    ) -> Result<Self, TransactionError> {
        let kind = TransactionKind::for_method(&request.method)?;
        let timers_config = timers_config.validate()?;
        let state = match kind {
            TransactionKind::Invite => ClientState::Calling,
            TransactionKind::NonInvite => ClientState::Trying,
        };
        let mut transaction = Self {
            kind,
            request,
            state,
            reliability,
            timers_config,
            timers: Vec::with_capacity(3),
        };
        transaction.start_initial_timers(now);
        Ok(transaction)
    }

    fn start_initial_timers(&mut self, now: Duration) {
        let timeout = self.timers_config.sixty_four_t1();
        match (self.kind, self.reliability) {
            (TransactionKind::Invite, TransportReliability::Unreliable) => {
                schedule(
                    &mut self.timers,
                    TimerKind::A,
                    now,
                    self.timers_config.t1,
                    Some(self.timers_config.t1),
                );
                schedule(&mut self.timers, TimerKind::B, now, timeout, None);
            }
            (TransactionKind::Invite, TransportReliability::Reliable) => {
                schedule(&mut self.timers, TimerKind::B, now, timeout, None);
            }
            (TransactionKind::NonInvite, TransportReliability::Unreliable) => {
                schedule(
                    &mut self.timers,
                    TimerKind::E,
                    now,
                    self.timers_config.t1,
                    Some(self.timers_config.t1),
                );
                schedule(&mut self.timers, TimerKind::F, now, timeout, None);
            }
            (TransactionKind::NonInvite, TransportReliability::Reliable) => {
                schedule(&mut self.timers, TimerKind::F, now, timeout, None);
            }
        }
    }

    pub fn kind(&self) -> TransactionKind {
        self.kind
    }

    pub fn state(&self) -> ClientState {
        self.state
    }

    pub fn request(&self) -> &SipRequest {
        &self.request
    }

    fn transition(&mut self, next: ClientState, actions: &mut Vec<ClientAction>) {
        if self.state == next {
            return;
        }
        let from = self.state;
        self.state = next;
        actions.push(ClientAction::StateChanged { from, to: next });
        if next == ClientState::Terminated {
            self.timers.clear();
            actions.push(ClientAction::Terminated);
        }
    }

    /// Applies a response from the transport/dialog layer.
    pub fn on_response(
        &mut self,
        response: &SipResponse,
        now: Duration,
    ) -> Result<Vec<ClientAction>, TransactionError> {
        validate_status(response.status_code)?;
        if self.state == ClientState::Terminated || self.state == ClientState::Completed {
            if self.state == ClientState::Completed
                && self.kind == TransactionKind::Invite
                && response.status_code >= 300
            {
                return Ok(vec![ClientAction::AckRequired]);
            }
            return Err(TransactionError::InvalidState);
        }
        let mut actions = Vec::with_capacity(2);
        if response.status_code < 200 {
            if matches!(self.state, ClientState::Calling | ClientState::Trying) {
                self.transition(ClientState::Proceeding, &mut actions);
                if self.kind == TransactionKind::Invite {
                    cancel(&mut self.timers, TimerKind::A);
                }
            }
            return Ok(actions);
        }

        match self.kind {
            TransactionKind::Invite if response.status_code < 300 => {
                self.transition(ClientState::Terminated, &mut actions);
            }
            TransactionKind::Invite => {
                cancel(&mut self.timers, TimerKind::A);
                cancel(&mut self.timers, TimerKind::B);
                actions.push(ClientAction::AckRequired);
                self.transition(ClientState::Completed, &mut actions);
                if self.reliability == TransportReliability::Unreliable {
                    schedule(
                        &mut self.timers,
                        TimerKind::D,
                        now,
                        Duration::from_secs(32),
                        None,
                    );
                } else {
                    self.transition(ClientState::Terminated, &mut actions);
                }
            }
            TransactionKind::NonInvite => {
                cancel(&mut self.timers, TimerKind::E);
                cancel(&mut self.timers, TimerKind::F);
                self.transition(ClientState::Completed, &mut actions);
                if self.reliability == TransportReliability::Reliable {
                    self.transition(ClientState::Terminated, &mut actions);
                } else {
                    schedule(
                        &mut self.timers,
                        TimerKind::K,
                        now,
                        self.timers_config.t4,
                        None,
                    );
                }
            }
        }
        Ok(actions)
    }

    /// Fires all timers whose deadlines are at or before `now`.
    pub fn poll(&mut self, now: Duration) -> Vec<ClientAction> {
        let mut actions = Vec::new();
        while let Some(index) = self.timers.iter().position(|entry| entry.deadline <= now) {
            let entry = self.timers.remove(index);
            match entry.kind {
                TimerKind::A if self.state == ClientState::Calling => {
                    actions.push(ClientAction::RetransmitRequest);
                    let interval = entry
                        .interval
                        .map(double_duration)
                        .unwrap_or(self.timers_config.t1)
                        .min(self.timers_config.t2);
                    schedule(
                        &mut self.timers,
                        TimerKind::A,
                        now,
                        interval,
                        Some(interval),
                    );
                }
                TimerKind::E
                    if matches!(self.state, ClientState::Trying | ClientState::Proceeding) =>
                {
                    actions.push(ClientAction::RetransmitRequest);
                    let interval = entry
                        .interval
                        .map(double_duration)
                        .unwrap_or(self.timers_config.t1)
                        .min(self.timers_config.t2);
                    schedule(
                        &mut self.timers,
                        TimerKind::E,
                        now,
                        interval,
                        Some(interval),
                    );
                }
                TimerKind::B | TimerKind::F
                    if !matches!(self.state, ClientState::Terminated | ClientState::Completed) =>
                {
                    self.transition(ClientState::Terminated, &mut actions);
                    actions.push(ClientAction::TimedOut);
                }
                TimerKind::D if self.state == ClientState::Completed => {
                    self.transition(ClientState::Terminated, &mut actions);
                }
                TimerKind::K if self.state == ClientState::Completed => {
                    self.transition(ClientState::Terminated, &mut actions);
                }
                _ => {}
            }
        }
        actions
    }
}

/// Server transaction states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerState {
    Trying,
    Proceeding,
    Completed,
    Confirmed,
    Terminated,
}

/// Actions emitted by a server transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerAction {
    StateChanged { from: ServerState, to: ServerState },
    RetransmitResponse,
    AckAccepted,
    TimedOut,
    Terminated,
}

/// A server-side SIP transaction with deterministic timer polling.
#[derive(Clone, Debug)]
pub struct ServerTransaction {
    kind: TransactionKind,
    request: SipRequest,
    state: ServerState,
    reliability: TransportReliability,
    timers_config: TimerConfig,
    timers: Vec<TimerEntry>,
    last_response: Option<SipResponse>,
}

impl ServerTransaction {
    /// Creates a server transaction for an incoming request.
    pub fn new(
        request: SipRequest,
        reliability: TransportReliability,
        timers_config: TimerConfig,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            kind: TransactionKind::for_method(&request.method)?,
            request,
            state: ServerState::Trying,
            reliability,
            timers_config: timers_config.validate()?,
            timers: Vec::with_capacity(3),
            last_response: None,
        })
    }

    pub fn kind(&self) -> TransactionKind {
        self.kind
    }

    /// Returns whether this transaction uses a reliable or unreliable transport.
    #[must_use]
    pub fn reliability(&self) -> TransportReliability {
        self.reliability
    }

    pub fn state(&self) -> ServerState {
        self.state
    }

    pub fn request(&self) -> &SipRequest {
        &self.request
    }

    pub fn last_response(&self) -> Option<&SipResponse> {
        self.last_response.as_ref()
    }

    fn transition(&mut self, next: ServerState, actions: &mut Vec<ServerAction>) {
        if self.state == next {
            return;
        }
        let from = self.state;
        self.state = next;
        actions.push(ServerAction::StateChanged { from, to: next });
        if next == ServerState::Terminated {
            self.timers.clear();
            actions.push(ServerAction::Terminated);
        }
    }

    /// Records and sends a provisional response.
    pub fn send_provisional(
        &mut self,
        response: SipResponse,
    ) -> Result<Vec<ServerAction>, TransactionError> {
        validate_status(response.status_code)?;
        if response.status_code >= 200
            || !matches!(self.state, ServerState::Trying | ServerState::Proceeding)
        {
            return Err(TransactionError::InvalidState);
        }
        self.last_response = Some(response);
        let mut actions = Vec::new();
        if self.state == ServerState::Trying {
            self.transition(ServerState::Proceeding, &mut actions);
        }
        Ok(actions)
    }

    /// Records and sends a final response, starting the appropriate timers.
    pub fn send_final(
        &mut self,
        response: SipResponse,
        now: Duration,
    ) -> Result<Vec<ServerAction>, TransactionError> {
        validate_status(response.status_code)?;
        if response.status_code < 200
            || !matches!(self.state, ServerState::Trying | ServerState::Proceeding)
        {
            return Err(TransactionError::InvalidState);
        }
        self.last_response = Some(response.clone());
        let mut actions = Vec::new();
        if self.kind == TransactionKind::Invite && response.status_code < 300 {
            self.transition(ServerState::Terminated, &mut actions);
            return Ok(actions);
        }
        self.transition(ServerState::Completed, &mut actions);
        match (self.kind, self.reliability) {
            (TransactionKind::Invite, TransportReliability::Unreliable) => {
                schedule(
                    &mut self.timers,
                    TimerKind::G,
                    now,
                    self.timers_config.t1,
                    Some(self.timers_config.t1),
                );
                schedule(
                    &mut self.timers,
                    TimerKind::H,
                    now,
                    self.timers_config.sixty_four_t1(),
                    None,
                );
            }
            (TransactionKind::Invite, TransportReliability::Reliable) => {
                // Reliable transports suppress response retransmissions, but a
                // non-2xx INVITE response still waits for the ACK (Timer H).
                schedule(
                    &mut self.timers,
                    TimerKind::H,
                    now,
                    self.timers_config.sixty_four_t1(),
                    None,
                );
            }
            (TransactionKind::NonInvite, TransportReliability::Unreliable) => {
                schedule(
                    &mut self.timers,
                    TimerKind::J,
                    now,
                    self.timers_config.sixty_four_t1(),
                    None,
                );
            }
            (TransactionKind::NonInvite, TransportReliability::Reliable) => {
                self.transition(ServerState::Terminated, &mut actions);
            }
        }
        Ok(actions)
    }

    /// Applies an ACK or retransmitted request from the network.
    pub fn on_request(
        &mut self,
        request: &SipRequest,
        now: Duration,
    ) -> Result<Vec<ServerAction>, TransactionError> {
        if self.state == ServerState::Terminated {
            return Err(TransactionError::InvalidState);
        }
        if self.kind == TransactionKind::Invite
            && request.method == SipMethod::Ack
            && self.state == ServerState::Completed
        {
            let mut actions = Vec::new();
            cancel(&mut self.timers, TimerKind::G);
            cancel(&mut self.timers, TimerKind::H);
            self.transition(ServerState::Confirmed, &mut actions);
            if self.reliability == TransportReliability::Unreliable {
                schedule(
                    &mut self.timers,
                    TimerKind::I,
                    now,
                    self.timers_config.t4,
                    None,
                );
            } else {
                self.transition(ServerState::Terminated, &mut actions);
            }
            actions.push(ServerAction::AckAccepted);
            return Ok(actions);
        }
        if matches!(self.state, ServerState::Proceeding | ServerState::Completed)
            && self.last_response.is_some()
        {
            return Ok(vec![ServerAction::RetransmitResponse]);
        }
        Err(TransactionError::InvalidState)
    }

    /// Fires all timers whose deadlines are at or before `now`.
    pub fn poll(&mut self, now: Duration) -> Vec<ServerAction> {
        let mut actions = Vec::new();
        while let Some(index) = self.timers.iter().position(|entry| entry.deadline <= now) {
            let entry = self.timers.remove(index);
            match entry.kind {
                TimerKind::G if self.state == ServerState::Completed => {
                    actions.push(ServerAction::RetransmitResponse);
                    let interval = entry
                        .interval
                        .map(double_duration)
                        .unwrap_or(self.timers_config.t1)
                        .min(self.timers_config.t2);
                    schedule(
                        &mut self.timers,
                        TimerKind::G,
                        now,
                        interval,
                        Some(interval),
                    );
                }
                TimerKind::H if self.state == ServerState::Completed => {
                    self.transition(ServerState::Terminated, &mut actions);
                    actions.push(ServerAction::TimedOut);
                }
                TimerKind::I if self.state == ServerState::Confirmed => {
                    self.transition(ServerState::Terminated, &mut actions);
                }
                TimerKind::J if self.state == ServerState::Completed => {
                    self.transition(ServerState::Terminated, &mut actions);
                }
                _ => {}
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sip_types::{Headers, SipRequest};

    fn request(method: SipMethod) -> SipRequest {
        SipRequest {
            method,
            request_uri: "sip:peer@example.com".to_owned(),
            version: "SIP/2.0".to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    fn response(status_code: u16) -> SipResponse {
        SipResponse {
            version: "SIP/2.0".to_owned(),
            status_code,
            reason: "OK".to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    fn fast_timers() -> TimerConfig {
        TimerConfig {
            t1: Duration::from_millis(10),
            t2: Duration::from_millis(40),
            t4: Duration::from_millis(80),
        }
    }

    #[test]
    fn unreliable_invite_retransmits_then_completes_on_final_response() {
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Invite),
            Duration::ZERO,
            TransportReliability::Unreliable,
            fast_timers(),
        )
        .unwrap();
        assert_eq!(transaction.state(), ClientState::Calling);
        assert_eq!(
            transaction.poll(Duration::from_millis(10)),
            vec![ClientAction::RetransmitRequest]
        );
        let actions = transaction
            .on_response(&response(486), Duration::ZERO)
            .unwrap();
        assert_eq!(transaction.state(), ClientState::Completed);
        assert!(actions.contains(&ClientAction::AckRequired));
        assert!(
            transaction
                .poll(Duration::from_secs(32))
                .contains(&ClientAction::Terminated)
        );
        assert_eq!(transaction.state(), ClientState::Terminated);
    }

    #[test]
    fn provisional_response_stops_retransmits_and_timeout_terminates() {
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Invite),
            Duration::ZERO,
            TransportReliability::Unreliable,
            fast_timers(),
        )
        .unwrap();
        let actions = transaction
            .on_response(&response(180), Duration::ZERO)
            .unwrap();
        assert_eq!(transaction.state(), ClientState::Proceeding);
        assert!(actions.iter().any(|action| matches!(
            action,
            ClientAction::StateChanged {
                from: ClientState::Calling,
                to: ClientState::Proceeding
            }
        )));
        assert!(transaction.poll(Duration::from_millis(10)).is_empty());
        assert!(
            transaction
                .poll(Duration::from_millis(640))
                .contains(&ClientAction::TimedOut)
        );
    }

    #[test]
    fn non_invite_retransmits_during_proceeding_until_final_response() {
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Options),
            Duration::ZERO,
            TransportReliability::Unreliable,
            fast_timers(),
        )
        .unwrap();
        transaction
            .on_response(&response(183), Duration::ZERO)
            .unwrap();
        assert_eq!(
            transaction.poll(Duration::from_millis(10)),
            vec![ClientAction::RetransmitRequest]
        );
        assert_eq!(
            transaction.poll(Duration::from_millis(30)),
            vec![ClientAction::RetransmitRequest]
        );
        transaction
            .on_response(&response(200), Duration::from_millis(30))
            .unwrap();
        assert!(transaction.poll(Duration::from_millis(109)).is_empty());
        assert!(
            transaction
                .poll(Duration::from_millis(110))
                .contains(&ClientAction::Terminated)
        );
    }

    #[test]
    fn server_invite_retransmits_final_until_ack_then_terminates() {
        let mut transaction = ServerTransaction::new(
            request(SipMethod::Invite),
            TransportReliability::Unreliable,
            fast_timers(),
        )
        .unwrap();
        transaction.send_provisional(response(180)).unwrap();
        let actions = transaction
            .send_final(response(486), Duration::ZERO)
            .unwrap();
        assert_eq!(transaction.state(), ServerState::Completed);
        assert!(actions.iter().any(|action| matches!(
            action,
            ServerAction::StateChanged {
                from: ServerState::Proceeding,
                to: ServerState::Completed
            }
        )));
        assert_eq!(
            transaction.poll(Duration::from_millis(10)),
            vec![ServerAction::RetransmitResponse]
        );
        let actions = transaction
            .on_request(&request(SipMethod::Ack), Duration::from_millis(20))
            .unwrap();
        assert!(actions.contains(&ServerAction::AckAccepted));
        assert_eq!(transaction.state(), ServerState::Confirmed);
        assert!(
            transaction
                .poll(Duration::from_millis(100))
                .contains(&ServerAction::Terminated)
        );
        assert!(matches!(
            transaction.send_provisional(response(180)),
            Err(TransactionError::InvalidState)
        ));
    }

    #[test]
    fn reliable_server_invite_waits_for_ack_without_retransmitting() {
        let mut transaction = ServerTransaction::new(
            request(SipMethod::Invite),
            TransportReliability::Reliable,
            fast_timers(),
        )
        .unwrap();
        transaction
            .send_final(response(486), Duration::ZERO)
            .unwrap();
        assert_eq!(transaction.state(), ServerState::Completed);
        assert!(transaction.poll(Duration::from_millis(10)).is_empty());
        assert!(
            transaction
                .poll(Duration::from_millis(640))
                .contains(&ServerAction::TimedOut)
        );
        assert_eq!(transaction.state(), ServerState::Terminated);

        let mut transaction = ServerTransaction::new(
            request(SipMethod::Invite),
            TransportReliability::Reliable,
            fast_timers(),
        )
        .unwrap();
        transaction
            .send_final(response(486), Duration::ZERO)
            .unwrap();
        let actions = transaction
            .on_request(&request(SipMethod::Ack), Duration::from_millis(20))
            .unwrap();
        assert!(actions.contains(&ServerAction::AckAccepted));
        assert_eq!(transaction.state(), ServerState::Terminated);
    }

    #[test]
    fn reliable_non_invite_terminates_after_final_response() {
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Options),
            Duration::ZERO,
            TransportReliability::Reliable,
            fast_timers(),
        )
        .unwrap();
        let actions = transaction
            .on_response(&response(200), Duration::ZERO)
            .unwrap();
        assert!(actions.contains(&ClientAction::Terminated));
        assert_eq!(transaction.state(), ClientState::Terminated);
    }

    #[test]
    fn non_invite_timer_k_starts_when_the_response_arrives() {
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Options),
            Duration::ZERO,
            TransportReliability::Unreliable,
            fast_timers(),
        )
        .unwrap();
        transaction
            .on_response(&response(200), Duration::from_millis(100))
            .unwrap();
        assert_eq!(transaction.state(), ClientState::Completed);
        assert!(transaction.poll(Duration::from_millis(179)).is_empty());
        assert!(
            transaction
                .poll(Duration::from_millis(180))
                .contains(&ClientAction::Terminated)
        );
    }

    #[test]
    fn invalid_methods_statuses_and_timer_values_fail_fast() {
        assert!(matches!(
            ClientTransaction::new(
                request(SipMethod::Ack),
                Duration::ZERO,
                TransportReliability::Unreliable,
                fast_timers()
            ),
            Err(TransactionError::InvalidMethod)
        ));
        assert!(matches!(
            TimerConfig {
                t1: Duration::ZERO,
                ..fast_timers()
            }
            .validate(),
            Err(TransactionError::InvalidTimerConfig)
        ));
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Options),
            Duration::ZERO,
            TransportReliability::Reliable,
            fast_timers(),
        )
        .unwrap();
        assert!(matches!(
            transaction.on_response(&response(700), Duration::ZERO),
            Err(TransactionError::InvalidStatusCode)
        ));
    }
}
