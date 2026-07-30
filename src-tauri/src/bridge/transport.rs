//! High-level PAX operations — ports the `paxService.js` section of `pax.js`.
//!
//! Turns intent (sale/refund/void/ping/batch) into protocol field arrays,
//! sends them via the live terminal transport (TCP or USB), and parses the
//! response frames into structured, cents-based objects.
//!
//! Concurrency: PAX terminals handle exactly ONE transaction at a time. Each
//! terminal (keyed by `tcp:ip:port` or `usb:/dev/...`) gets a serialized queue
//! so commands never interleave on the wire. `is_busy()` lets callers fail
//! fast with 409 TERMINAL_BUSY instead of queueing.

use crate::bridge::config;
use crate::bridge::db::Terminal;
use crate::bridge::protocol::{self, BatchResponse, CreditFieldsInput, CreditResponse, Field, InitializeInfo, OnState, PaxError};
use crate::bridge::serial::{self, SerialPortInfo};
use crate::bridge::tcp;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum Target {
    Tcp { ip: String, port: u16 },
    Usb { path: String, baud_rate: u32 },
}

impl Target {
    pub fn from_terminal(t: &Terminal) -> Self {
        if t.conn_type == "usb" {
            Target::Usb { path: t.serial_path.clone(), baud_rate: t.baud_rate }
        } else {
            Target::Tcp { ip: t.ip.clone(), port: t.port }
        }
    }

    pub fn key(&self) -> String {
        match self {
            Target::Tcp { ip, port } => format!("tcp:{}:{}", ip, port),
            Target::Usb { path, .. } => format!("usb:{}", path),
        }
    }
}

struct TerminalQueue {
    lock: tokio::sync::Mutex<()>,
    pending: AtomicUsize,
}

static QUEUES: OnceLock<StdMutex<HashMap<String, Arc<TerminalQueue>>>> = OnceLock::new();

/// Cancellation tokens for commands currently in flight, keyed by terminal.
/// Lets `cancel()` abort a card prompt the cashier no longer wants to wait on.
static CANCELS: OnceLock<StdMutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn cancels() -> &'static StdMutex<HashMap<String, CancellationToken>> {
    CANCELS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Abort the in-flight command for this terminal, if any.
///
/// This drops the ECR connection mid-transaction, which is what releases the
/// terminal from the prompt. It is NOT a protocol-level "void": if the card was
/// already authorized in the split second before cancelling, the cancel does not
/// reverse it — callers must treat the outcome as unknown and verify.
/// Returns false if nothing was in flight.
pub fn cancel(terminal: &Terminal) -> bool {
    let key = Target::from_terminal(terminal).key();
    let guard = cancels().lock().unwrap();
    match guard.get(&key) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

/// Removes this terminal's cancel token when the command finishes, so a later
/// cancel can never abort an unrelated command.
struct CancelGuard(String);
impl Drop for CancelGuard {
    fn drop(&mut self) {
        cancels().lock().unwrap().remove(&self.0);
    }
}

fn queues() -> &'static StdMutex<HashMap<String, Arc<TerminalQueue>>> {
    QUEUES.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn queue_for(key: &str) -> Arc<TerminalQueue> {
    let mut guard = queues().lock().unwrap();
    guard
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(TerminalQueue { lock: tokio::sync::Mutex::new(()), pending: AtomicUsize::new(0) }))
        .clone()
}

/// True if a command is already in flight/queued for this terminal.
pub fn is_busy(terminal: &Terminal) -> bool {
    let key = Target::from_terminal(terminal).key();
    let guard = queues().lock().unwrap();
    guard.get(&key).map(|q| q.pending.load(Ordering::SeqCst) > 0).unwrap_or(false)
}

struct PendingGuard(Arc<TerminalQueue>);
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Send a command to a terminal and await its response frame, serialized
/// per-terminal so only one command is ever in flight on the wire.
async fn send_command(terminal: &Terminal, fields: Vec<Field>, timeout_ms: u64, on_state: Option<OnState>) -> Result<protocol::ParsedResponse, PaxError> {
    let target = Target::from_terminal(terminal);
    let key = target.key();
    let q = queue_for(&key);

    q.pending.fetch_add(1, Ordering::SeqCst);
    let _pending_guard = PendingGuard(q.clone());

    // Serialize: only one in-flight command at a time per terminal.
    let _lock = q.lock.lock().await;

    let command = match fields.first() {
        Some(Field::Single(s)) => s.clone(),
        _ => String::new(),
    };
    let expected = protocol::response_for(&command).map(|s| s.to_string());
    let request = protocol::build_message(&fields);

    // Registered only now that we hold the queue lock, so the token always
    // belongs to the command actually on the wire.
    let token = CancellationToken::new();
    cancels().lock().unwrap().insert(key.clone(), token.clone());
    let _cancel_guard = CancelGuard(key);

    let send = async {
        match target {
            Target::Tcp { ip, port } => tcp::send_command(&ip, port, &request, expected.as_deref(), timeout_ms, on_state).await,
            Target::Usb { path, baud_rate } => serial::send_command(&path, baud_rate, request, expected, timeout_ms, on_state).await,
        }
    };

    // Dropping `send` on cancel closes the socket / serial handle, which is what
    // releases the terminal from its card prompt.
    tokio::select! {
        result = send => result,
        _ = token.cancelled() => Err(PaxError::new(
            "CANCELED",
            "Cancelled from the point of sale before the terminal responded.",
        )),
    }
}

/// List serial ports available on the host (for the "detect USB device" UI).
pub async fn list_serial_ports() -> Result<Vec<SerialPortInfo>, PaxError> {
    serial::list_ports().await
}

/// A00 initialize / ping. Returns terminal info + latencyMs.
pub async fn initialize(terminal: &Terminal, on_state: Option<OnState>) -> Result<InitializeInfo, PaxError> {
    let started = Instant::now();
    let fields = vec![Field::Single(protocol::COMMAND_INITIALIZE.to_string()), Field::Single(config::protocol_version())];
    let parsed = send_command(terminal, fields, config::ping_timeout_ms(), on_state).await?;
    let mut info = protocol::parse_initialize(&parsed);
    info.latency_ms = started.elapsed().as_millis() as i64;
    Ok(info)
}

/// T00 SALE. amountCents/tipCents are integer cents.
pub async fn sale(terminal: &Terminal, amount_cents: i64, ecr_ref_num: String, tip_cents: i64, on_state: Option<OnState>) -> Result<CreditResponse, PaxError> {
    let fields = protocol::build_credit_fields(CreditFieldsInput {
        txn_type: protocol::TXN_TYPE_SALE,
        amount_cents,
        tip_cents,
        ecr_ref_num,
        cashier_id: String::new(),
        orig_ref_num: None,
        orig_trans_num: None,
    });
    let parsed = send_command(terminal, fields, config::payment_timeout_ms(), on_state).await?;
    Ok(protocol::parse_credit_response(&parsed))
}

/// T00 RETURN / refund.
pub async fn refund(terminal: &Terminal, amount_cents: i64, ecr_ref_num: String, on_state: Option<OnState>) -> Result<CreditResponse, PaxError> {
    let fields = protocol::build_credit_fields(CreditFieldsInput {
        txn_type: protocol::TXN_TYPE_RETURN,
        amount_cents,
        tip_cents: 0,
        ecr_ref_num,
        cashier_id: String::new(),
        orig_ref_num: None,
        orig_trans_num: None,
    });
    let parsed = send_command(terminal, fields, config::payment_timeout_ms(), on_state).await?;
    Ok(protocol::parse_credit_response(&parsed))
}

/// T00 VOID. Voids a previous transaction by its original ref number.
pub async fn void_transaction(
    terminal: &Terminal,
    orig_ref_num: String,
    ecr_ref_num: String,
    amount_cents: i64,
    orig_trans_num: Option<String>,
    on_state: Option<OnState>,
) -> Result<CreditResponse, PaxError> {
    let fields = protocol::build_credit_fields(CreditFieldsInput {
        txn_type: protocol::TXN_TYPE_VOID,
        amount_cents,
        tip_cents: 0,
        ecr_ref_num,
        cashier_id: String::new(),
        orig_ref_num: Some(orig_ref_num),
        orig_trans_num,
    });
    let parsed = send_command(terminal, fields, config::payment_timeout_ms(), on_state).await?;
    Ok(protocol::parse_credit_response(&parsed))
}

/// B00 batch close / settle.
pub async fn batch_close(terminal: &Terminal) -> Result<BatchResponse, PaxError> {
    let fields = vec![
        Field::Single(protocol::COMMAND_BATCH_CLOSE.to_string()),
        Field::Single(config::protocol_version()),
        Field::Single(protocol::EDC_TYPE_ALL.to_string()),
    ];
    let parsed = send_command(terminal, fields, config::payment_timeout_ms(), None).await?;
    Ok(protocol::parse_batch_response(&parsed))
}

/// Lightweight LAN/USB diagnostics without sending a full payment. Helps
/// distinguish "device offline" vs "BroadPOS ECR not listening".
pub async fn diagnose(terminal: &Terminal) -> Result<Value, PaxError> {
    let protocol_version = config::protocol_version();

    if terminal.conn_type == "usb" {
        let ports = serial::list_ports().await?;
        let path = terminal.serial_path.clone();
        let matching = ports.iter().find(|p| p.path == path).cloned();
        let port_present = matching.is_some();
        let next_steps: Vec<&str> = if port_present {
            vec![
                "Serial device is present. Run Test Connection.",
                "In BroadPOS: External POS / ECR ON, Communication = USB, leave idle.",
            ]
        } else {
            vec![
                "No matching USB serial device. Set Android USB to PAX POSVCOM USB MODE.",
                "In BroadPOS: External POS / ECR ON, Communication = USB.",
                "Replug the USB cable into this Mac, then Detect again.",
            ]
        };
        return Ok(json!({
            "connType": "usb",
            "protocolVersion": protocol_version,
            "serialPath": path,
            "portPresent": port_present,
            "portsFound": ports.len(),
            "matchingPort": matching,
            "ecrLikelyListening": port_present,
            "nextSteps": next_steps,
        }));
    }

    let ip = terminal.ip.clone();
    let port = if terminal.port != 0 { terminal.port } else { 10009 };
    let (open, host_reachable, error) = tcp::probe(&ip, port, 3000).await;
    let next_steps: Vec<&str> = if open {
        vec!["TCP port is open — BroadPOS ECR appears to be listening. Run Test Connection."]
    } else if host_reachable {
        vec![
            "Device is on the network but port is closed — BroadPOS ECR is not listening.",
            "Open BroadPOS TSYS Sierra → Settings (squares) → password = today's date MMDDYYYY (try ±1 day).",
            "System Settings → ECR-Terminal Integration Mode → External POS.",
            "ECR Comm Settings → Protocol Type = TCP/IP, Host Port = 10009.",
            "Leave BroadPOS on the idle / ready screen, then Test Connection again.",
        ]
    } else {
        vec!["Cannot reach this IP. Confirm the terminal Wi-Fi IP and that the Mac is on the same LAN (AP isolation off)."]
    };

    Ok(json!({
        "connType": "tcp",
        "protocolVersion": protocol_version,
        "ip": ip,
        "port": port,
        "hostReachable": host_reachable,
        "tcpOpen": open,
        "tcpError": error,
        "ecrLikelyListening": open,
        "nextSteps": next_steps,
    }))
}
