//! GameShare Opus -> Windows Media Foundation PCM -> bounded WASAPI shared output.
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    mem::ManuallyDrop,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};
use windows::Win32::{
    Foundation::HANDLE,
    Media::{Audio::*, MediaFoundation::*},
    System::{Com::*, Threading::*},
};

// Only this renderer thread joins MMCSS. The registration is restored on exit.
struct Priority(HANDLE);
impl Priority {
    fn new() -> Result<Self> {
        unsafe {
            let mut index = 0;
            let task = Self(AvSetMmThreadCharacteristicsW(
                windows::core::w!("Audio"),
                &mut index,
            )?);
            AvSetMmThreadPriority(task.0, AVRT_PRIORITY_HIGH)?;
            Ok(task)
        }
    }
}
impl Drop for Priority {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}
struct TimerResolution;
impl TimerResolution {
    fn new() -> Option<Self> {
        (unsafe { windows::Win32::Media::timeBeginPeriod(1) } == 0).then_some(Self)
    }
}
impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }
}

pub fn opus_payload(bytes: &[u8]) -> Result<&[u8]> {
    ensure!(bytes.len() >= 9, "truncated GameShare audio");
    let len = u32::from_be_bytes(bytes[..4].try_into()?) as usize;
    ensure!(
        len == bytes.len() - 8 && len <= 1275,
        "invalid GameShare Opus size"
    );
    Ok(&bytes[8..])
}
struct Runtime;
impl Runtime {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(error.into());
            }
        }
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}
struct Decoder {
    transform: IMFTransform,
    float_output: bool,
}
impl Decoder {
    fn new() -> Result<Self> {
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_MSOpusDecoder, None, CLSCTX_INPROC_SERVER)
                    .context("Windows Opus decoder unavailable")?;
            let mt = MFCreateMediaType()?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Opus)?;
            mt.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            mt.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)?;
            transform
                .SetInputType(0, &mt, 0)
                .context("Opus input type")?;
            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            for (key, value) in [
                (MF_MT_AUDIO_NUM_CHANNELS, 2),
                (MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000),
                (MF_MT_AUDIO_BITS_PER_SAMPLE, 16),
                (MF_MT_AUDIO_BLOCK_ALIGNMENT, 4),
                (MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192000),
                (MF_MT_AUDIO_CHANNEL_MASK, 3),
            ] {
                pcm.SetUINT32(&key, value)?;
            }
            let float_output = if transform.SetOutputType(0, &pcm, 0).is_ok() {
                false
            } else {
                pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 8)?;
                pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 384000)?;
                transform
                    .SetOutputType(0, &pcm, 0)
                    .context("Opus float output type")?;
                true
            };
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(Self {
                transform,
                float_output,
            })
        }
    }
    fn decode(&mut self, bytes: &[u8], sequence: u64) -> Result<Vec<u8>> {
        let opus = opus_payload(bytes)?;
        unsafe {
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(opus.len() as u32)?;
            let mut dest = std::ptr::null_mut();
            buffer.Lock(&mut dest, None, None)?;
            std::ptr::copy_nonoverlapping(opus.as_ptr(), dest, opus.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(opus.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(sequence.saturating_mul(100_000).min(i64::MAX as u64) as i64)?;
            self.transform.ProcessInput(0, &sample, 0)?;
            let mut pcm = Vec::new();
            for _ in 0..16 {
                let info = self.transform.GetOutputStreamInfo(0)?;
                let supplied = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                let output = if supplied {
                    None
                } else {
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(info.cbSize.max(23040))?)?;
                    Some(s)
                };
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(output),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0;
                let result =
                    self.transform
                        .ProcessOutput(0, std::slice::from_mut(&mut data), &mut status);
                let sample = ManuallyDrop::take(&mut data.pSample);
                drop(ManuallyDrop::take(&mut data.pEvents));
                match result {
                    Ok(()) => {
                        if let Some(sample) = sample {
                            let buffer = sample.ConvertToContiguousBuffer()?;
                            let mut src = std::ptr::null_mut();
                            let mut len = 0;
                            buffer.Lock(&mut src, None, Some(&mut len))?;
                            pcm.extend_from_slice(std::slice::from_raw_parts(src, len as usize));
                            buffer.Unlock()?;
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        if self.float_output {
                            ensure!(pcm.len().is_multiple_of(8), "unaligned stereo float PCM");
                            let converted = pcm
                                .chunks_exact(4)
                                .flat_map(|v| {
                                    let f = f32::from_le_bytes(v.try_into().unwrap());
                                    let v = if f.is_finite() {
                                        (f.clamp(-1.0, 1.0) * 32767.0).round() as i16
                                    } else {
                                        0
                                    };
                                    v.to_le_bytes()
                                })
                                .collect();
                            return Ok(converted);
                        }
                        return Ok(pcm);
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            anyhow::bail!("Opus decoder output exceeded limit")
        }
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}
struct Output {
    client: IAudioClient,
    render: IAudioRenderClient,
    frames: u32,
    started: bool,
    underruns: u64,
    prefill_frames: usize,
}
impl Output {
    fn new() -> Result<Self> {
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_INPROC_SERVER, None)?;
            let format = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: 2,
                nSamplesPerSec: 48000,
                nAvgBytesPerSec: 192000,
                nBlockAlign: 4,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                300_000,
                0,
                &format,
                None,
            )?;
            let frames = client.GetBufferSize()?;
            let render = client.GetService()?;
            println!("WASAPI 48kHz stereo PCM16 buffer={} frames", frames);
            Ok(Self {
                client,
                render,
                frames,
                started: false,
                underruns: 0,
                prefill_frames: 2880,
            })
        }
    }
    fn pump(&mut self, pending: &mut VecDeque<u8>) -> Result<bool> {
        unsafe {
            let padding = self.client.GetCurrentPadding()?;
            if self.started && padding == 0 {
                self.underruns += 1;
                // Rebuild a short cushion after a transport stall instead of
                // repeatedly starting and stopping with each late 10 ms packet.
                self.client.Stop()?;
                self.client.Reset()?;
                self.started = false;
                self.prefill_frames = (self.prefill_frames + 480).min(3840);
            }
            if !self.started && pending.len() / 4 < self.prefill_frames {
                return Ok(false);
            }
            let available = self
                .frames
                .saturating_sub(padding)
                .min(1440u32.saturating_sub(padding));
            let frames = available.min((pending.len() / 4) as u32);
            if frames == 0 {
                return Ok(false);
            }
            let dest = self.render.GetBuffer(frames)?;
            for i in 0..frames as usize * 4 {
                *dest.add(i) = pending.pop_front().unwrap();
            }
            self.render.ReleaseBuffer(frames, 0)?;
            if !self.started {
                self.client.Start()?;
                self.started = true;
                return Ok(true);
            }
            Ok(false)
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
pub struct Playback {
    pub sender: SyncSender<crate::lsp::MediaUnit>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Playback {
    pub fn start(timeline: Arc<Mutex<crate::timeline::Timeline>>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<crate::lsp::MediaUnit>(8);
        let (ready, started) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let _priority = Priority::new().ok();
            let _timer = TimerResolution::new();
            let runtime = Runtime::new();
            if let Err(error) = &runtime {
                let _ = ready.send(Err(format!("{error:#}")));
                return;
            }
            let _runtime = runtime.unwrap();
            let initialized = (|| Ok::<_, anyhow::Error>((Decoder::new()?, Output::new()?)))();
            let (mut decoder, mut output) = match initialized {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready.send(Err(format!("{e:#}")));
                    return;
                }
            };
            let _ = ready.send(Ok(()));
            let mut pending = VecDeque::new();
            let mut decoded = 0u64;
            let mut dropped = 0u64;
            let mut last = None;
            let mut first_pts = None;
            let mut latest_end_pts = None;
            let mut max_pending = 0;
            let mut concealed = 0;
            while !stopping.load(Ordering::Relaxed) {
                match receiver.recv_timeout(Duration::from_millis(2)) {
                    Ok(unit) => {
                        if let Some(previous) = last
                            && unit.sequence <= previous
                        {
                            continue;
                        }
                        if first_pts.is_none() {
                            first_pts = unit.timestamps.map(|(pts, _)| pts >> 8);
                        }
                        if let Some(previous) = last {
                            let gap = unit.sequence.saturating_sub(previous + 1).min(6);
                            concealed += gap;
                            pending.extend(std::iter::repeat_n(0, gap as usize * 1920));
                        }
                        last = Some(unit.sequence);
                        match decoder.decode(&unit.payload, unit.sequence) {
                            Ok(pcm) => {
                                latest_end_pts = unit.timestamps.map(|(pts, _)| {
                                    (pts >> 8) + (pcm.len() as u64 / 4) * 1_000_000_000 / 48000
                                });
                                pending.extend(pcm);
                                decoded += 1;
                            }
                            Err(e) => {
                                if dropped < 3 {
                                    eprintln!("Opus packet rejected: {e:#}");
                                }
                                dropped += 1;
                            }
                        }
                        // Bounded PCM queue; compressed queue is independently bounded.
                        max_pending = max_pending.max(pending.len() / 4);
                        while pending.len() > 23040 {
                            for _ in 0..1920 {
                                pending.pop_front();
                            }
                            dropped += 1;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                match output.pump(&mut pending) {
                    Ok(true) => {
                        if let Some(pts) = first_pts {
                            timeline
                                .lock()
                                .unwrap()
                                .start(pts, std::time::Instant::now());
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("Audio output stopped: {e:#}");
                        break;
                    }
                }
                if output.started
                    && let Some(end) = latest_end_pts
                {
                    let queued = unsafe { output.client.GetCurrentPadding().unwrap_or(0) } as u64
                        + pending.len() as u64 / 4;
                    if queued > 0 {
                        timeline.lock().unwrap().align(
                            end.saturating_sub(queued * 1_000_000_000 / 48000),
                            std::time::Instant::now(),
                        );
                    }
                }
            }
            println!(
                "音声 decoded={} dropped={} concealed={} underruns={} max_pending_frames={}",
                decoded, dropped, concealed, output.underruns, max_pending
            );
        });
        match started
            .recv()
            .context("audio initialization thread stopped")?
        {
            Ok(()) => Ok(Self {
                sender,
                stop,
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                anyhow::bail!("{error}");
            }
        }
    }
}
impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub fn probe(path: &std::path::Path) -> Result<()> {
    let _runtime = Runtime::new()?;
    let mut decoder = Decoder::new()?;
    if path.extension().is_some_and(|e| e == "pcap") {
        let mut reader = crate::pcap::Reader::open(path)?;
        ensure!(reader.linktype == 1, "audio probe requires Ethernet pcap");
        let mut lsp = crate::lsp::Reassembler::default();
        let mut count = 0u64;
        let mut frames = 0u64;
        let mut sizes = std::collections::BTreeMap::<usize, u64>::new();
        let mut arrivals = Vec::new();
        while let Some((time, frame)) = reader.next_timed_packet()? {
            let Some(payload) = crate::diagnose::udp_payload(&frame) else {
                continue;
            };
            if !payload.starts_with(&[0xec, 0xc2]) {
                continue;
            }
            for unit in lsp.push(payload)? {
                if unit.stream == 1 {
                    arrivals.push(time);
                    let pcm = decoder.decode(&unit.payload, unit.sequence)?;
                    count += 1;
                    frames += (pcm.len() / 4) as u64;
                    *sizes.entry(pcm.len() / 4).or_default() += 1;
                }
            }
        }
        println!(
            "audio packets={} PCM frames={} seconds={:.3} frame_sizes={:?}",
            count,
            frames,
            frames as f64 / 48000.0,
            sizes
        );
        let mut intervals: Vec<_> = arrivals
            .windows(2)
            .map(|a| a[1].saturating_sub(a[0]).as_micros())
            .collect();
        intervals.sort_unstable();
        if !intervals.is_empty() {
            println!(
                "arrival interval us p50={} p95={} p99={} max={}",
                intervals[intervals.len() / 2],
                intervals[intervals.len() * 95 / 100],
                intervals[intervals.len() * 99 / 100],
                intervals.last().unwrap()
            );
        }
        return Ok(());
    }
    let pcm = decoder.decode(&std::fs::read(path)?, 1)?;
    ensure!(
        !pcm.is_empty() && pcm.len() % 4 == 0,
        "no stereo PCM decoded"
    );
    println!(
        "Opus -> PCM16 frames={} nonzero_bytes={}",
        pcm.len() / 4,
        pcm.iter().filter(|&&x| x != 0).count()
    );
    Ok(())
}
pub fn self_test(seconds: u64) -> Result<()> {
    ensure!((1..=30).contains(&seconds), "seconds must be 1..30");
    let audio = Playback::start(Arc::new(Mutex::new(crate::timeline::Timeline::default())))?;
    let start = std::time::Instant::now();
    for sequence in 1..=seconds * 100 {
        let due = start + Duration::from_millis(sequence * 10);
        thread::sleep(due.saturating_duration_since(std::time::Instant::now()));
        audio.sender.send(crate::lsp::MediaUnit {
            stream: 1,
            kind: 0,
            sequence,
            timestamps: Some((sequence * 10_000_000 * 256, 0)),
            payload: vec![0, 0, 0, 3, 0, 0, 0, 0, 0xf4, 0xff, 0xfe],
        })?;
    }
    thread::sleep(Duration::from_millis(100));
    drop(audio);
    Ok(())
}
pub fn replay(path: &std::path::Path, seconds: u64) -> Result<()> {
    ensure!((1..=300).contains(&seconds), "seconds must be 1..300");
    let mut reader = crate::pcap::Reader::open(path)?;
    ensure!(reader.linktype == 1, "audio replay requires Ethernet pcap");
    let mut lsp = crate::lsp::Reassembler::default();
    let audio = Playback::start(Arc::new(Mutex::new(crate::timeline::Timeline::default())))?;
    let mut anchor = None;
    let mut queue_dropped = 0;
    while let Some((time, frame)) = reader.next_timed_packet()? {
        let Some(payload) = crate::diagnose::udp_payload(&frame) else {
            continue;
        };
        if !payload.starts_with(&[0xec, 0xc2]) {
            continue;
        }
        for unit in lsp.push(payload)? {
            if unit.stream != 1 {
                continue;
            }
            let (first, start) = *anchor.get_or_insert((time, std::time::Instant::now()));
            let elapsed = time.saturating_sub(first);
            if elapsed >= Duration::from_secs(seconds) {
                println!("audio replay queue_dropped={queue_dropped}");
                return Ok(());
            }
            thread::sleep((start + elapsed).saturating_duration_since(std::time::Instant::now()));
            if audio.sender.try_send(unit).is_err() {
                queue_dropped += 1;
            }
        }
    }
    println!("audio replay queue_dropped={queue_dropped}");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_length_is_exact() {
        assert!(opus_payload(&[0; 8]).is_err());
        assert_eq!(
            opus_payload(&[0, 0, 0, 1, 0, 0, 0, 0, 0xf4]).unwrap(),
            [0xf4]
        );
        assert!(opus_payload(&[0, 0, 0, 2, 0, 0, 0, 0, 0xf4]).is_err());
    }
    #[test]
    fn windows_opus_decodes_standard_stereo_silence() {
        let _runtime = Runtime::new().unwrap();
        let mut decoder = Decoder::new().unwrap();
        let packet = [0, 0, 0, 3, 0, 0, 0, 0, 0xf4, 0xff, 0xfe];
        let pcm = decoder.decode(&packet, 1).unwrap();
        assert_eq!(pcm.len(), 480 * 2 * 2);
    }
}
