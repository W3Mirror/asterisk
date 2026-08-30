//! Deterministic UDP UAS used by the repository's local `SIPp` scenarios.

use std::{env, error::Error, io, net::SocketAddr, time::Duration};

use call_core::{CallEventKind, CallId, CallState};
use call_engine::{CallEngine, EngineConfig};
use call_runtime::{CallRuntime, RuntimeOutput};
use sip_security::SourceIpPolicy;
use sip_transport::UdpTransport;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Success,
    Busy,
    Cancel,
}

impl Outcome {
    fn parse(value: &str) -> Result<Self, io::Error> {
        match value {
            "success" => Ok(Self::Success),
            "busy" => Ok(Self::Busy),
            "cancel" => Ok(Self::Cancel),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported SIPp fixture outcome {value:?}"),
            )),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let bind_address = arguments
        .next()
        .ok_or_else(|| invalid_input("usage: sipp_uas <bind-address> <success|busy|cancel>"))?
        .parse::<SocketAddr>()?;
    let outcome = Outcome::parse(
        &arguments
            .next()
            .ok_or_else(|| invalid_input("missing SIPp fixture outcome"))?,
    )?;
    if arguments.next().is_some() {
        return Err(invalid_input("unexpected extra SIPp fixture argument").into());
    }

    let transport = UdpTransport::bind(bind_address, 16 * 1_024)?;
    let local_address = transport.local_addr()?;
    let mut policy = SourceIpPolicy::default();
    policy.add_allow("127.0.0.0/8")?;
    let mut runtime = CallRuntime::udp_with_source_policy(
        CallEngine::new(EngineConfig::default())?,
        transport,
        policy,
    );

    println!("READY {local_address} {outcome:?}");
    let invite = runtime.receive_once(Duration::ZERO)?;
    let call_id = created_call(&invite)?;

    match outcome {
        Outcome::Success => run_success(&mut runtime, &call_id)?,
        Outcome::Busy => run_busy(&mut runtime, &call_id)?,
        Outcome::Cancel => run_cancel(&mut runtime, &call_id)?,
    }

    let snapshot = runtime.engine().snapshot(&call_id)?;
    if snapshot.state != CallState::Ended {
        return Err(invalid_input(format!(
            "SIPp fixture ended in {:?}, expected Ended",
            snapshot.state
        ))
        .into());
    }
    runtime.engine_mut().reclaim_terminal_call(&call_id)?;
    if !runtime.engine().list(1)?.is_empty() {
        return Err(invalid_input("terminal SIPp fixture call was not reclaimed").into());
    }

    println!("COMPLETE {local_address} {outcome:?} {call_id}");
    Ok(())
}

fn run_success(runtime: &mut CallRuntime, call_id: &CallId) -> Result<(), Box<dyn Error>> {
    runtime.respond_to_invite(call_id, 180, "Ringing", Vec::new(), Duration::ZERO)?;
    runtime.respond_to_invite(call_id, 200, "OK", Vec::new(), Duration::ZERO)?;
    let acknowledgement = runtime.receive_once(Duration::ZERO)?;
    ensure_no_actions("ACK", &acknowledgement)?;
    let bye = runtime.receive_once(Duration::ZERO)?;
    ensure_one_action("BYE", &bye)?;
    Ok(())
}

fn run_busy(runtime: &mut CallRuntime, call_id: &CallId) -> Result<(), Box<dyn Error>> {
    runtime.respond_to_invite(call_id, 486, "Busy Here", Vec::new(), Duration::ZERO)?;
    let acknowledgement = runtime.receive_once(Duration::ZERO)?;
    ensure_no_actions("failure ACK", &acknowledgement)?;
    Ok(())
}

fn run_cancel(runtime: &mut CallRuntime, call_id: &CallId) -> Result<(), Box<dyn Error>> {
    runtime.respond_to_invite(call_id, 180, "Ringing", Vec::new(), Duration::ZERO)?;
    let cancellation = runtime.receive_once(Duration::ZERO)?;
    if cancellation.actions().len() != 2 {
        return Err(invalid_input(format!(
            "CANCEL emitted {} actions, expected 200 and 487",
            cancellation.actions().len()
        ))
        .into());
    }
    let acknowledgement = runtime.receive_once(Duration::ZERO)?;
    ensure_no_actions("cancel ACK", &acknowledgement)?;
    Ok(())
}

fn created_call(output: &RuntimeOutput) -> Result<CallId, io::Error> {
    output
        .events()
        .iter()
        .find(|event| event.kind == CallEventKind::Created)
        .map(|event| event.call_id.clone())
        .ok_or_else(|| invalid_input("INVITE did not create a call"))
}

fn ensure_no_actions(label: &str, output: &RuntimeOutput) -> Result<(), io::Error> {
    if output.actions().is_empty() {
        Ok(())
    } else {
        Err(invalid_input(format!(
            "{label} emitted {} unexpected actions",
            output.actions().len()
        )))
    }
}

fn ensure_one_action(label: &str, output: &RuntimeOutput) -> Result<(), io::Error> {
    if output.actions().len() == 1 {
        Ok(())
    } else {
        Err(invalid_input(format!(
            "{label} emitted {} actions, expected one",
            output.actions().len()
        )))
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
