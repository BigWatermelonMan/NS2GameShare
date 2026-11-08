use anyhow::{Context, Result, bail, ensure};
use c5play::{
    ldn::{self, Host, Keys},
    lrp::{ControlDecoder, MatchingInfo},
    net::Network,
    pcap,
    radio::Radio,
    wire,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::SyncSender,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub struct Hooks {
    pub video: SyncSender<c5play::lsp::MediaUnit>,
    pub audio: Option<SyncSender<c5play::lsp::MediaUnit>>,
    pub controls: Arc<Mutex<c5play::feedback::Controls>>,
    pub stop: Arc<AtomicBool>,
}
fn now(start: Instant) -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_millis(start.elapsed().as_millis() as i64)
}
pub struct Options {
    pub name: String,
    pub port: String,
    pub baud: u32,
    pub channels: String,
    pub scan_seconds: u64,
    pub seconds: u64,
    pub output: Option<PathBuf>,
    pub probe_control: bool,
    pub control_request_body: Option<PathBuf>,
    pub request_session: bool,
    pub link_baud: Option<u32>,
    pub receive_kbps: u32,
    pub video_size: (u32, u32),
}
pub fn run(keys: &Keys, passphrase: &[u8], options: Options) -> Result<()> {
    run_with_hooks(keys, passphrase, options, None)
}
pub fn run_with_hooks(
    keys: &Keys,
    passphrase: &[u8],
    options: Options,
    hooks: Option<Hooks>,
) -> Result<()> {
    let Options {
        name,
        port,
        baud,
        channels,
        scan_seconds,
        seconds,
        output,
        probe_control,
        control_request_body,
        request_session,
        link_baud,
        receive_kbps,
        video_size,
    } = options;
    c5play::profile::validate_name(&name)?;
    ensure!(
        (600..=2000).contains(&receive_kbps),
        "receive-kbps must be 600..2000"
    );
    ensure!(
        (320..=1920).contains(&video_size.0) && (180..=1080).contains(&video_size.1),
        "invalid video size"
    );
    ensure!(
        (16..=64).contains(&passphrase.len()),
        "passphrase file must contain 16..64 raw bytes"
    );
    let mut radio = Radio::open(&port, baud)?;
    let hello = radio.greet()?;
    println!("{}", hello);
    for c in [
        "stop",
        "mode monitor",
        "sta leave",
        "filter 5",
        "rssi -90",
        "mac clear",
    ] {
        radio.command(c)?;
    }
    if let Some(rate) = link_baud
        && hello.contains("port=uart")
    {
        radio.switch_baud(rate)?;
    }
    let mut channels: Vec<u8> = channels
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if hello.contains("board=basic") {
        channels.retain(|channel| *channel <= 14);
    }
    ensure!(
        !channels.is_empty() && channels.len() <= 32,
        "invalid channel list"
    );
    let cmd = if channels.len() == 1 {
        format!("ch {}", channels[0])
    } else {
        format!(
            "hop 200 {}",
            channels
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    radio.command(&cmd)?;
    radio.command("start")?;
    println!("探索中：Switch 2 でおすそわけを開始してください。");
    let deadline = Instant::now() + Duration::from_secs(scan_seconds);
    let host = 'scan: loop {
        ensure!(
            !c5play::cancel::requested()
                && hooks
                    .as_ref()
                    .is_none_or(|h| !h.stop.load(Ordering::Relaxed)),
            "cancelled"
        );
        ensure!(
            scan_seconds == 0 || Instant::now() < deadline,
            "GameShare host not found before scan timeout"
        );
        for (kind, p) in radio.poll()? {
            if kind == wire::PKT && p.len() >= 24 && ldn::is_advertisement(&p[24..]) {
                let h = Host::decode(&p[24..], p[10], keys)?;
                if MatchingInfo::decode(&h.app).is_ok() {
                    break 'scan h;
                }
            }
        }
    };
    let info = MatchingInfo::decode(&host.app)?;
    println!(
        "ホスト {} ch={} 映像={}x{} {}fps 制御ポート={}",
        ldn::mac(&host.bssid),
        host.channel,
        info.width,
        info.height,
        info.fps,
        info.control_port
    );
    // A fresh transmitter address avoids reusing CCMP (TK, MAC, PN) after a C5 reboot.
    let mut station_mac = [0x98, 0xb6, 0xe9, 0, 0, 0];
    loop {
        getrandom::fill(&mut station_mac[3..])
            .map_err(|_| anyhow::anyhow!("random source failed"))?;
        if !host.nodes.iter().any(|(mac, _)| *mac == station_mac) {
            break;
        }
    }
    radio.command(&format!("sta macset {}", ldn::mac(&station_mac)))?;
    let line = radio.command("sta mac")?;
    let mac: [u8; 6] = ldn::unhex(
        &line
            .split_whitespace()
            .last()
            .context("missing station MAC")?
            .replace(':', ""),
    )?
    .try_into()
    .map_err(|_| anyhow::anyhow!("invalid station MAC"))?;
    let tk = host.tk(keys, passphrase)?;
    radio.command(&format!(
        "sta set {} {} {} {}",
        host.ssid(),
        ldn::mac(&host.bssid),
        host.channel,
        ldn::hex(&tk)
    ))?;
    radio.command("sta join")?;
    let deadline = Instant::now() + Duration::from_secs(40);
    'join: loop {
        ensure!(
            !c5play::cancel::requested()
                && hooks
                    .as_ref()
                    .is_none_or(|h| !h.stop.load(Ordering::Relaxed)),
            "cancelled"
        );
        ensure!(Instant::now() < deadline, "station join timeout");
        for (kind, p) in radio.poll()? {
            if kind == wire::EVENT {
                let ev = String::from_utf8_lossy(&p);
                println!("{ev}");
                if ev.starts_with("sta linked") {
                    break 'join;
                }
                ensure!(!ev.starts_with("sta join fail"), "station join failed");
            }
        }
    }
    radio.command("filter 1")?;
    let auth = host.auth(keys, mac, &name)?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut next_auth = start;
    let mut attempts = 0;
    let mut authenticated = false;
    let mut net = None;
    let mut tcp_connected = false;
    let mut decoder = ControlDecoder::default();
    let mut writer = output.map(|p| pcap::Writer::create(&p)).transpose()?;
    let mut request = if let Some(path) = control_request_body {
        let body = std::fs::read(path)?;
        let (kind, _) = c5play::flat::envelope(&body)?;
        ensure!(kind == 2, "experimental request must be a control request");
        c5play::lrp::frame_unsigned(&body)?
    } else if request_session {
        c5play::lrp::session_request(&name, 51000, 51001, video_size.0, video_size.1)?
    } else {
        c5play::lrp::host_information_request(&info.host_user)?
    };
    let mut sent = 0;
    let mut udp = None;
    let mut udp_count = 0;
    let mut lsp = c5play::lsp::Reassembler::default();
    let mut video_count = 0;
    let mut peer_ip = None;
    let mut session_info: Option<c5play::lrp::SessionInfo> = None;
    let mut stream_id = None;
    let mut next_ntp = start;
    let mut next_input = start;
    let mut clock_offset = None;
    let mut best_rtt = u64::MAX;
    let mut input_sequence = 0u32;
    let mut input_encoder = c5play::feedback::Encoder::default();
    let mut input_packets = 0;
    let mut video_dropped = 0;
    let mut audio_dropped = 0;
    let mut audio_count = 0;
    let mut next_qos = start;
    let mut qos_gaps = 0;
    let mut quality = c5play::quality::Controller::new(receive_kbps);
    let mut last_capacity = 0;
    let mut control_serial = 2u16;
    let mut recovery = c5play::quality::Recovery::new(start);
    let mut next_health = start + Duration::from_secs(5);
    let mut device_tick = device_time(&hello);
    while (seconds == 0 || Instant::now() < deadline)
        && !c5play::cancel::requested()
        && hooks
            .as_ref()
            .is_none_or(|h| !h.stop.load(Ordering::Relaxed))
    {
        if Instant::now() >= next_health {
            radio.send("hello")?;
            radio.send("sta stats")?;
            println!(
                "通信状態 {} UDP={} video={} gaps={} invalid={} malformed={}",
                radio.stats(),
                udp_count,
                video_count,
                lsp.gaps,
                lsp.invalid,
                lsp.malformed
            );
            next_health = Instant::now() + Duration::from_secs(5);
        }
        if !authenticated && Instant::now() >= next_auth {
            ensure!(attempts < 3, "LDN authentication timed out");
            radio.ethernet(&auth.request)?;
            attempts += 1;
            next_auth = Instant::now() + Duration::from_millis(700);
        }
        for (kind, p) in radio.poll()? {
            match kind {
                wire::D2H => {
                    let eth = radio.receive_ethernet(&p)?;
                    if let Some(w) = &mut writer {
                        w.packet(eth)?;
                    }
                    if !authenticated && let Some(status) = auth.verify(eth)? {
                        ensure!(status == 0, "LDN authentication rejected: {status}");
                        authenticated = true;
                        println!("LDN 認証成功（応答 GCM 検証済み）");
                    }
                    if authenticated && let Some(n) = &mut net {
                        Network::receive(n, eth)?;
                    }
                }
                wire::PKT if p.len() >= 24 && authenticated && net.is_none() => {
                    let f = &p[24..];
                    if ldn::is_advertisement(f) && f[10..16] == host.bssid {
                        let updated = Host::decode(f, p[10], keys)?;
                        ensure!(
                            updated.session == host.session,
                            "host session changed; reconnect required"
                        );
                        if let Some(ip) = updated.ip(&mac) {
                            let host_ip = updated.ip(&host.bssid).context("host IPv4 missing")?;
                            println!(
                                "IPv4 {} / ホスト {}",
                                std::net::Ipv4Addr::from(ip),
                                std::net::Ipv4Addr::from(host_ip)
                            );
                            net = Some(Network::new(
                                mac,
                                ip,
                                host_ip,
                                info.control_port,
                                now(start),
                            )?);
                            peer_ip = Some(host_ip);
                        }
                    }
                }
                wire::EVENT => {
                    let ev = String::from_utf8_lossy(&p);
                    if ev.starts_with("sta disconnected")
                        || ev.starts_with("sta native disconnect")
                        || ev.starts_with("sta link dropped")
                    {
                        bail!("station disconnected")
                    }
                }
                wire::TEXT => {
                    let text = String::from_utf8_lossy(&p);
                    if text.starts_with("ok hello") {
                        if let (Some(previous), Some(current)) = (device_tick, device_time(&text)) {
                            ensure!(current >= previous, "ESP32 restarted: {text}");
                        }
                        device_tick = device_time(&text);
                        println!("{text}");
                    } else if text.starts_with("sta h2d=") {
                        println!("{text}");
                    }
                }
                _ => {}
            }
        }
        if let Some(n) = &mut net {
            if request_session && udp.is_none() {
                udp = Some((n.open_udp(51000)?, n.open_udp(51001)?, n.open_udp(51002)?));
            }
            let data = n.poll(now(start))?;
            if n.connected && !tcp_connected {
                tcp_connected = true;
                println!("LRP TCP 接続成功");
            }
            if (probe_control || request_session) && n.connected && sent < request.len() {
                sent += n.send(&request[sent..])?;
                if sent == request.len() {
                    println!("制御要求送信完了（session={}）", request_session);
                }
            }
            if let Some((feedback, media, ntp)) = udp {
                if let (Some(session), Some(ip)) = (&session_info, peer_ip) {
                    let local_ns = start.elapsed().as_nanos() as u64;
                    if Instant::now() >= next_ntp {
                        n.send_udp(
                            ntp,
                            &c5play::feedback::ntp_request(local_ns),
                            ip,
                            session.ntp_port,
                        )?;
                        next_ntp = Instant::now()
                            + Duration::from_millis(if clock_offset.is_none() { 30 } else { 1000 });
                    }
                    for packet in n.receive_udp(ntp)? {
                        if let Ok((offset, rtt)) =
                            c5play::feedback::ntp_offset(&packet, start.elapsed().as_nanos() as u64)
                            && rtt < best_rtt
                        {
                            best_rtt = rtt;
                            clock_offset = Some(offset);
                        }
                    }
                    if let Some(offset) = clock_offset
                        && Instant::now() >= next_input
                    {
                        let controls = hooks
                            .as_ref()
                            .map(|h| *h.controls.lock().unwrap())
                            .unwrap_or_default();
                        input_sequence = input_sequence.wrapping_add(1);
                        let mut next_encoder = input_encoder.clone();
                        let packet = next_encoder.packet(
                            controls,
                            input_sequence as u64,
                            input_sequence,
                            (local_ns as i128 + offset).max(0) as u64,
                            local_ns,
                        )?;
                        if n.send_udp(feedback, &packet, ip, session.feedback_port)? {
                            input_encoder = next_encoder;
                            input_packets += 1;
                        }
                        next_input = Instant::now() + Duration::from_millis(8);
                    }
                }
                for (handle, label) in [(feedback, "feedback"), (media, "media")] {
                    for packet in n.receive_udp(handle)? {
                        udp_count += 1;
                        if udp_count < 4 {
                            println!("UDP {label} {}B", packet.len());
                        }
                        if packet.starts_with(&[0xec, 0xc2]) {
                            for unit in lsp.push_resilient(&packet) {
                                if unit.stream == 0 {
                                    let keyframe =
                                        c5play::diagnose::nal_types(&unit.payload).any(|n| n == 5);
                                    recovery.observe(unit.sequence, keyframe);
                                    video_count += 1;
                                    if video_count <= 3 || video_count % 60 == 0 {
                                        println!(
                                            "H264 unit={} {}B kind={}",
                                            video_count,
                                            unit.payload.len(),
                                            unit.kind
                                        );
                                    }
                                    if let Some(h) = &hooks {
                                        let sequence = unit.sequence;
                                        if h.video.try_send(unit).is_err() {
                                            video_dropped += 1;
                                            recovery.dropped(sequence);
                                        }
                                    }
                                } else if unit.stream == 1 {
                                    audio_count += 1;
                                    if let Some(sender) =
                                        hooks.as_ref().and_then(|h| h.audio.as_ref())
                                        && sender.try_send(unit).is_err()
                                    {
                                        audio_dropped += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !data.is_empty() {
                for record in decoder.push(&data)? {
                    let (kind, payload) = c5play::flat::envelope(&record.body)?;
                    println!(
                        "LRP 受信 body={} type={}（署名検証は未実装）",
                        record.body.len(),
                        kind
                    );
                    if kind == 4
                        && let Some(stream) = c5play::lrp::VideoStream::notice(payload)?
                    {
                        println!(
                            "ホスト映像ストリーム id={:08x} {}x{}",
                            stream.id, stream.width, stream.height
                        );
                        if stream_id != Some(stream.id) {
                            stream_id = Some(stream.id);
                            control_serial = control_serial.wrapping_add(1).max(1);
                            if sent == request.len() {
                                request.clear();
                                sent = 0;
                            }
                            request
                                .extend(c5play::lrp::request_keyframe(stream.id, control_serial)?);
                        }
                    }
                    if kind == 3
                        && let Some(header) = payload.table(0)?
                    {
                        let status = header.u16(3, 0)?;
                        println!("LRP response status={status}");
                        ensure!(
                            status == 200,
                            "LRP request rejected (unverified signature), status={status}"
                        );
                        if request_session && payload.u8(1, 0)? == 1 {
                            let session = c5play::lrp::SessionInfo::decode(
                                payload.table(2)?.context("session reply body missing")?,
                            )?;
                            println!(
                                "参加セッションを取得 feedback={} ntp={}",
                                session.feedback_port, session.ntp_port
                            );
                            request = c5play::lrp::controller_info()?;
                            sent = 0;
                            session_info = Some(session);
                        }
                    }
                }
            }
            if let Some(stream) = stream_id
                && udp_count > 0
                && sent == request.len()
                && Instant::now() >= next_qos
            {
                let missing = lsp.gaps.saturating_sub(qos_gaps);
                let delay = radio.take_transit_delay();
                let flags = if missing > 0 || delay >= Duration::from_millis(60) {
                    4
                } else {
                    0
                };
                qos_gaps = lsp.gaps;
                let capacity = quality.update(missing, delay);
                if capacity != last_capacity {
                    println!(
                        "映像受信容量 {} kbps（直近欠落={} USB滞留={}ms）",
                        capacity / 1000,
                        missing,
                        delay.as_millis()
                    );
                    last_capacity = capacity;
                }
                control_serial = control_serial.wrapping_add(1).max(1);
                request = c5play::lrp::qos(stream, control_serial, flags, capacity, 100_000)?;
                if let Some((first, last)) = recovery.request(Instant::now()) {
                    control_serial = control_serial.wrapping_add(1).max(1);
                    request.extend(c5play::lrp::invalidate(
                        stream,
                        control_serial,
                        first,
                        last,
                    )?);
                    println!("映像参照の再同期要求 frames={first}..{last}");
                }
                sent = 0;
                next_qos = Instant::now() + Duration::from_millis(500);
            }
            while let Some(p) = n.device.tx.pop_front() {
                if let Some(w) = &mut writer {
                    w.packet(&p)?
                }
                radio.ethernet(&p)?;
            }
        }
    }
    println!("{}", radio.stats());
    println!(
        "UDP={} video={} LSP gaps={} invalid={} malformed={}",
        udp_count, video_count, lsp.gaps, lsp.invalid, lsp.malformed
    );
    println!(
        "input_packets={} clock_sync={} video_queue_dropped={}",
        input_packets,
        clock_offset.is_some(),
        video_dropped
    );
    println!(
        "audio_packets={} audio_queue_dropped={}",
        audio_count, audio_dropped
    );
    ensure!(authenticated, "LDN authentication was not verified");
    let n = net.context("IPv4 assignment not observed")?;
    println!("TCP state={:?} 受信={}B", n.state(), n.received);
    ensure!(tcp_connected, "LRP TCP connection was not established");
    Ok(())
}
fn device_time(text: &str) -> Option<u64> {
    text.split_whitespace()
        .find_map(|v| v.strip_prefix("t=")?.parse().ok())
}
