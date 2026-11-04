//! Fake phone: connects to the desktop app exactly like the phone app does and streams a
//! beeping tone, so the desktop side can be tested without a phone.
//!
//!   cargo run --example fake_phone -- [--host 127.0.0.1] [--name "Fake Phone"]
//!       [--freq 440] [--loss 0] [--jitter 0] [--seconds 0] [--muted] [--code 1234]
//!
//! --loss is the percentage of packets to drop, --jitter the maximum random extra delay
//! in ms, --seconds 0 runs until Ctrl-C.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::UdpSocket;
use tokio_tungstenite::tungstenite::Message;
use tstv_phone_mics::opus::Encoder;
use tstv_phone_mics::protocol::{self, Datagram, FRAME_SAMPLES, KIND_PONG, SAMPLE_RATE};

struct Args {
    host: String,
    name: String,
    freq: f64,
    loss: f64,
    jitter: f64,
    seconds: f64,
    muted: bool,
    code: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args { host: "127.0.0.1".into(), name: "Fake Phone".into(), freq: 440.0, loss: 0.0, jitter: 0.0, seconds: 0.0, muted: false, code: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--host" => a.host = value(),
            "--name" => a.name = value(),
            "--freq" => a.freq = value().parse().expect("--freq"),
            "--loss" => a.loss = value().parse().expect("--loss"),
            "--jitter" => a.jitter = value().parse().expect("--jitter"),
            "--seconds" => a.seconds = value().parse().expect("--seconds"),
            "--muted" => a.muted = true,
            "--code" => a.code = Some(value()),
            other => panic!("unknown option {other}"),
        }
    }
    a
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let ipv6 = args.host.contains(':');
    let url = if ipv6 {
        format!("ws://[{}]:{}/", args.host, protocol::CONTROL_PORT)
    } else {
        format!("ws://{}:{}/", args.host, protocol::CONTROL_PORT)
    };
    println!("Connecting to {url} …");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("can't reach the desktop app");
    let (mut sink, mut source) = ws.split();

    let hello = json!({
        "type": "hello", "protocol": 1, "deviceId": format!("fake-{}", args.name),
        "name": args.name, "platform": "test", "model": "fake_phone", "appVersion": env!("CARGO_PKG_VERSION"),
        "code": args.code,
    });
    sink.send(Message::Text(hello.to_string().into())).await.unwrap();

    let welcome = loop {
        match source.next().await {
            Some(Ok(Message::Text(t))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str() {
                    Some("welcome") => break v,
                    Some("reject") => {
                        eprintln!("Rejected: {}", v["message"]);
                        std::process::exit(1);
                    }
                    _ => {}
                }
            }
            other => panic!("connection closed before welcome: {other:?}"),
        }
    };
    let token = welcome["token"].as_u64().unwrap() as u32;
    let udp_port = welcome["udpPort"].as_u64().unwrap() as u16;
    println!("Joined slot {} → output channel {}", welcome["slot"], welcome["channel"]);

    let status = json!({
        "type": "status", "talentMuted": args.muted,
        "input": { "name": "Fake phone (beeps)", "kind": "other" },
        "captureLatencyMs": 10.0 + 2.5, "inputSampleRate": 48000, "battery": null, "interrupted": false,
    });
    sink.send(Message::Text(status.to_string().into())).await.unwrap();

    let sock = Arc::new(UdpSocket::bind(if ipv6 { "[::]:0" } else { "0.0.0.0:0" }).await.unwrap());
    sock.connect((args.host.as_str(), udp_port)).await.unwrap();

    // Answer pings.
    let pong_sock = sock.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 64];
        while let Ok(n) = pong_sock.recv(&mut buf).await {
            if let Some(Datagram::Ping { token, stamp }) = protocol::parse(&buf[..n]) {
                let _ = pong_sock.send(&protocol::write_ping(KIND_PONG, token, stamp)).await;
            }
        }
    });

    // Print what the desktop reports.
    tokio::spawn(async move {
        while let Some(Ok(msg)) = source.next().await {
            if let Message::Text(t) = msg {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                match v["type"].as_str() {
                    Some("stats") => println!(
                        "desktop: latency {:.0} ms, loss {:.1}%, channel {}",
                        v["latencyMs"].as_f64().unwrap_or(0.0),
                        v["lossPct"].as_f64().unwrap_or(0.0),
                        v["channel"]
                    ),
                    Some("operator") => println!("desktop: operator muted = {}", v["operatorMuted"]),
                    _ => {}
                }
            }
        }
        println!("Desktop closed the connection.");
        std::process::exit(0);
    });

    let mut enc = Encoder::new(protocol::DEFAULT_BITRATE as i32).unwrap();
    let mut seq: u32 = rand::random();
    let mut prev = Vec::new();
    let mut phase = 0.0f64;
    let mut sample_index: u64 = 0;
    let sent = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    let mut packet = Vec::new();
    loop {
        tick.tick().await;
        if args.seconds > 0.0 && start.elapsed().as_secs_f64() > args.seconds {
            break;
        }
        // 200 ms beep every second.
        let mut pcm = [0f32; FRAME_SAMPLES];
        for s in pcm.iter_mut() {
            let in_beep = (sample_index % SAMPLE_RATE as u64) < (SAMPLE_RATE as u64 / 5);
            *s = if in_beep { (0.4 * (phase * std::f64::consts::TAU).sin()) as f32 } else { 0.0 };
            phase = (phase + args.freq / SAMPLE_RATE as f64).fract();
            sample_index += 1;
        }
        let primary = if args.muted {
            Vec::new()
        } else {
            let mut buf = [0u8; 1500];
            let n = enc.encode(&pcm, &mut buf).unwrap();
            buf[..n].to_vec()
        };
        let flags = if args.muted { protocol::FLAG_TALENT_MUTED } else { 0 };
        protocol::write_audio(&mut packet, token, seq, flags, &primary, &prev);
        prev = primary;
        seq = seq.wrapping_add(1);

        if rand::random::<f64>() * 100.0 < args.loss {
            continue;
        }
        let delay = rand::random::<f64>() * args.jitter;
        let (sock, data, sent) = (sock.clone(), packet.clone(), sent.clone());
        if delay < 0.5 {
            let _ = sock.send(&data).await;
            sent.fetch_add(1, Relaxed);
        } else {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs_f64(delay / 1000.0)).await;
                let _ = sock.send(&data).await;
                sent.fetch_add(1, Relaxed);
            });
        }
    }
    println!("Done: sent {} packets.", sent.load(Relaxed));
}
