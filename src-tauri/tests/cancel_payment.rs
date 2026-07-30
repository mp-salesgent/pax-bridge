//! Proves transport::cancel() aborts a command that is still waiting on the
//! terminal, against a fake PAX terminal that accepts the connection and then
//! deliberately never responds (i.e. a card sitting at the prompt).

use pax_bridge_desktop_lib::bridge::db::Terminal;
use pax_bridge_desktop_lib::bridge::transport;
use std::time::{Duration, Instant};

async fn fake_terminal() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            // Hold it open and never reply: the terminal is prompting for a card.
            tokio::spawn(async move {
                let _held = stream;
                tokio::time::sleep(Duration::from_secs(120)).await;
            });
        }
    });
    port
}

fn terminal_on(port: u16) -> Terminal {
    Terminal {
        id: "t1".into(),
        name: "Fake".into(),
        model: "A35".into(),
        conn_type: "tcp".into(),
        ip: "127.0.0.1".into(),
        port,
        serial_path: String::new(),
        baud_rate: 9600,
        created_at: String::new(),
    }
}

#[tokio::test]
async fn cancel_aborts_a_waiting_sale() {
    // Long timeout so the ONLY thing that can end this early is our cancel.
    std::env::set_var("PAX_PAYMENT_TIMEOUT_MS", "120000");

    let port = fake_terminal().await;
    let terminal = terminal_on(port);

    let sale_terminal = terminal.clone();
    let started = Instant::now();
    let sale = tokio::spawn(async move {
        transport::sale(&sale_terminal, 11000, "1".to_string(), 0, None).await
    });

    // Wait until the command is genuinely in flight.
    let mut in_flight = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if transport::is_busy(&terminal) {
            in_flight = true;
            break;
        }
    }
    assert!(in_flight, "sale never became in-flight");
    // Give the socket a moment to actually connect+write before cancelling.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(transport::cancel(&terminal), "cancel found nothing in flight");

    let result = tokio::time::timeout(Duration::from_secs(10), sale)
        .await
        .expect("sale did not return after cancel — it hung")
        .unwrap();

    let err = result.expect_err("cancelled sale should not report success");
    assert_eq!(err.code, "CANCELED", "unexpected error: {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "cancel did not short-circuit the 120s payment timeout"
    );
    println!("OK: cancelled in {:?} with code {}", started.elapsed(), err.code);

    // Nothing left registered, and a second cancel is a no-op.
    assert!(!transport::cancel(&terminal), "cancel token outlived the command");
}

#[tokio::test]
async fn cancel_with_nothing_in_flight_is_a_noop() {
    let terminal = terminal_on(1);
    assert!(!transport::cancel(&terminal));
}
