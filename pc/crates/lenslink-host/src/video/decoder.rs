//! Hardware video decoding with the Media Foundation decoders that ship with Windows.
//!
//! The Microsoft H.264 / H.265 decoder MFTs are synchronous and use DXVA when they are given a D3D11
//! device manager, so decoded frames stay on the GPU as NV12 textures. Low-latency mode makes the
//! decoder return every frame as soon as it is decoded instead of holding a reorder queue.

use std::mem::ManuallyDrop;

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use super::Codec;

/// One decoded frame: a slice of the decoder's texture array.
pub struct Frame {
    pub texture: ID3D11Texture2D,
    pub index: u32,
    /// Keeps the decoder from reusing the texture slice while we read it.
    _sample: IMFSample,
}

pub struct Decoder {
    mft: IMFTransform,
    _manager: IMFDXGIDeviceManager,
    codec: Codec,
    /// SPS/PPS(/VPS) from the encoder's CONFIG packet, prepended to every keyframe.
    config: Vec<u8>,
    provides_samples: bool,
}

impl Decoder {
    pub fn new(device: &ID3D11Device, codec: Codec, width: u32, height: u32, fps: u32) -> windows::core::Result<Self> {
        unsafe {
            let clsid = match codec {
                Codec::H264 => CLSID_MSH264DecoderMFT,
                Codec::H265 => CLSID_MSH265DecoderMFT,
            };
            let mft: IMFTransform = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)?;

            let attrs = mft.GetAttributes()?;
            attrs.SetUINT32(&CODECAPI_AVLowLatencyMode, 1)?;

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.expect("MFCreateDXGIDeviceManager returned no manager");
            manager.ResetDevice(device, token)?;
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(
                &MF_MT_SUBTYPE,
                match codec {
                    Codec::H264 => &MFVideoFormat_H264,
                    Codec::H265 => &MFVideoFormat_HEVC,
                },
            )?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &input, 0)?;

            let mut decoder = Decoder {
                mft,
                _manager: manager,
                codec,
                config: Vec::new(),
                provides_samples: false,
            };
            decoder.set_output_type()?;
            decoder.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            decoder.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(decoder)
        }
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// Picks NV12 output; called at start and again whenever the decoder reports a format change.
    fn set_output_type(&mut self) -> windows::core::Result<()> {
        unsafe {
            let mut i = 0;
            loop {
                let t = self.mft.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &t, 0)?;
                    break;
                }
                i += 1;
            }
            let info = self.mft.GetOutputStreamInfo(0)?;
            self.provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            Ok(())
        }
    }

    /// Feeds one encoded packet. Returns the frames that became ready (usually zero or one).
    pub fn decode(&mut self, data: &[u8], pts_us: u64, config: bool, keyframe: bool) -> windows::core::Result<Vec<Frame>> {
        if config {
            self.config = data.to_vec();
            return Ok(Vec::new());
        }
        let mut frames = Vec::new();
        unsafe {
            let extra = if keyframe { self.config.len() } else { 0 };
            let buffer = MFCreateMemoryBuffer((extra + data.len()) as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            if extra > 0 {
                std::ptr::copy_nonoverlapping(self.config.as_ptr(), ptr, extra);
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.add(extra), data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength((extra + data.len()) as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts_us as i64 * 10)?;
            if keyframe {
                sample.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
            }

            loop {
                match self.mft.ProcessInput(0, &sample, 0) {
                    Ok(()) => break,
                    // Output must be drained before the decoder takes more input.
                    Err(e) if e.code() == MF_E_NOTACCEPTING => self.drain(&mut frames)?,
                    Err(e) => return Err(e),
                }
            }
            self.drain(&mut frames)?;
        }
        Ok(frames)
    }

    fn drain(&mut self, frames: &mut Vec<Frame>) -> windows::core::Result<()> {
        unsafe {
            loop {
                let mut out = [MFT_OUTPUT_DATA_BUFFER::default()];
                if !self.provides_samples {
                    // Not expected with a D3D manager; kept so a software fallback still works.
                    let info = self.mft.GetOutputStreamInfo(0)?;
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(info.cbSize)?)?;
                    out[0].pSample = ManuallyDrop::new(Some(s));
                }
                let mut status = 0;
                let result = self.mft.ProcessOutput(0, &mut out, &mut status);
                let sample = ManuallyDrop::take(&mut out[0].pSample);
                drop(ManuallyDrop::take(&mut out[0].pEvents));
                match result {
                    Ok(()) => {
                        if let Some(sample) = sample {
                            if let Some(f) = frame_from_sample(sample)? {
                                frames.push(f);
                            }
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.set_output_type()?,
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

unsafe fn frame_from_sample(sample: IMFSample) -> windows::core::Result<Option<Frame>> {
    unsafe {
        let buffer = sample.GetBufferByIndex(0)?;
        let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() else {
            return Ok(None); // a system-memory frame: nothing to show without a GPU texture
        };
        let mut raw = std::ptr::null_mut();
        dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
        let texture = ID3D11Texture2D::from_raw(raw);
        let index = dxgi.GetSubresourceIndex()?;
        Ok(Some(Frame { texture, index, _sample: sample }))
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// Whether Windows has a decoder for this codec (H.265 needs the "HEVC Video Extensions" from the Store).
pub fn available(codec: Codec) -> bool {
    let subtype: GUID = match codec {
        Codec::H264 => MFVideoFormat_H264,
        Codec::H265 => MFVideoFormat_HEVC,
    };
    unsafe {
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
        let mut list = std::ptr::null_mut();
        let mut count = 0u32;
        let ok = MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            None,
            &mut list,
            &mut count,
        )
        .is_ok();
        if !list.is_null() {
            for i in 0..count as usize {
                drop((*list.add(i)).take());
            }
            windows::Win32::System::Com::CoTaskMemFree(Some(list as _));
        }
        ok && count > 0
    }
}
