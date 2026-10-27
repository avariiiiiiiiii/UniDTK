use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;

const RING_SAMPLES: usize = 4096;
const FFT_SIZE: usize = 2048;
/// ~30 Hz analysis — enough for beat reaction without burning CPU.
const TICK: Duration = Duration::from_millis(33);

#[derive(Clone, Serialize)]
struct AudioBands {
    bass: f32,
    mid: f32,
    treble: f32,
}

pub struct AudioState {
    stop: Arc<AtomicBool>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl AudioState {
    /// Только выставить флаг — без join'ов. Используется на пути завершения
    /// процесса: выход не должен ждать поток захвата.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.request_stop();
        if let Ok(mut threads) = self.threads.lock() {
            for thread in threads.drain(..) {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for AudioState {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut threads) = self.threads.lock() {
            for thread in threads.drain(..) {
                let _ = thread.join();
            }
        }
    }
}

pub fn start() -> AudioState {
    log_default_output_device();

    let stop = Arc::new(AtomicBool::new(false));
    let ring = Arc::new(Mutex::new(RingBuffer::new(RING_SAMPLES)));
    let sample_rate = Arc::new(AtomicU32::new(48_000));
    let mut threads = Vec::new();

    if let Ok(thread) = thread::Builder::new()
        .name("audio-loopback".to_string())
        .spawn({
            let stop = stop.clone();
            let ring = ring.clone();
            let sample_rate = sample_rate.clone();
            move || {
                if let Err(error) = run_capture_loop(ring, sample_rate, stop.clone()) {
                    unidtk_api::log(format!("audio: capture stopped: {error}"));
                }
            }
        })
    {
        threads.push(thread);
    }

    if let Ok(thread) = thread::Builder::new().name("audio-fft".to_string()).spawn({
        let stop = stop.clone();
        move || run_fft_loop(ring, sample_rate, stop)
    }) {
        threads.push(thread);
    }

    AudioState {
        stop,
        threads: Mutex::new(threads),
    }
}

fn log_default_output_device() {
    let host = cpal::default_host();
    if let Some(device) = host.default_output_device() {
        if let Ok(description) = device.description() {
            unidtk_api::log(format!(
                "audio: default output via cpal: {}",
                description.name()
            ));
        }
    }
}

struct RingBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push_silent(&mut self, count: usize) {
        for _ in 0..count {
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(0.0);
        }
    }

    fn latest_into(&self, out: &mut [f32]) {
        let len = out.len();
        let count = self.samples.len().min(len);
        let src_start = self.samples.len().saturating_sub(count);
        let dst_start = len - count;
        out[..dst_start].fill(0.0);
        for (dst, src) in out[dst_start..]
            .iter_mut()
            .zip(self.samples.iter().skip(src_start))
        {
            *dst = *src;
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
    }
}

fn run_fft_loop(
    ring: Arc<Mutex<RingBuffer>>,
    sample_rate: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
) {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect();
    let mut buffer = vec![Complex { re: 0.0, im: 0.0 }; FFT_SIZE];
    let mut samples = vec![0.0f32; FFT_SIZE];
    // Peak-hold AGC: decays fast enough that quieter bars after a drop still move,
    // but holds peaks long enough that normalized levels stay readable.
    let mut peak = [1e-5f32; 3];
    let mut floor = [1e-6f32; 3];
    let mut prev = [0.0f32; 3];
    let mut smooth = [0.0f32; 3];

    let mut was_active = false;

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(TICK);

        if let Ok(ring_guard) = ring.lock() {
            ring_guard.latest_into(&mut samples);
        } else {
            samples.fill(0.0);
        }

        // Silence gate: skip FFT work and decay visuals when capture is quiet.
        let mut peak_sample = 0.0f32;
        for &s in &samples {
            peak_sample = peak_sample.max(s.abs());
        }
        if peak_sample < 1e-4 {
            let mut is_decaying = false;
            for i in 0..3 {
                if smooth[i] > 1e-3 {
                    smooth[i] *= 0.82;
                    prev[i] *= 0.9;
                    is_decaying = true;
                } else {
                    smooth[i] = 0.0;
                    prev[i] = 0.0;
                }
            }
            if is_decaying || was_active {
                was_active = is_decaying;
                unidtk_api::emit(
                    "audio-bands",
                    serde_json::to_value(AudioBands {
                        bass: smooth[0],
                        mid: smooth[1],
                        treble: smooth[2],
                    })
                    .unwrap_or(serde_json::Value::Null),
                );
            }
            continue;
        }

        was_active = true;

        for i in 0..FFT_SIZE {
            buffer[i].re = samples[i] * hann[i];
            buffer[i].im = 0.0;
        }

        fft.process(&mut buffer);
        let sr = sample_rate.load(Ordering::Relaxed).max(1) as f32;
        // Slightly tighter bands so bass / mid / treble arcs feel independent.
        let raw = [
            band_energy(&buffer, sr, 30.0, 160.0),
            band_energy(&buffer, sr, 160.0, 1800.0),
            band_energy(&buffer, sr, 1800.0, 10000.0),
        ];

        for i in 0..3 {
            // Spectral flux: rising edges (kicks, snares, hats) punch harder.
            let flux = (raw[i] - prev[i]).max(0.0);
            prev[i] = raw[i];
            let energy = raw[i] + flux * 1.35;

            // Noise floor EMA + peak hold with ~0.5 s half-life decay.
            floor[i] = floor[i] * 0.97 + energy * 0.03;
            peak[i] = (peak[i] * 0.972).max(energy).max(floor[i] * 2.5).max(1e-5);

            // Map relative to (peak - floor): ignores steady hiss, keeps beats.
            let span = (peak[i] - floor[i]).max(1e-5);
            let mut normalized = ((energy - floor[i]) / span).clamp(0.0, 1.0);
            // Mild gamma: lifts medium hits without crushing peaks.
            normalized = normalized.powf(0.62);

            // Fast attack / medium release so glyphs flash on the beat.
            let coef = if normalized > smooth[i] { 0.62 } else { 0.22 };
            smooth[i] += (normalized - smooth[i]) * coef;
        }

        unidtk_api::emit(
            "audio-bands",
            serde_json::to_value(AudioBands {
                bass: smooth[0],
                mid: smooth[1],
                treble: smooth[2],
            })
            .unwrap_or(serde_json::Value::Null),
        );
    }
}

fn band_energy(buffer: &[Complex<f32>], sample_rate: f32, low: f32, high: f32) -> f32 {
    let hz_per_bin = sample_rate / FFT_SIZE as f32;
    let start = ((low / hz_per_bin).ceil() as usize).max(1);
    let end = ((high / hz_per_bin).floor() as usize).min(FFT_SIZE / 2);
    if start > end {
        return 0.0;
    }

    // Peak-weighted RMS: mean power alone washes out kicks in wide mid/treble bands.
    let mut sum = 0.0f32;
    let mut peak = 0.0f32;
    let mut count = 0usize;
    for bin in start..=end {
        let mag2 = buffer[bin].norm_sqr();
        sum += mag2;
        peak = peak.max(mag2);
        count += 1;
    }
    let rms = (sum / count as f32).sqrt();
    let peak_m = peak.sqrt();
    rms * 0.55 + peak_m * 0.45
}

#[cfg(target_os = "windows")]
fn run_capture_loop(
    ring: Arc<Mutex<RingBuffer>>,
    sample_rate: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    // Device unplug / default-output switch invalidates the WASAPI client.
    // Retry instead of killing capture for the rest of the session, and clear
    // the ring so the FFT does not keep re-analyzing a frozen last buffer.
    while !stop.load(Ordering::Relaxed) {
        match windows_loopback::run(ring.clone(), sample_rate.clone(), stop.clone()) {
            Ok(()) => return Ok(()),
            Err(error) => {
                unidtk_api::log(format!("audio: capture error, retrying: {error}"));
                if let Ok(mut ring) = ring.lock() {
                    ring.clear();
                }
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn run_capture_loop(
    _ring: Arc<Mutex<RingBuffer>>,
    _sample_rate: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod windows_loopback {
    use super::{RingBuffer, TICK};
    use std::{
        ffi::c_void,
        ptr::{addr_of, null_mut},
        slice,
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            Arc, Mutex,
        },
        thread,
    };

    use windows::{
        core::GUID,
        Win32::{
            Foundation::RPC_E_CHANGED_MODE,
            Media::{
                Audio::{
                    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
                    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVE_FORMAT_PCM,
                },
                KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM,
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
                COINIT_MULTITHREADED,
            },
        },
    };

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
    const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
        GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

    #[derive(Clone, Copy)]
    enum SampleKind {
        Float,
        Pcm,
    }

    #[derive(Clone, Copy)]
    struct CaptureFormat {
        channels: usize,
        sample_rate: u32,
        block_align: usize,
        bits_per_sample: u16,
        kind: SampleKind,
    }

    pub fn run(
        ring: Arc<Mutex<RingBuffer>>,
        sample_rate: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let initialized = hr.is_ok();
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("CoInitializeEx failed: {hr:?}"));
        }

        let result = unsafe { capture_inner(ring, sample_rate, stop) };
        if initialized {
            unsafe { CoUninitialize() };
        }
        result
    }

    unsafe fn capture_inner(
        ring: Arc<Mutex<RingBuffer>>,
        sample_rate: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("GetDefaultAudioEndpoint: {e}"))?;

        let initial_id = {
            let id_pwstr = device.GetId().map_err(|e| format!("GetId: {e}"))?;
            let id_str = id_pwstr.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id_pwstr.as_ptr() as *const c_void));
            id_str
        };

        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioClient: {e}"))?;
        let mix_format = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat: {e}"))?;

        let format = match read_format(mix_format) {
            Ok(format) => format,
            Err(error) => {
                CoTaskMemFree(Some(mix_format as *const c_void));
                return Err(error);
            }
        };
        sample_rate.store(format.sample_rate, Ordering::Relaxed);

        let init_result = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            mix_format,
            None,
        );
        CoTaskMemFree(Some(mix_format as *const c_void));
        init_result.map_err(|e| format!("IAudioClient::Initialize: {e}"))?;

        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("GetService IAudioCaptureClient: {e}"))?;
        client
            .Start()
            .map_err(|e| format!("IAudioClient::Start: {e}"))?;

        let mut poll_count: u32 = 0;
        while !stop.load(Ordering::Relaxed) {
            drain_capture_packets(&capture, format, &ring)?;
            thread::sleep(TICK / 5);

            poll_count = poll_count.wrapping_add(1);
            if poll_count % 150 == 0 {
                // Periodic probe for default audio endpoint changes (~every 1s)
                if let Ok(current_device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                    if let Ok(current_id_pwstr) = current_device.GetId() {
                        let current_id_str = current_id_pwstr.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(current_id_pwstr.as_ptr() as *const c_void));
                        if !current_id_str.is_empty() && current_id_str != initial_id {
                            unidtk_api::log(format!(
                                "audio: default endpoint changed to '{current_id_str}', reconnecting"
                            ));
                            let _ = client.Stop();
                            return Err("default audio endpoint changed".to_string());
                        }
                    }
                }
            }
        }

        let _ = client.Stop();
        Ok(())
    }

    unsafe fn drain_capture_packets(
        capture: &IAudioCaptureClient,
        format: CaptureFormat,
        ring: &Arc<Mutex<RingBuffer>>,
    ) -> Result<(), String> {
        let mut packet_frames = capture
            .GetNextPacketSize()
            .map_err(|e| format!("GetNextPacketSize: {e}"))?;

        while packet_frames > 0 {
            let mut data = null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .map_err(|e| format!("GetBuffer: {e}"))?;

            if let Ok(mut ring_guard) = ring.lock() {
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    ring_guard.push_silent(frames as usize);
                } else {
                    decode_into(data, frames as usize, format, &mut ring_guard);
                }
            }

            capture
                .ReleaseBuffer(frames)
                .map_err(|e| format!("ReleaseBuffer: {e}"))?;
            packet_frames = capture
                .GetNextPacketSize()
                .map_err(|e| format!("GetNextPacketSize: {e}"))?;
        }

        Ok(())
    }

    unsafe fn read_format(format: *const WAVEFORMATEX) -> Result<CaptureFormat, String> {
        let tag = addr_of!((*format).wFormatTag).read_unaligned();
        let channels = addr_of!((*format).nChannels).read_unaligned();
        let sample_rate = addr_of!((*format).nSamplesPerSec).read_unaligned();
        let block_align = addr_of!((*format).nBlockAlign).read_unaligned();
        let bits_per_sample = addr_of!((*format).wBitsPerSample).read_unaligned();

        let kind = match tag {
            WAVE_FORMAT_IEEE_FLOAT => SampleKind::Float,
            tag if tag == WAVE_FORMAT_PCM as u16 => SampleKind::Pcm,
            WAVE_FORMAT_EXTENSIBLE => {
                let sub_format = read_extensible_sub_format(format);
                if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                    SampleKind::Float
                } else if sub_format == KSDATAFORMAT_SUBTYPE_PCM {
                    SampleKind::Pcm
                } else {
                    return Err(format!("unsupported WASAPI subformat: {sub_format:?}"));
                }
            }
            _ => return Err(format!("unsupported WASAPI format tag: {tag}")),
        };

        if channels == 0 || block_align == 0 || sample_rate == 0 {
            return Err("invalid WASAPI mix format".to_string());
        }

        Ok(CaptureFormat {
            channels: channels as usize,
            sample_rate,
            block_align: block_align as usize,
            bits_per_sample,
            kind,
        })
    }

    unsafe fn read_extensible_sub_format(format: *const WAVEFORMATEX) -> GUID {
        let base = format as *const u8;
        let sub_format = base.add(24) as *const GUID;
        sub_format.read_unaligned()
    }

    unsafe fn decode_into(data: *const u8, frames: usize, format: CaptureFormat, ring: &mut RingBuffer) {
        let bytes_len = frames.saturating_mul(format.block_align);
        let bytes = slice::from_raw_parts(data, bytes_len);
        let bytes_per_sample = format.block_align / format.channels;
        if bytes_per_sample == 0 {
            ring.push_silent(frames);
            return;
        }

        // For multi-channel setups (5.1/7.1), mix only the front L/R channels (min 2)
        // so stereo audio does not get attenuated by dividing by 6 or 8 channels.
        let channels_to_mix = format.channels.min(2);
        for frame in 0..frames {
            let mut mono = 0.0;
            for channel in 0..channels_to_mix {
                let offset = frame * format.block_align + channel * bytes_per_sample;
                mono += decode_sample(&bytes[offset..], format);
            }
            let sample = (mono / channels_to_mix as f32).clamp(-1.0, 1.0);
            if ring.samples.len() == ring.capacity {
                ring.samples.pop_front();
            }
            ring.samples.push_back(sample);
        }
    }

    fn decode_sample(bytes: &[u8], format: CaptureFormat) -> f32 {
        match (format.kind, format.bits_per_sample) {
            (SampleKind::Float, 32) if bytes.len() >= 4 => {
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            }
            (SampleKind::Float, 64) if bytes.len() >= 8 => f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]) as f32,
            (SampleKind::Pcm, 8) if !bytes.is_empty() => (bytes[0] as f32 - 128.0) / 128.0,
            (SampleKind::Pcm, 16) if bytes.len() >= 2 => {
                i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / i16::MAX as f32
            }
            (SampleKind::Pcm, 24) if bytes.len() >= 3 => {
                let raw = bytes[0] as i32 | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
                let signed = if raw & 0x800000 != 0 {
                    raw | !0x00ff_ffff
                } else {
                    raw
                };
                signed as f32 / 8_388_608.0
            }
            (SampleKind::Pcm, 32) if bytes.len() >= 4 => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / i32::MAX as f32
            }
            _ => 0.0,
        }
    }
}
