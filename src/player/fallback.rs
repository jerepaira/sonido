//! Backend de audio 100% Rust (rodio + symphonia + cpal).
//!
//! Es el *fallback* cuando `libmpv` no está instalado. Reproduce MP3, FLAC,
//! AAC/MP4, OGG/Vorbis y WAV por HTTP(S), con seek vía Range requests y
//! reproducción encadenada (gapless: symphonia recorta padding del encoder).

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use rodio::source::Source;

use crate::player::{AudioInfo, Backend, PlayRequest, PlayerEvent, Slot};

// ─────────────────────────── lector HTTP con seek ───────────────────────────

/// `Read + Seek` sobre HTTP usando `Range`. Lo ya descargado queda en memoria
/// para que los seeks hacia atrás no cuesten un round-trip.
struct HttpReader {
    url: String,
    headers: Vec<(String, String)>,
    reader: Option<ureq::BodyReader<'static>>,
    buf: Vec<u8>,
    pos: usize,
    total: Option<u64>,
    range_ok: bool,
    mime: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_global(None)
        .no_delay(true)
        .max_idle_connections_per_host(2)
        .build()
        .new_agent()
}

impl HttpReader {
    fn open(url: &str, headers: &[(String, String)]) -> Result<Self> {
        let mut req = agent().get(url);
        req = req.header("Accept", "audio/*,*/*;q=0.8");
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .call()
            .map_err(|e| anyhow!("no se pudo abrir el audio: {e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(anyhow!("el audio respondió HTTP {status}"));
        }
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let mime = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or("").trim().to_string());
        let range_ok = resp
            .headers()
            .get("accept-ranges")
            .and_then(|v| v.to_str().ok())
            .map(|v| !v.eq_ignore_ascii_case("none"))
            .unwrap_or(false);
        let reader = resp.into_body().into_reader();
        Ok(Self {
            url: url.to_string(),
            headers: headers.to_vec(),
            reader: Some(reader),
            buf: Vec::with_capacity(1 << 20),
            pos: 0,
            total,
            range_ok,
            mime,
        })
    }

    fn reopen_at(&mut self, from: u64) -> Result<()> {
        if self.range_ok && from > 0 {
            let mut req = agent().get(&self.url);
            req = req.header("Range", format!("bytes={from}-"));
            for (k, v) in &self.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            if let Ok(resp) = req.call() {
                if resp.status().as_u16() == 206 {
                    if let Some(cl) = resp
                        .headers()
                        .get("content-length")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                    {
                        self.total = Some(from + cl);
                    }
                    let body = resp.into_body();
                    self.reader = Some(body.into_reader());
                    self.buf.truncate(from as usize);
                    self.pos = from as usize;
                    return Ok(());
                }
            }
        }
        // El servidor no soporta Range: reabrir y descartar bytes.
        let mut fresh = Self::open(&self.url, &self.headers)?;
        self.reader = fresh.reader.take();
        self.total = fresh.total;
        self.range_ok = fresh.range_ok;
        self.buf.clear();
        self.pos = 0;
        if from > 0 {
            let mut discard = vec![0u8; 64 * 1024];
            let mut left = from;
            while left > 0 {
                let n = std::cmp::min(left, discard.len() as u64) as usize;
                match self.fill(&mut discard[..n])? {
                    0 => break,
                    read => left -= read as u64,
                }
            }
        }
        Ok(())
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<usize> {
        let Some(r) = self.reader.as_mut() else {
            return Ok(0);
        };
        match r.read(out) {
            Ok(n) if n > 0 => {
                self.buf.extend_from_slice(&out[..n]);
                self.pos += n;
                Ok(n)
            }
            Ok(_) => {
                self.reader = None;
                Ok(0)
            }
            Err(e) => Err(anyhow!("leyendo audio: {e}")),
        }
    }
}

impl Read for HttpReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos < self.buf.len() {
            let n = std::cmp::min(out.len(), self.buf.len() - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.fill(out).map_err(std::io::Error::other)
    }
}

impl Seek for HttpReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target: i128 = match pos {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => {
                let len = self.total.unwrap_or(self.buf.len() as u64) as i128;
                len + d as i128
            }
        };
        let target = target.max(0) as u64;
        if (target as usize) <= self.buf.len() {
            self.pos = target as usize;
            return Ok(target);
        }
        self.reopen_at(target).map_err(std::io::Error::other)?;
        Ok(self.pos as u64)
    }
}

// ──────────────────────────────── backend ────────────────────────────────

struct Item {
    slot: Slot,
    duration: f64,
    started_at: Instant,
    paused_accum: Duration,
    paused_at: Option<Instant>,
}

impl Item {
    fn elapsed(&self) -> f64 {
        let paused = self.paused_accum + self.paused_at.map(|p| p.elapsed()).unwrap_or_default();
        self.started_at.elapsed().saturating_sub(paused).as_secs_f64()
    }
}

struct State {
    current: Option<Item>,
    queued: Option<Item>,
    position: f64,
    duration: f64,
    paused: bool,
    audio: Option<AudioInfo>,
}

pub struct RodioBackend {
    device_sink: Arc<rodio::stream::MixerDeviceSink>,
    player: Arc<rodio::Player>,
    state: Arc<Mutex<State>>,
    loading: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
    volume: AtomicU64,
    muted: AtomicBool,
    speed: AtomicU64,
    stop_flag: Arc<AtomicBool>,
    _monitor: Option<std::thread::JoinHandle<()>>,
}

impl RodioBackend {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>) -> Self {
        let device_sink = rodio::stream::DeviceSinkBuilder::from_default_device()
            .and_then(|b| b.open_sink_or_fallback())
            .map(Arc::new)
            .map_err(|e| anyhow!("sin dispositivo de audio: {e}"))
            .expect("no se pudo abrir la salida de audio (¿hay dispositivo ALSA/PipeWire?)");
        let mixer = device_sink.mixer().clone();
        let player = Arc::new(rodio::Player::connect_new(&mixer));

        let state = Arc::new(Mutex::new(State {
            current: None,
            queued: None,
            position: 0.0,
            duration: 0.0,
            paused: false,
            audio: None,
        }));
        let loading = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let monitor = std::thread::Builder::new()
            .name("audio-monitor".into())
            .spawn({
                let state = state.clone();
                let tx = tx.clone();
                let loading = loading.clone();
                let stop = stop_flag.clone();
                let player = player.clone();
                move || monitor_loop(state, tx, loading, stop, player)
            })
            .ok();

        Self {
            device_sink,
            player,
            state,
            loading,
            tx,
            volume: AtomicU64::new(1.0f64.to_bits()),
            muted: AtomicBool::new(false),
            speed: AtomicU64::new(1.0f64.to_bits()),
            stop_flag,
            _monitor: monitor,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn load(&self, req: PlayRequest, slot: Slot, replace: bool) -> Result<()> {
        if req.adaptive != crate::resolve::Adaptive::None {
            return Err(anyhow!(
                "el addon entrega {} y el backend de audio 100% Rust no reproduce                  streaming adaptativo. Instalá libmpv (`sudo apt install libmpv2` o `mpv`)                  y recompilá para habilitarlo.",
                req.adaptive.label()
            ));
        }
        if replace {
            self.player.stop();
            self.player.clear();
        }
        self.loading.store(true, Ordering::SeqCst);

        let player = self.player.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let loading = self.loading.clone();

        std::thread::Builder::new()
            .name("audio-load".into())
            .spawn(move || {
                let _guard = LoadingGuard(loading.clone());
                let reader = match HttpReader::open(&req.url, &req.headers) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(PlayerEvent::Failed {
                            slot,
                            error: e.to_string(),
                        });
                        return;
                    }
                };
                let mime = reader.mime.clone();
                let total_bytes = reader.total;
                let decoder = match rodio::Decoder::builder()
                    .with_data(reader)
                    .with_seekable(true)
                    .with_gapless(true)
                    .with_mime_type(mime.as_deref().unwrap_or("audio/mpeg"))
                    .build()
                {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(PlayerEvent::Failed {
                            slot,
                            error: format!("decodificador: {e}"),
                        });
                        return;
                    }
                };
                let info = AudioInfo {
                    codec: mime
                        .as_deref()
                        .and_then(|m| m.split('/').nth(1))
                        .unwrap_or("audio")
                        .to_string(),
                    sample_rate: decoder.sample_rate().get(),
                    channels: decoder.channels().get() as u32,
                    bit_depth: 0,
                    bitrate_kbps: match decoder.total_duration() {
                        Some(d) if d.as_secs_f64() > 0.0 && total_bytes.unwrap_or(0) > 0 => {
                            ((total_bytes.unwrap_or(0) as f64 * 8.0)
                                / (d.as_secs_f64() * 1000.0)) as u32
                        }
                        _ => 0,
                    },
                };
                let dur = decoder
                    .total_duration()
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                player.append(decoder);

                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.audio = Some(info.clone());
                let item = Item {
                    slot,
                    duration: dur,
                    started_at: Instant::now(),
                    paused_accum: Duration::ZERO,
                    paused_at: None,
                };
                if replace {
                    s.current = Some(item);
                    s.queued = None;
                    s.duration = dur;
                    s.position = req.start_at;
                } else {
                    s.queued = Some(item);
                }
                drop(s);

                let _ = tx.send(PlayerEvent::AudioInfo(info));
                if dur > 0.0 {
                    let _ = tx.send(PlayerEvent::Duration(dur));
                }
                if replace {
                    player.play();
                    if req.start_at > 0.05 {
                        let _ = player.try_seek(Duration::from_secs_f64(req.start_at));
                    }
                    let _ = tx.send(PlayerEvent::Playing { slot });
                }
            })
            .map_err(|e| anyhow!("no se pudo lanzar la carga: {e}"))?;
        Ok(())
    }
}

struct LoadingGuard(Arc<AtomicBool>);
impl Drop for LoadingGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn monitor_loop(
    state: Arc<Mutex<State>>,
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
    loading: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    player: Arc<rodio::Player>,
) {
    let mut seen: usize = player.len();
    let mut idle_sent = true;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(80));
        let n = player.len();

        if n < seen {
            // se consumió un source -> terminó un tema
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(done) = s.current.take() {
                let _ = tx.send(PlayerEvent::Ended { slot: done.slot });
                idle_sent = false;
                if let Some(next) = s.queued.take() {
                    s.duration = next.duration;
                    s.position = 0.0;
                    let _ = tx.send(PlayerEvent::Playing { slot: next.slot });
                    s.current = Some(next);
                }
            }
        }
        seen = n;

        let pos = {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            let p = match &s.current {
                Some(c) => {
                    let e = c.elapsed() * player.speed() as f64;
                    s.position = if c.duration > 0.0 { e.min(c.duration) } else { e };
                    s.position
                }
                None => 0.0,
            };
            p
        };
        if pos > 0.0 {
            let _ = tx.send(PlayerEvent::Position(pos));
        }

        if n == 0 && !loading.load(Ordering::SeqCst) && !idle_sent {
            let has = state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .current
                .is_some();
            if !has {
                let _ = tx.send(PlayerEvent::Idle);
                idle_sent = true;
            }
        }
        if n > 0 {
            idle_sent = false;
        }
    }
}

impl Backend for RodioBackend {
    fn name(&self) -> &'static str {
        "rodio (Rust)"
    }

    fn play(&self, slot: Slot, req: PlayRequest) -> Result<()> {
        self.load(req, slot, true)
    }

    fn preload(&self, slot: Slot, req: PlayRequest) -> Result<()> {
        self.load(req, slot, false)
    }

    fn promote(&self, _slot: Slot) {
        // el monitor promueve automáticamente; esto cubre promociones manuales
        let mut s = self.lock();
        if s.current.is_none() {
            if let Some(q) = s.queued.take() {
                s.duration = q.duration;
                s.position = 0.0;
                s.current = Some(q);
            }
        }
    }

    fn pause(&self, on: bool) {
        if on {
            self.player.pause();
        } else {
            self.player.play();
        }
        let mut s = self.lock();
        s.paused = on;
        if let Some(c) = s.current.as_mut() {
            match (on, c.paused_at) {
                (true, None) => c.paused_at = Some(Instant::now()),
                (false, Some(at)) => {
                    c.paused_accum += at.elapsed();
                    c.paused_at = None;
                }
                _ => {}
            }
        }
    }

    fn toggle_pause(&self) -> bool {
        let cur = self.is_paused();
        self.pause(!cur);
        !cur
    }

    fn stop(&self) {
        self.player.stop();
        self.player.clear();
        let mut s = self.lock();
        s.current = None;
        s.queued = None;
        s.position = 0.0;
        s.paused = false;
    }

    fn seek(&self, secs: f64) -> Option<f64> {
        let target = Duration::from_secs_f64(secs.max(0.0));
        if self.player.try_seek(target).is_ok() {
            let mut s = self.lock();
            s.position = secs.max(0.0);
            if let Some(c) = s.current.as_mut() {
                c.started_at = Instant::now() - target;
                c.paused_accum = Duration::ZERO;
            }
            Some(secs.max(0.0))
        } else {
            None
        }
    }

    fn position(&self) -> f64 {
        self.lock().position
    }

    fn duration(&self) -> f64 {
        self.lock().duration
    }

    fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.5);
        self.volume.store((v as f64).to_bits(), Ordering::Relaxed);
        let eff = if self.muted() { 0.0 } else { v };
        self.player.set_volume(eff);
    }

    fn volume(&self) -> f32 {
        f64::from_bits(self.volume.load(Ordering::Relaxed)) as f32
    }

    fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
        self.player.set_volume(if m { 0.0 } else { self.volume() });
    }

    fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    fn set_speed(&self, s: f32) {
        let s = s.clamp(0.25, 4.0);
        self.speed.store((s as f64).to_bits(), Ordering::Relaxed);
        self.player.set_speed(s);
    }

    fn speed(&self) -> f32 {
        f64::from_bits(self.speed.load(Ordering::Relaxed)) as f32
    }

    fn set_replaygain(&self, _mode: &str) {}

    fn audio_info(&self) -> Option<AudioInfo> {
        self.lock().audio.clone()
    }

    fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    fn current_slot(&self) -> Option<Slot> {
        self.lock().current.as_ref().map(|c| c.slot)
    }

    fn audio_devices(&self) -> Vec<(String, String)> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        let mut out = Vec::new();
        if let Ok(devs) = rodio::cpal::default_host().output_devices() {
            for d in devs {
                #[allow(deprecated)]
                if let Ok(n) = d.name() {
                    out.push((n.clone(), n));
                }
            }
        }
        out
    }

    fn set_audio_device(&self, _id: &str) {
        // cambiar de dispositivo implicaría reabrir el sink; se ignora en el fallback
    }

    fn set_exclusive(&self, _on: bool) {}

    fn backend_stats(&self) -> String {
        let cfg = self.device_sink.config();
        format!(
            "salida {}Hz/{}ch · cola {} · {:?}",
            cfg.sample_rate().get(),
            cfg.channel_count().get(),
            self.player.len(),
            cfg.sample_format()
        )
    }
}

impl Drop for RodioBackend {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}
