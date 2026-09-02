//! Explicit call identifiers and a deterministic call lifecycle state machine.

use std::{
    error::Error,
    fmt::{Display, Formatter},
};

macro_rules! identifier {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let value = value.into();
                if value.len() > 128
                    || value.is_empty()
                    || !value.bytes().all(|byte| byte.is_ascii_graphic())
                    || !value.starts_with($prefix)
                {
                    return Err(IdentifierError);
                }
                Ok(Self(value))
            }

            pub fn from_sequence(sequence: u64) -> Self {
                Self(format!("{}{}", $prefix, sequence))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

identifier!(CallId, "call_");
identifier!(LegId, "leg_");
identifier!(StreamId, "stream_");
identifier!(EventId, "evt_");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdentifierError;

impl Display for IdentifierError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("identifier must use its stable prefix and printable bytes")
    }
}

impl Error for IdentifierError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallState {
    Created,
    Inviting,
    Early,
    Ringing,
    Answered,
    Active,
    Transferring,
    Ending,
    Ended,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallEventKind {
    Created,
    InviteReceived,
    Ringing,
    EarlyMedia,
    Answered,
    MediaStarted,
    Transferring,
    Transferred,
    Hangup,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleEvent {
    pub event_id: EventId,
    pub call_id: CallId,
    pub kind: CallEventKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    InvalidTransition { from: CallState, to: CallState },
}

impl Display for TransitionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let Self::InvalidTransition { from, to } = self;
        write!(formatter, "invalid call transition from {from:?} to {to:?}")
    }
}

impl Error for TransitionError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Call {
    pub id: CallId,
    pub state: CallState,
}

impl Call {
    pub fn new(id: CallId) -> Self {
        Self {
            id,
            state: CallState::Created,
        }
    }

    pub fn transition(&mut self, next: CallState) -> Result<(), TransitionError> {
        if is_valid_transition(self.state, next) {
            self.state = next;
            Ok(())
        } else {
            Err(TransitionError::InvalidTransition {
                from: self.state,
                to: next,
            })
        }
    }
}

fn is_valid_transition(from: CallState, to: CallState) -> bool {
    matches!(
        (from, to),
        (
            CallState::Created,
            CallState::Inviting | CallState::Ending | CallState::Failed
        ) | (
            CallState::Inviting,
            CallState::Early
                | CallState::Ringing
                | CallState::Answered
                | CallState::Ending
                | CallState::Failed
        ) | (
            CallState::Early,
            CallState::Ringing | CallState::Answered | CallState::Ending | CallState::Failed
        ) | (
            CallState::Ringing,
            CallState::Answered | CallState::Ending | CallState::Failed
        ) | (CallState::Answered, CallState::Active | CallState::Ending)
            | (
                CallState::Active,
                CallState::Transferring | CallState::Ending
            )
            | (
                CallState::Transferring,
                CallState::Active | CallState::Ending | CallState::Failed
            )
            | (CallState::Ending, CallState::Ended)
            | (CallState::Failed, CallState::Ended)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_independent_from_sip_call_id() {
        let call = CallId::from_sequence(7);
        let leg = LegId::from_sequence(7);
        assert_eq!(call.as_str(), "call_7");
        assert_eq!(leg.as_str(), "leg_7");
        assert_ne!(call.to_string(), leg.to_string());
        assert!(CallId::new("sip-call-id").is_err());
    }

    #[test]
    fn valid_lifecycle_and_impossible_transition() {
        let mut call = Call::new(CallId::from_sequence(1));
        for state in [
            CallState::Inviting,
            CallState::Ringing,
            CallState::Answered,
            CallState::Active,
            CallState::Ending,
            CallState::Ended,
        ] {
            call.transition(state).unwrap();
        }
        assert_eq!(call.state, CallState::Ended);
        assert!(call.transition(CallState::Active).is_err());
    }
}
