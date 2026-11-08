//! Direct H.264 MFT decoder; input access units are never wrapped in a file/container.
use anyhow::{Result, ensure};
use std::mem::ManuallyDrop;
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::*},
    core::Interface,
};
pub struct Decoder {
    mft: IMFTransform,
    pub width: u32,
    pub height: u32,
}
impl Decoder {
    pub fn new(manager: &IMFDXGIDeviceManager, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let mft: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            let attrs = mft.GetAttributes()?;
            let codec: ICodecAPI = mft.cast()?;
            let low_latency = windows::Win32::System::Variant::VARIANT::from(1u32);
            codec.SetValue(&CODECAPI_AVLowLatencyMode, &low_latency)?;
            ensure!(
                attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0,
                "H264 decoder does not support D3D11"
            );
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
            let mt = MFCreateMediaType()?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            mt.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
            mt.SetUINT64(&MF_MT_FRAME_RATE, (60u64 << 32) | 1)?;
            mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &mt, 0)?;
            let mut d = Self { mft, width, height };
            d.output_type()?;
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(d)
        }
    }
    fn output_type(&mut self) -> Result<()> {
        unsafe {
            for i in 0..128 {
                let mt = self.mft.GetOutputAvailableType(0, i)?;
                if mt.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &mt, 0)?;
                    let size = mt.GetUINT64(&MF_MT_FRAME_SIZE)?;
                    self.width = (size >> 32) as u32;
                    self.height = size as u32;
                    return Ok(());
                }
            }
            anyhow::bail!("NV12 decoder output unavailable")
        }
    }
    pub fn push(
        &mut self,
        bytes: &[u8],
        sequence: u64,
        pts_ns: Option<u64>,
    ) -> Result<Vec<IMFSample>> {
        unsafe {
            ensure!(bytes.len() <= 4 * 1024 * 1024, "H264 input exceeds limit");
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut dest = std::ptr::null_mut();
            buffer.Lock(&mut dest, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(
                pts_ns
                    .map(|v| v / 100)
                    .unwrap_or(sequence.saturating_mul(10_000_000) / 60)
                    .min(i64::MAX as u64) as i64,
            )?;
            sample.SetSampleDuration(10_000_000 / 60)?;
            let mut out = Vec::new();
            match self.mft.ProcessInput(0, &sample, 0) {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    out.extend(self.drain()?);
                    self.mft.ProcessInput(0, &sample, 0)?;
                }
                Err(e) => return Err(e.into()),
            }
            out.extend(self.drain()?);
            Ok(out)
        }
    }
    fn drain(&mut self) -> Result<Vec<IMFSample>> {
        unsafe {
            let mut samples = Vec::new();
            for _ in 0..32 {
                let info = self.mft.GetOutputStreamInfo(0)?;
                ensure!(
                    info.dwFlags
                        & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                            | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                        != 0,
                    "decoder requires CPU-backed output samples"
                );
                let mut out = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(None),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0;
                let result = self
                    .mft
                    .ProcessOutput(0, std::slice::from_mut(&mut out), &mut status);
                let sample = ManuallyDrop::take(&mut out.pSample);
                drop(ManuallyDrop::take(&mut out.pEvents));
                match result {
                    Ok(()) => {
                        if let Some(sample) = sample {
                            samples.push(sample);
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(samples),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.output_type()?,
                    Err(e) => return Err(e.into()),
                }
            }
            anyhow::bail!("decoder did not drain within limit")
        }
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}
