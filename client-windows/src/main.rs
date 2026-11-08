#![cfg_attr(windows, windows_subsystem = "windows")]
mod connect;
use anyhow::{Context, Result, ensure};
use c5play::{
    ldn::{self, Host, Keys},
    lrp::MatchingInfo,
    pcap,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    version,
    name = "NS2GameShare",
    about = "NS2GameShare — Native Windows GameShare client"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Extract from your own mounted SYSTEM / splay NCA / main NSO; never prints secret bytes.
    ExtractPassphrase {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Open the native settings and connection window.
    #[cfg(windows)]
    Gui {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Read board identity without scanning or joining Wi-Fi.
    DeviceInfo {
        #[arg(long)]
        port: String,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
    },
    /// List FX2LP USB device paths (driver installation is separate).
    #[cfg(windows)]
    Fx2List,
    /// Check bridge signature, configuration, High Speed and bulk endpoints.
    #[cfg(windows)]
    Fx2Status,
    /// Probe raw FIFO bytes and Basic link diagnostics without joining Wi-Fi.
    #[cfg(windows)]
    Fx2Probe {
        #[arg(long, default_value_t = 6)]
        seconds: u64,
        #[arg(long)]
        bench: bool,
        #[arg(long)]
        raw: Option<PathBuf>,
    },
    /// Load a RAM-only FX2LP bridge image; does not write EEPROM.
    #[cfg(windows)]
    Fx2Load {
        file: PathBuf,
        #[arg(long)]
        device: Option<String>,
    },
    /// Exercise the complete Basic/FX2 wire link with deterministic benchmark frames.
    #[cfg(windows)]
    LinkBench {
        #[arg(long, default_value = "fx2")]
        port: String,
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },
    #[cfg(windows)]
    AudioProbe { file: PathBuf },
    /// Exercise the audio renderer with paced Opus silence, without USB.
    #[cfg(windows)]
    AudioSelfTest {
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },
    /// Replay captured audio at its recorded arrival times, without USB or GPU.
    #[cfg(windows)]
    AudioReplay {
        file: PathBuf,
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// Analyze saved Ethernet/LSP traffic without opening a device or GPU.
    Diagnose {
        pcap: PathBuf,
        #[arg(long)]
        video_write: Option<PathBuf>,
    },
    /// Play a live GameShare session with GPU video and native input.
    #[cfg(windows)]
    Play {
        #[arg(long, default_value = "NS2GameShare")]
        name: String,
        #[arg(long, hide = true)]
        stop_event: Option<String>,
        #[arg(long)]
        fx2_firmware: Option<PathBuf>,
        #[arg(long, default_value = "auto")]
        port: String,
        #[arg(long, default_value_t = 2_000_000)]
        baud: u32,
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        passphrase_file: PathBuf,
        #[arg(long, default_value_t = 0)]
        seconds: u64,
        #[arg(long)]
        ethernet_write: Option<PathBuf>,
        #[arg(long)]
        input_log: Option<PathBuf>,
        #[arg(long)]
        input_config: Option<PathBuf>,
        #[arg(long, default_value_t = 2_000_000)]
        link_baud: u32,
        #[arg(long, default_value_t = 1400)]
        receive_kbps: u32,
        #[arg(long, default_value_t = 960)]
        video_width: u32,
        #[arg(long, default_value_t = 540)]
        video_height: u32,
        #[arg(long)]
        mute: bool,
        /// Hold video against the audio clock; adds presentation latency.
        #[arg(long)]
        av_sync: bool,
        #[arg(long)]
        denoise: bool,
    },
    /// List attached DirectInput game controllers and default mappings.
    #[cfg(windows)]
    Controllers,
    /// Play a local H.264 MP4 through Media Foundation / D3D11 and sample input.
    #[cfg(windows)]
    Preview {
        file: PathBuf,
        #[arg(long, default_value_t = 0)]
        seconds: u64,
        #[arg(long)]
        input_log: Option<PathBuf>,
    },
    /// Read authenticated matching information from a radiotap pcap.
    Inspect {
        pcap: PathBuf,
        #[arg(long)]
        keys: PathBuf,
    },
    /// Authenticate to GameShare and establish the LRP control TCP connection.
    Connect {
        #[arg(long, default_value = "NS2GameShare")]
        name: String,
        #[arg(long, default_value = "auto")]
        port: String,
        #[arg(long, default_value_t = 2_000_000)]
        baud: u32,
        #[arg(long)]
        keys: PathBuf,
        #[arg(long)]
        passphrase_file: PathBuf,
        #[arg(long, default_value = "1,6,11,36,40,44,48")]
        channels: String,
        #[arg(long, default_value_t = 30)]
        scan_seconds: u64,
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        #[arg(long)]
        ethernet_write: Option<PathBuf>,
        /// Request host information using the advertised host profile ID.
        #[arg(long)]
        probe_control: bool,
        /// Experimental FlatBuffers body for a read-only control request.
        #[arg(long, requires = "probe_control")]
        control_request_body: Option<PathBuf>,
        /// Request an actual GameShare session (experimental).
        #[arg(long, conflicts_with = "control_request_body")]
        request_session: bool,
        #[arg(long)]
        link_baud: Option<u32>,
    },
}
fn main() -> Result<()> {
    c5play::cancel::install()?;
    let command = Args::parse().command;
    #[cfg(windows)]
    let command = match command {
        None => return c5play::settings::run(None),
        Some(Command::Gui { config }) => return c5play::settings::run(config),
        Some(command) => command,
    };
    #[cfg(not(windows))]
    let command = command.context("a command is required")?;
    match command {
        Command::ExtractPassphrase {
            source,
            keys,
            output,
        } => {
            c5play::passphrase::extract_to_file(
                &source,
                &keys,
                &output,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            println!(
                "Extracted 16 bytes to the selected private output file. Secret contents are not displayed."
            );
            Ok(())
        }
        #[cfg(windows)]
        Command::Gui { .. } => unreachable!(),
        #[cfg(windows)]
        Command::Fx2Probe {
            seconds,
            bench,
            raw,
        } => c5play::fx2::probe(seconds, bench, raw.as_deref()),
        Command::DeviceInfo { port, baud } => {
            let mut radio = c5play::radio::Radio::open(&port, baud)?;
            println!("{}", radio.greet()?);
            Ok(())
        }
        #[cfg(windows)]
        Command::Fx2Status => {
            let mut device = c5play::fx2::Device::open(None)?;
            println!("FX2 B5FX v1: USB High Speed, configured, bulk OUT 02 / IN 86 (512 bytes)");
            device.diagnostics()?;
            Ok(())
        }
        #[cfg(windows)]
        Command::Fx2List => {
            for path in c5play::fx2::paths()? {
                println!("{path}");
            }
            Ok(())
        }
        #[cfg(windows)]
        Command::Fx2Load { file, device } => c5play::fx2::load(&file, device.as_deref()),
        #[cfg(windows)]
        Command::LinkBench { port, seconds } => link_bench(&port, seconds),
        #[cfg(windows)]
        Command::AudioProbe { file } => c5play::audio::probe(&file),
        #[cfg(windows)]
        Command::AudioSelfTest { seconds } => c5play::audio::self_test(seconds),
        #[cfg(windows)]
        Command::AudioReplay { file, seconds } => c5play::audio::replay(&file, seconds),
        Command::Diagnose { pcap, video_write } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&c5play::diagnose::run(
                    &pcap,
                    video_write.as_deref()
                )?)?
            );
            Ok(())
        }
        #[cfg(windows)]
        Command::Play {
            name,
            stop_event,
            fx2_firmware,
            port,
            baud,
            keys,
            passphrase_file,
            seconds,
            ethernet_write,
            input_log,
            input_config,
            link_baud,
            receive_kbps,
            video_width,
            video_height,
            mute,
            av_sync,
            denoise,
        } => {
            c5play::profile::validate_name(&name)?;
            if let Some(event) = stop_event {
                c5play::cancel::open_stop_event(&event)?;
            }
            if let Some(firmware) = fx2_firmware {
                ensure!(
                    port == "fx2",
                    "FX2 RAM自動ロードには --port fx2 を使用してください。"
                );
                c5play::fx2::load(&firmware, None)?;
                let mut ready = false;
                for _ in 0..30 {
                    if c5play::cancel::requested() {
                        anyhow::bail!("cancelled");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if c5play::fx2::Device::open(None).is_ok() {
                        ready = true;
                        break;
                    }
                }
                ensure!(ready, "FX2LPの再認識がタイムアウトしました。");
            }
            let input_config = c5play::input::Config::load(input_config.as_deref())?;
            let keys = Keys::load(&keys)?;
            let passphrase = std::fs::read(passphrase_file)?;
            let (video, receiver) = std::sync::mpsc::sync_channel(16);
            let timeline =
                std::sync::Arc::new(std::sync::Mutex::new(c5play::timeline::Timeline::default()));
            let audio = if mute {
                None
            } else {
                match c5play::audio::Playback::start(timeline.clone()) {
                    Ok(audio) => Some(audio),
                    Err(error) => {
                        eprintln!("音声の初期化に失敗: {error:#}");
                        None
                    }
                }
            };
            let controls =
                std::sync::Arc::new(std::sync::Mutex::new(c5play::feedback::Controls::default()));
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let hooks = connect::Hooks {
                video,
                audio: audio.as_ref().map(|a| a.sender.clone()),
                controls: controls.clone(),
                stop: stop.clone(),
            };
            let worker = std::thread::spawn(move || {
                connect::run_with_hooks(
                    &keys,
                    &passphrase,
                    connect::Options {
                        name,
                        port,
                        baud,
                        channels: "1,6,11,36,40,44,48".into(),
                        scan_seconds: 0,
                        seconds,
                        output: ethernet_write,
                        probe_control: false,
                        control_request_body: None,
                        request_session: true,
                        link_baud: Some(link_baud),
                        receive_kbps,
                        video_size: (video_width, video_height),
                    },
                    Some(hooks),
                )
            });
            let result = c5play::video::live(
                c5play::video::Live {
                    receiver,
                    timeline: audio.as_ref().filter(|_| av_sync).map(|_| timeline.clone()),
                    denoise,
                    controls,
                    stop: stop.clone(),
                    input_config,
                },
                seconds,
                input_log.as_deref(),
            );
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let network = worker
                .join()
                .map_err(|_| anyhow::anyhow!("network thread panicked"))?;
            if let Err(error) = &network {
                eprintln!("接続エラー: {error:#}");
            }
            result?;
            network
        }
        #[cfg(windows)]
        Command::Controllers => {
            println!(
                "{}",
                serde_json::to_string_pretty(&c5play::dinput::list()?)?
            );
            Ok(())
        }
        #[cfg(windows)]
        Command::Preview {
            file,
            seconds,
            input_log,
        } => c5play::video::preview(&file, seconds, input_log.as_deref()),
        Command::Inspect { pcap, keys } => {
            let keys = Keys::load(&keys)?;
            let mut p = pcap::Reader::open(&pcap)?;
            ensure!(p.linktype == 127, "inspect requires radiotap pcap");
            while let Some(b) = p.next_packet()? {
                if b.len() < 8 {
                    continue;
                }
                let len = u16::from_le_bytes(b[2..4].try_into()?) as usize;
                if let Some(f) = b.get(len..) {
                    if !ldn::is_advertisement(f) {
                        continue;
                    }
                    let h = Host::decode(f, 0, &keys)?;
                    if let Ok(info) = MatchingInfo::decode(&h.app) {
                        println!("{}", serde_json::to_string_pretty(&info)?);
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("no GameShare advertisement found")
        }
        Command::Connect {
            name,
            port,
            baud,
            keys,
            passphrase_file,
            channels,
            scan_seconds,
            seconds,
            ethernet_write,
            probe_control,
            control_request_body,
            request_session,
            link_baud,
        } => connect::run(
            &Keys::load(&keys)?,
            &std::fs::read(passphrase_file)?,
            connect::Options {
                name,
                port,
                baud,
                channels,
                scan_seconds,
                seconds,
                output: ethernet_write,
                probe_control,
                control_request_body,
                request_session,
                link_baud,
                receive_kbps: 1200,
                video_size: (640, 360),
            },
        ),
    }
}
#[cfg(windows)]
fn link_bench(port: &str, seconds: u64) -> Result<()> {
    use std::time::{Duration, Instant};
    ensure!(
        (1..=30).contains(&seconds),
        "benchmark seconds must be 1..30"
    );
    let mut radio = c5play::radio::Radio::open(port, 115200)?;
    let hello = radio.greet()?;
    println!("{hello}");
    ensure!(
        hello.contains("board=basic") && hello.contains("port=fx2"),
        "benchmark requires Basic/FX2 bridge firmware"
    );
    radio.command("stop")?;
    radio.send(&format!("bench {}", seconds * 1000))?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds + 5);
    let mut received = 0u64;
    let mut previous = None;
    let mut gaps = 0u64;
    let mut report = None;
    while Instant::now() < deadline && report.is_none() {
        for (kind, payload) in radio.poll()? {
            if kind == c5play::wire::PKT {
                ensure!(
                    payload.len() == 1524 && payload[22] == 0xff,
                    "invalid bench frame"
                );
                let sequence = u32::from_le_bytes(payload[16..20].try_into()?);
                if let Some(last) = previous {
                    ensure!(sequence > last, "bench sequence reversed");
                    gaps += u64::from(sequence - last - 1);
                } else {
                    gaps += u64::from(sequence);
                }
                previous = Some(sequence);
                ensure!(
                    payload[24..]
                        .iter()
                        .enumerate()
                        .all(|(i, b)| *b == (i as u32 + sequence) as u8),
                    "bench byte pattern mismatch"
                );
                received += 1;
            } else if kind == c5play::wire::TEXT {
                let text = String::from_utf8_lossy(&payload);
                if let Some(rest) = text.strip_prefix("ok bench sent=") {
                    let sent = rest
                        .split_whitespace()
                        .next()
                        .context("missing bench count")?
                        .parse::<u64>()?;
                    report = Some(sent);
                } else {
                    ensure!(!text.starts_with("err"), "{text}");
                }
            }
        }
    }
    let sent = report.context("bench completion timeout")?;
    println!(
        "bench sent={sent} received={received} gaps={gaps} framed_bytes_per_second={} {}",
        received * 1531 / seconds,
        radio.stats()
    );
    ensure!(received == sent && gaps == 0, "bench dropped frames");
    ensure!(
        radio.stats().contains("CRC不正=0 不正ヘッダ=0"),
        "bench framing corruption"
    );
    Ok(())
}
