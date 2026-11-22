use wasapi::*;

pub type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

pub fn com_init() {
    let _ = initialize_mta();
}

pub struct DevInfo {
    pub name: String,
    pub id: String,
    pub is_default: bool,
}

pub fn list(dir: Direction) -> Res<Vec<DevInfo>> {
    let en = DeviceEnumerator::new().map_err(e)?;
    let def = en.get_default_device(dir).ok().and_then(|d| d.get_id().ok());
    let col = en.get_device_collection(dir).map_err(e)?;
    let mut v = Vec::new();
    for i in 0..col.get_nbr_devices().map_err(e)? {
        let d = col.get_device_at_index(i).map_err(e)?;
        let id = d.get_id().map_err(e)?;
        v.push(DevInfo {
            name: d.get_friendlyname().unwrap_or_default(),
            is_default: def.as_deref() == Some(&id),
            id,
        });
    }
    Ok(v)
}

/// Busca por parte do nome (sem diferenciar maiúsculas); vazio = padrão.
pub fn find(dir: Direction, pattern: &str) -> Res<Device> {
    let en = DeviceEnumerator::new().map_err(e)?;
    if pattern.trim().is_empty() {
        return en.get_default_device(dir).map_err(e);
    }
    let p = pattern.to_lowercase();
    let col = en.get_device_collection(dir).map_err(e)?;
    for i in 0..col.get_nbr_devices().map_err(e)? {
        let d = col.get_device_at_index(i).map_err(e)?;
        if d.get_friendlyname().unwrap_or_default().to_lowercase().contains(&p) {
            return Ok(d);
        }
    }
    Err(format!("dispositivo não encontrado: '{pattern}' ({dir:?})"))
}

pub fn default_id(dir: Direction) -> Option<String> {
    DeviceEnumerator::new().ok()?.get_default_device(dir).ok()?.get_id().ok()
}

pub struct Mmcss(windows::Win32::Foundation::HANDLE);

pub fn mmcss_pro_audio() -> Option<Mmcss> {
    use windows::Win32::System::Threading::*;
    let mut idx = 0u32;
    unsafe {
        let h = AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut idx).ok()?;
        let _ = AvSetMmThreadPriority(h, AVRT_PRIORITY_HIGH);
        Some(Mmcss(h))
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

pub struct Capture {
    pub client: AudioClient,
    pub cap: AudioCaptureClient,
    pub ev: Handle,
    pub channels: usize,
    pub name: String,
    buf: Vec<f32>,
    /// Bytes por amostra inteira (2, 3 ou 4); 0 = float.
    int: usize,
    raw: Vec<u8>,
}

pub struct Render {
    pub client: AudioClient,
    pub rend: AudioRenderClient,
    pub ev: Handle,
    pub channels: usize,
    pub name: String,
    pub buffer_frames: u32,
    /// Período do motor de áudio para este stream, em frames.
    pub period: u32,
    buf: Vec<f32>,
    int: usize,
    raw: Vec<u8>,
}

fn int_to_f32(b: &[u8], int: usize) -> f32 {
    match int {
        2 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        3 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32) / 2147483648.0,
        _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2147483648.0,
    }
}

fn f32_to_int(x: f32, out: &mut [u8], int: usize) {
    let v = (x.clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32;
    let b = v.to_le_bytes();
    match int {
        2 => out.copy_from_slice(&b[2..4]),
        3 => out.copy_from_slice(&b[1..4]),
        _ => out.copy_from_slice(&b),
    }
}

fn fmt(ch: usize) -> WaveFormat {
    WaveFormat::new(32, 32, SampleType::Float, crate::SR, ch, None)
}

/// Captura (mic) ou loopback (dispositivo de reprodução).
pub fn open_capture(dev: &Device, loopback: bool, raw: bool, channels: usize) -> Res<Capture> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let mut try_raw = raw && !loopback;
    loop {
        let mut client = dev.get_iaudioclient().map_err(e)?;
        if try_raw {
            let p = AudioClientProperties::new().set_option(StreamOption::Raw);
            if client.set_properties(p).is_err() {
                try_raw = false;
                continue;
            }
        }
        let (_, min) = client.get_device_period().map_err(e)?;
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min.max(100_000) };
        match client.initialize_client(&fmt(channels), Direction::Capture, mode, None) {
            Ok(()) => {}
            Err(err) if try_raw => {
                crate::log!("RAW recusado em '{name}' ({err}), usando modo normal");
                try_raw = false;
                continue;
            }
            Err(err) => return Err(format!("init captura '{name}': {err}")),
        }
        let ev = client.set_get_eventhandle().map_err(e)?;
        let cap = client.get_audiocaptureclient().map_err(e)?;
        let frames = client.get_buffer_size().map_err(e)? as usize;
        crate::log!("captura aberta: '{name}' loopback={loopback} raw={try_raw} buffer={frames}");
        return Ok(Capture { client, cap, ev, channels, name, buf: vec![0.0; (frames + 4096) * channels], int: 0, raw: vec![] });
    }
}

impl Capture {
    /// Lê todos os pacotes; chama `f` com amostras mono. Retorna (frames, descontinuidade).
    pub fn drain(&mut self, mut f: impl FnMut(&[f32])) -> Res<(usize, bool)> {
        self.drain_ts(|s, _| f(s))
    }

    /// Igual a `drain`, com o timestamp QPC (100 ns) do primeiro frame de cada pacote.
    pub fn drain_ts(&mut self, mut f: impl FnMut(&[f32], u64)) -> Res<(usize, bool)> {
        let mut total = 0;
        let mut disc = false;
        loop {
            let n = match self.cap.get_next_packet_size().map_err(e)? {
                Some(0) | None => break,
                Some(n) => n as usize,
            };
            let need = n * self.channels;
            if self.buf.len() < need {
                return Err("pacote maior que o buffer".into());
            }
            let (got, info) = if self.int == 0 {
                let bytes = unsafe { std::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut u8, need * 4) };
                self.cap.read_from_device(bytes).map_err(e)?
            } else {
                let w = self.int;
                let r = self.cap.read_from_device(&mut self.raw[..need * w]).map_err(e)?;
                for (k, v) in self.buf[..r.0 as usize * self.channels].iter_mut().enumerate() {
                    *v = int_to_f32(&self.raw[k * w..k * w + w], w);
                }
                r
            };
            disc |= info.flags.data_discontinuity;
            let got = got as usize;
            if self.channels == 1 {
                f(&self.buf[..got], info.timestamp);
            } else {
                let ch = self.channels;
                for i in 0..got {
                    let s: f32 = self.buf[i * ch..(i + 1) * ch].iter().sum::<f32>() / ch as f32;
                    self.buf[i] = s;
                }
                f(&self.buf[..got], info.timestamp);
            }
            total += got;
        }
        Ok((total, disc))
    }
}

pub fn open_render(dev: &Device, channels: usize, buffer_hns: Option<i64>) -> Res<Render> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let mut client = dev.get_iaudioclient().map_err(e)?;
    let (_, min) = client.get_device_period().map_err(e)?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: buffer_hns.unwrap_or(min).max(min),
    };
    client
        .initialize_client(&fmt(channels), Direction::Render, mode, None)
        .map_err(|x| format!("init render '{name}': {x}"))?;
    let ev = client.set_get_eventhandle().map_err(e)?;
    let rend = client.get_audiorenderclient().map_err(e)?;
    let buffer_frames = client.get_buffer_size().map_err(e)?;
    let period = (client.get_device_period().map_err(e)?.0 * crate::SR as i64 / 10_000_000) as u32;
    crate::log!("render aberto: '{name}' buffer={buffer_frames} período={period}");
    Ok(Render {
        client,
        rend,
        ev,
        channels,
        name,
        buffer_frames,
        period,
        buf: vec![0.0; buffer_frames as usize * channels],
        int: 0,
        raw: vec![],
    })
}

/// Render com o menor período que o motor aceitar (IAudioClient3). Só no formato nativo de 48 kHz float.
pub fn open_render_fast(dev: &Device) -> Res<Render> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let mut client = dev.get_iaudioclient().map_err(e)?;
    let f = client.get_mixformat().map_err(e)?;
    if f.get_samplespersec() != crate::SR as u32 || f.get_bitspersample() != 32 || f.get_subformat().map_err(e)? != SampleType::Float {
        return Err(format!("'{name}' não está em 48 kHz float"));
    }
    let p = client.get_shared_mode_engine_period(&f).map_err(e)?;
    if p.min >= p.default {
        return Err(format!("'{name}' não aceita período menor"));
    }
    let period = p.min.max(96).div_ceil(p.fundamental.max(1)) * p.fundamental.max(1);
    client.initialize_shared_audio_stream(&f, TimingMode::Events, period).map_err(e)?;
    let ev = client.set_get_eventhandle().map_err(e)?;
    let rend = client.get_audiorenderclient().map_err(e)?;
    let buffer_frames = client.get_buffer_size().map_err(e)?;
    let channels = f.get_nchannels() as usize;
    crate::log!("render rápido: '{name}' buffer={buffer_frames} período={period} ({:.2} ms)", period as f64 * 1000.0 / crate::SR as f64);
    Ok(Render { client, rend, ev, channels, name, buffer_frames, period, buf: vec![0.0; buffer_frames as usize * channels], int: 0, raw: vec![] })
}

/// Abre em modo exclusivo tentando float e depois inteiros. Retorna (cliente, bytes por amostra inteira, período em 100 ns).
fn init_exclusive(dev: &Device, dir: Direction, channels: usize) -> Res<(AudioClient, usize, i64)> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let mut last = String::new();
    for (store, valid, st, int) in [(32, 32, SampleType::Float, 0), (32, 32, SampleType::Int, 4), (32, 24, SampleType::Int, 4), (24, 24, SampleType::Int, 3), (16, 16, SampleType::Int, 2)] {
        let f = WaveFormat::new(store, valid, st, crate::SR, channels, None);
        let mut client = dev.get_iaudioclient().map_err(e)?;
        let (_, min) = client.get_device_period().map_err(e)?;
        let p = client.calculate_aligned_period_near(min, Some(128), &f).unwrap_or(min);
        match client.initialize_client(&f, dir, StreamMode::EventsExclusive { period_hns: p }, None) {
            Ok(()) => {
                crate::log!("exclusivo: '{name}' {store}/{valid} bits {st:?}, período {:.2} ms", p as f64 / 1e4);
                return Ok((client, int, p));
            }
            Err(x) => last = format!("{store}/{valid} {st:?}: {x}"),
        }
    }
    Err(format!("'{name}' recusou o modo exclusivo ({last})"))
}

/// Exclusivo, sem o mixer do Windows.
pub fn open_render_exclusive(dev: &Device, channels: usize) -> Res<Render> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let (client, int, _) = init_exclusive(dev, Direction::Render, channels)?;
    let ev = client.set_get_eventhandle().map_err(e)?;
    let rend = client.get_audiorenderclient().map_err(e)?;
    let buffer_frames = client.get_buffer_size().map_err(e)?;
    let n = buffer_frames as usize * channels;
    Ok(Render { client, rend, ev, channels, name, buffer_frames, period: buffer_frames, buf: vec![0.0; n], int, raw: vec![0; n * 4] })
}

pub fn open_capture_exclusive(dev: &Device, channels: usize) -> Res<Capture> {
    let name = dev.get_friendlyname().unwrap_or_default();
    let (client, int, _) = init_exclusive(dev, Direction::Capture, channels)?;
    let ev = client.set_get_eventhandle().map_err(e)?;
    let cap = client.get_audiocaptureclient().map_err(e)?;
    let frames = client.get_buffer_size().map_err(e)? as usize;
    let n = (frames + 4096) * channels;
    Ok(Capture { client, cap, ev, channels, name, buf: vec![0.0; n], int, raw: vec![0; n * 4] })
}

impl Render {
    pub fn available(&self) -> Res<usize> {
        self.client.get_available_space_in_frames().map(|x| x as usize).map_err(e)
    }

    /// Escreve `n` frames gerados por `f` (mono -> todos os canais).
    pub fn write_mono(&mut self, mono: &[f32]) -> Res<()> {
        let n = mono.len();
        let ch = self.channels;
        for (i, &s) in mono.iter().enumerate() {
            for c in 0..ch {
                self.buf[i * ch + c] = s;
            }
        }
        self.flush(n)
    }

    fn flush(&mut self, n: usize) -> Res<()> {
        let ch = self.channels;
        if self.int == 0 {
            let bytes = unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const u8, n * ch * 4) };
            return self.rend.write_to_device(bytes, None).map_err(e);
        }
        let w = self.int;
        for k in 0..n * ch {
            f32_to_int(self.buf[k], &mut self.raw[k * w..k * w + w], w);
        }
        self.rend.write_to_device(&self.raw[..n * ch * w], None).map_err(e)
    }

    pub fn write_silence(&mut self, n: usize) -> Res<()> {
        let ch = self.channels;
        self.buf[..n * ch].fill(0.0);
        self.flush(n)
    }

    pub fn latency_hns(&self) -> i64 {
        self.client.get_stream_latency().unwrap_or(0)
    }
}

/// Conta eventos de dispositivo (entrou, saiu, mudou estado ou padrão).
pub static HOTPLUG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn hotplug() -> u64 {
    HOTPLUG.load(std::sync::atomic::Ordering::Relaxed)
}

mod notify {
    use std::sync::atomic::Ordering::Relaxed;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::*;
    use windows::core::{PCWSTR, Result, implement};

    #[implement(IMMNotificationClient)]
    pub struct Client;

    fn bump() -> Result<()> {
        super::HOTPLUG.fetch_add(1, Relaxed);
        Ok(())
    }

    impl IMMNotificationClient_Impl for Client_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
            bump()
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
            bump()
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
            bump()
        }
        fn OnDefaultDeviceChanged(&self, _: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
            if role == eConsole { bump() } else { Ok(()) }
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }
}

/// Registra o aviso de dispositivos do Windows e mantém até `stop`.
pub fn watch_devices(stop: &std::sync::atomic::AtomicBool) {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;
    com_init();
    let r = unsafe {
        CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL).and_then(|en| {
            let c: IMMNotificationClient = notify::Client.into();
            en.RegisterEndpointNotificationCallback(&c).map(|_| (en, c))
        })
    };
    match r {
        Ok((en, c)) => {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            unsafe {
                let _ = en.UnregisterEndpointNotificationCallback(&c);
            }
        }
        Err(err) => crate::log!("aviso de dispositivos indisponível: {err}"),
    }
}
