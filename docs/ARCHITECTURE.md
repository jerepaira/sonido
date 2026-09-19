# Arquitectura de Sonido

Notas de implementación: hilos, canales, cachés y los presupuestos de rendimiento
que guiaron cada decisión.

## 1. Procesos e hilos

```
┌────────────────────────── proceso sonido ──────────────────────────┐
│                                                                    │
│  hilo UI (winit + egui + glow/wgpu)                                │
│     · pinta, no bloquea nunca                                      │
│     · drena 2 canales por frame (AppEvent / AddonResult)           │
│                                                                    │
│  runtime Tokio (N workers = min(cpu, 6), 32 blocking threads)      │
│     · todo lo que es red o decode corre acá vía spawn_blocking     │
│                                                                    │
│  hilo "event-pump"                                                 │
│     · mueve PlayerEvent del backend al canal de la app             │
│                                                                    │
│  hilo "mpv-events"          (solo con libmpv)                      │
│     · mpv_wait_event(0.2 s) → propiedades observadas y END_FILE    │
│                                                                    │
│  hilo "audio-monitor"       (solo backend rodio)                   │
│     · detecta fin de tema y promueve la fuente encadenada          │
│                                                                    │
│  hilos "audio-load" efímeros                                       │
│     · abren el stream HTTP y lo dejan pre-cargado                  │
└────────────────────────────────────────────────────────────────────┘
```

Regla dura: **ninguna llamada de red ni de decode ocurre en el hilo de UI.** Las únicas
operaciones bloqueantes que hace la UI son lecturas de `RwLock` en memoria.

## 2. Canales

| Canal | Productor | Consumidor | Contenido |
|---|---|---|---|
| `atx: UnboundedSender<AddonResult>` | workers de red | `App::poll_events` | resultados de search/catalog/detail/install |
| `tx: UnboundedSender<AppEvent>` | event-pump y workers de playback | `App::poll_events` | `PlayerEvent`, `PlayReady`, `PrerollReady`, `PlayFailed` |

Ambos se drenan una vez por frame, en lote (`try_recv` hasta vaciar), y solo se pide
`request_repaint()` si llegó algo. Con música sonando se repinta igual por la posición;
en reposo la app cae a `request_repaint_after(250 ms)`.

### Ciclo de vida de "play"

```
UI: click en fila
  → App::play_tracks() → Queue::set(items, index)
  → App::play_current() → play_gen += 1 → spawn_resolve(gen, item, 0.0, false)
worker:
  → resolve_direct (streamURL? / caché 4 min? / GET /stream)
  → si falla: resolve_identity por toda la cadena
  → tx.send(PlayReady { gen, resolved, start_at })
UI (frame siguiente):
  → si gen != play_gen, se descarta (el usuario ya clickeó otra cosa)
  → Player::play_resolved → Backend::play(Slot::A, req)
  → library.record_play()
```

El `gen` es lo que hace que un click rápido sobre tres temas distintos no termine
reproduciendo el primero en llegar: gana el último pedido, el resto se tira.

## 3. Gapless

`Queue` tiene los temas; el `Backend` tiene dos *slots* (A/B). La pre-carga se dispara
desde la UI cuando `duración - posición <= prefs.preroll_secs` (default 12 s), una sola
vez por tema (`preroll_started == Some(preroll_gen)`).

```
Backend::play(A, url1)          mpv: stop + playlist-clear + loadfile url1 replace
Backend::preload(B, url2)       mpv: loadfile url2 append-play
                                (mpv pre-bufferiza url2 mientras suena url1)
── EOF de url1 ──
  mpv arranca url2 solo (gapless-audio=yes, mismo dispositivo, sin reopen)
  evento END_FILE(EOF)          → App::on_track_ended → Queue::advance
                                  Player::promote_prerolled (slot A→B)
  evento PLAYBACK_RESTART       → se mide gap = now - eof_at  → PlayerEvent::Gap(ms)
```

Con el backend `rodio` el esquema es el mismo pero la "promoción" la hace el hilo
`audio-monitor`, que observa `Player::len()` bajar de 2 a 1.

El gap se acumula en `Diagnostics.gaps` (últimos 40) y se muestra el promedio en el
panel ⓘ. Es la métrica honesta de si el gapless está funcionando: si el pre-carga no
llegó a tiempo, el gap pasa de ~30 ms a ~300–800 ms y se ve al instante.

## 4. Cachés

| Capa | Clave | TTL | Dónde |
|---|---|---|---|
| manifest | `GET {base}/manifest.json` | 6 h | disco |
| search | `GET {base}/search?q=…&<settings>` | 30 min | disco |
| album/artist/playlist | `GET {base}/{kind}/{id}?<settings>` | 3 d | disco |
| catalog | `GET {base}/catalog/{id}?skip=…&<settings>` | 30 min | disco |
| imágenes | `img|{url}` | 30 d | disco |
| imágenes decodificadas | `{url}` | LRU 2048 | RAM |
| streams resueltos | `{addonId}::{trackId}` | 4 min o `expiresAt` | RAM |
| manifests parseados | `{base}` | mientras viva el proceso | RAM |

Detalle importante: **los settings del addon forman parte de la clave del caché de HTTP.**
Cambiar `quality=MAX` → `quality=LOW` genera otra clave y por lo tanto otra respuesta;
no hay que invalidar nada a mano.

El caché de disco está *shardeado* por los primeros 4 caracteres del hash FNV-1a de la
clave (`~/.cache/sonido/http/ab/cd/<hash>.bin`), con cabecera de 17 bytes
(`u64 timestamp | u64 len | u8 flags`) y escritura atómica `.tmp` + `rename`.
Al arrancar se poda lo más viejo hasta el límite configurable (default 512 MB).

Los **streams no se cachean a disco**: las URLs de audio expiran y una URL vieja produce
un error confuso en el reproductor.

## 5. Deserialización tolerante

Los addons reales son inconsistentes. `models.rs` normaliza:

* `duration` en segundos **o** `durationMs` en milisegundos → siempre segundos
  (heurística: `>= 10_000` es ms).
* números que llegan como string, bools como `"true"`, arrays como CSV.
* `isrc: ""` → `None` (la spec lo dice explícitamente: un string vacío no es un ISRC).
* campos desconocidos → `#[serde(flatten)] extra: BTreeMap<String, Value>`
  (se pueden inspeccionar sin recompilar cuando un addon agrega algo).

Ningún campo raro tira error: lo peor que puede pasar es que un campo quede `None`.

## 6. Scoring de matching

`util::match_score()` decide si un resultado de búsqueda *es* el tema pedido:

```
ISRC igual                      → 1.00 (gana siempre)
si no:  0.55 · sim(título)  +  0.45 · sim(artista)
        ± duración:  +0.08 si |Δ| ≤ 2 s,  −0.25 si |Δ| > 20 s
sim() = 0.75 · dice(bigramas) + 0.25 · prefijo_común
```

Antes de comparar se limpia el ruido de los títulos: se quitan `(...)`/`[...]` que
contengan *remaster, deluxe, live, mono, stereo, explicit, radio edit, version…* y se
normaliza (NFD + plegado ASCII + colapsado de espacios). Umbral de aceptación: **0.62**.

La regla es la misma que propone la spec de Eclipse: *un candidato parecido pero
incorrecto es peor que ninguno*. Si nada supera el umbral, la app pasa al siguiente
addon de la cadena y, si ninguno puede, lo dice.

## 7. Presupuesto de rendimiento

Objetivos y cómo se miden:

| Métrica | Objetivo | Dónde se ve |
|---|---|---|
| Arranque en frío (proceso → primer frame) | < 150 ms | log `arranque en …` |
| Frame de UI | < 4 ms con 1000 filas visibles | panel ⓘ → `frame ui` |
| Búsqueda federada | = max(latencia de addons) | topbar, al terminar |
| Resolución de stream | < 400 ms (1 round-trip) | badge `NNNms` en la playerbar |
| Gap entre temas | < 60 ms con pre-carga | badge `gap NNms` |
| RAM en régimen | < 180 MB con 500 portadas | `ps` / panel ⓘ |

Para medir de verdad:

```bash
RUST_LOG=sonido=trace ./target/release/sonido     # latencia de cada GET
perf stat -e task-clock ./target/release/sonido
```

## 8. Decisiones que se evaluaron y se descartaron

* **Stremio addon protocol** — el manifest de ejemplo tiene forma de Stremio
  (`types`/`resources`/`settings`), pero los endpoints reales del addon son los de
  Eclipse (`/search?q=`, `/stream/{id}`, `/catalog/{id}?skip=`). Se probó
  `/catalog/track/top.json` y `/configure.json`: 404. Se implementó Eclipse.
* **reqwest + tokio para todo** — en reqwest 0.13 el feature `rustls` arrastra
  `aws-lc-rs`, que necesita CMake y un toolchain C en la máquina del usuario. `ureq`
  usa `ring`, compila en cualquier lado y es bloqueante (ideal para `spawn_blocking`).
* **wgpu por defecto** — `wgpu`→`ash`→Vulkan duplica el tiempo de build y la RAM
  necesaria para compilar. Por defecto se usa `glow` (OpenGL), que está en todos los
  drivers Linux; `wgpu` queda detrás de `--features wgpu_renderer`.
* **SQLite para la biblioteca** — la biblioteca (favoritos + playlists + historial) es
  chica y se lee entera al arrancar. Un JSON con escritura atómica arranca más rápido y
  no agrega una dependencia con C.
* **crate `libmpv`/`mpv-sys`** — desactualizados respecto de la API actual. El FFI de
  `player/mpv.rs` son ~40 funciones declaradas a mano contra `/usr/include/mpv/client.h`
  y se valida en `build.rs` con `pkg-config`.

## 9. FFI de libmpv

`build.rs` busca `libmpv` (pkg-config → `MPV_LIB_DIR` → rutas típicas de las distros).
Si la encuentra emite `cfg(mpv)` y linkea `-lmpv`; si no, la app compila igual y usa el
backend Rust. No hay `unsafe` fuera de `player/mpv.rs`.

Detalles que mordieron y quedaron resueltos:

* Los `*const c_char` se cachean por hilo en un `HashMap<String, *const c_char>` con
  `CString::into_raw()` (memoria retenida a propósito): el conjunto de strings es
  acotado y evita un `CString` nuevo por evento en el hot path del event loop.
* `MpvPtr(*mut c_void)` con `unsafe impl Send` para poder mover el handle al hilo de
  eventos (el client API de mpv es thread-safe por contrato).
* Opciones verificadas contra mpv 0.35 (`mpv --opt=x`): `gapless-audio`,
  `demuxer-readahead-secs`, `demuxer-max-bytes`, `network-timeout`, `audio-buffer`,
  `replaygain`, `audio-device`, `audio-exclusive`, `http-header-fields`.
  (`stream-lavf-opts` y `audio-exclusive-mode` **no** existen en 0.35: los headers se
  ponen con `http-header-fields` antes de cada `loadfile`.)
* `END_FILE(EOF)` no distingue "terminó la playlist" de "arrancó el siguiente con
  gapless": se consulta `playlist-playing-pos` en ese momento para decidir si además
  hay que emitir `Idle`.
