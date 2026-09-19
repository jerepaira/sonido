# Sonido

Reproductor de **música en streaming nativo para Linux**, escrito en Rust, que consume
**addons** con el mismo protocolo que usa **Eclipse Music**. Sin Electron, sin WebView,
sin navegador: un binario, una ventana, audio por `libmpv`.

```
┌────────────┬──────────────────────────────────────────────┬───────────┐
│  Sonido    │  ⌕ Buscar temas, álbumes, artistas…   ⓘ FLAC │           │
│            ├──────────────────────────────────────────────┤   COLA    │
│  ⌂ Inicio  │                                              │           │
│  ⌕ Buscar  │   Lo más escuchado · Tido                    │  1 ▶ Luna │
│            │   ┌──────────────────────────────────────┐   │  2   Cama │
│  ♥ Favorit.│   │ 01  Luna sobre el río   Litoral  3:34│   │  3   Baja │
│  ≡ Playlist│   │     HI-RES · ISRC · Tido             │   │           │
│  ☰ Cola    │   ├──────────────────────────────────────┤   │           │
│  ⧉ Addons  │   │ 02  Camalote            Litoral  3:07│   │           │
│            │   └──────────────────────────────────────┘   │           │
│  ⚙ Ajustes │                                              │           │
├────────────┴──────────────────────────────────────────────┴───────────┤
│  ♪ Luna sobre el río   ♥   ⏮  ▶  ⏭   ───●───── 1:12/3:34   GAPLESS   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## Estado: qué está verificado y qué no

**Verificado de punta a punta** (build release + binario corriendo en Linux x86_64):

* `cargo build --release` limpio, con `libmpv` detectada por `build.rs`
  (`sonido: libmpv encontrada en /usr/lib/x86_64-linux-gnu -> gapless + FLAC Hi-Res + Atmos`).
* Binario de ~24 MB, enlazado a `libmpv.so.2`, `libasound.so.2` y `libGL`.
* CLI completa contra un addon mock y contra tu addon real:
  * `sonido add <tu-url-tido>` → lee el manifest, lista los **5 settings**
    (`quality`, `exactQuality`, `hifiApiUrl`, `videosEnabled`, `hideExplicit`),
    los `resources` y los `types` correctamente.
  * `sonido set … quality LOSSLESS` / `--cellular LOW` → guarda ambos valores por red.
  * `sonido stream t7` → la URL devuelta sale con `?quality=LOSSLESS`, es decir
    **los settings viajan como query params en cada petición**, igual que en Eclipse.
  * `sonido search` / `catalog` / `detail album` → 1.6–2 ms contra el mock,
    301.9 ms con 300 ms de latencia artificial inyectada (o sea, el overhead propio es ~2 ms).
  * `sonido play` → resolvió el stream, arrancó `libmpv`, reportó duración y terminó limpio.

**No verificado en este entorno** (no hay display ni GPU donde se desarrolló):

* El **render de la GUI**. Todo el código de UI compila, pero no se vio pintado en una
  ventana. Puede haber detalles visuales a ajustar (espaciados, algún glifo que la fuente
  embebida no tenga).
* `cargo test` (los tests de integración de `tests/addon_protocol.rs` se escribieron
  después del último build). `cargo build` **no** compila `tests/`, así que no afecta.

Si al abrir la GUI algo se ve raro, avisame con una captura y lo ajusto.

---

## Por qué es rápido

| Decisión | Efecto |
|---|---|
| **Rust + egui (immediate mode) + glow/wgpu** | Un solo binario, ~10 ms de arranque de UI, sin JS ni DOM. |
| **Fan-out paralelo por addon** | Con 5 addons, buscar tarda lo que tarda *el más lento*, no la suma. Cada respuesta se pinta apenas llega. |
| **HTTP bloqueante (`ureq`) dentro de `spawn_blocking`** | Cero hyper/tower/h2 en el binario: menos código, menos RAM, menos arranque. Pool de conexiones keep-alive por host, `TCP_NODELAY`, gzip+brotli. |
| **Ningún request en el hilo de UI** | La interfaz nunca se traba: todo va por canales y `request_repaint()`. |
| **Caché a disco con sharding** | Manifests 6 h, búsqueda 30 min, álbum/artista/playlist 3 d, portadas 30 d. El segundo arranque no toca la red. |
| **Portadas reescaladas antes de la GPU** | Nunca se sube un JPEG de 1500×1500 para una miniatura de 48 px. LRU acota la RAM. |
| **Listas virtualizadas (`show_rows`)** | 100 000 temas cuestan lo mismo que 20: solo se dibujan las filas visibles. |
| **Repaint bajo demanda** | Si nada cambia, no se dibuja. Con música sonando repinta por posición; en reposo, 4 Hz. |
| **Gapless por pre-carga** | A los N segundos del final ya se resolvió y pre-cargó el siguiente stream en el motor; el corte entre temas es de unos pocos milisegundos (se mide y se muestra en la UI). |

El panel **ⓘ** de la barra superior muestra en vivo: peticiones HTTP, hits de caché,
latencia de la última búsqueda, tiempo de resolución del stream, gap medido entre temas,
codec/sample rate/bit depth reales y el costo de cada frame de UI.

---

## Requisitos

* Linux (X11 o Wayland)
* `libmpv` **recomendado** (gapless real, FLAC 24/192, Dolby Atmos con downmix).
  Si no está, la app compila y funciona igual con el backend 100% Rust
  (`rodio` + `symphonia` + `cpal`).
* Toolchain de Rust estable (el instalador la baja si falta).

Paquetes de compilación:

| Distro | Comando |
|---|---|
| Debian/Ubuntu | `sudo apt install build-essential pkg-config libmpv-dev libasound2-dev libxkbcommon-dev libgl1-mesa-dev libwayland-dev` |
| Fedora | `sudo dnf install gcc gcc-c++ pkgconf-pkg-config mpv-devel alsa-lib-devel libxkbcommon-devel mesa-libGL-devel wayland-devel` |
| Arch | `sudo pacman -S base-devel pkgconf mpv alsa-lib libxkbcommon mesa wayland` |
| openSUSE | `sudo zypper install gcc gcc-c++ pkg-config libmpv-devel alsa-lib-devel libxkbcommon-devel Mesa-libGL-devel libwayland-devel` |

En runtime solo hace falta `libmpv.so.2` (o `libmpv.so.1`) y ALSA/PipeWire.

## Instalación

```bash
git clone <este repo> sonido && cd sonido
./install.sh                 # dependencias + build release + ~/.local/bin/sonido + .desktop
```

Manual:

```bash
cargo build --release
./target/release/sonido
```

Opciones de build útiles:

```bash
SONIDO_NO_MPV=1 cargo build --release        # sin libmpv (backend Rust puro)
cargo build --release --features wgpu_renderer   # Vulkan en vez de OpenGL
cargo build --release -- --renderer glow     # (runtime) forzar OpenGL
```

---

## Agregar tu addon

El addon que pasaste como ejemplo es un addon de **Eclipse Music**:

```
https://tido.tido.tido.dpdns.org/479f187c279e42819675c81a973529c1/manifest.json
```

Esa URL de 32 hexadecimales es el **token de sesión** que te da Eclipse. Sonido la usa
tal cual: normaliza la base (`…/479f187c…`) y de ahí llama a `/search`, `/stream`, etc.

```bash
sonido add https://tido.tido.tido.dpdns.org/479f187c279e42819675c81a973529c1/manifest.json
```

o en la UI: **Addons → pegar URL → Instalar**.

> ⚠️ Al probar desde este entorno, ese token devolvió `HTTP 403 {"error":"Unavailable"}`
> en `/search` y `/stream` (el `/manifest.json` sí responde). Los tokens de Eclipse
> caducan/son por cuenta: si ves 403, generá una URL nueva desde tu cuenta de Eclipse
> y reinstalá el addon. La app lo reporta explícitamente como
> *"403 — token/permiso rechazado por el addon"*.

### Protocolo implementado

Sonido habla el protocolo de addons de Eclipse Music
([spec](https://eclipsemusic.app/docs)), **no** el de Stremio:

| Endpoint | Uso en Sonido |
|---|---|
| `GET /manifest.json` | id, nombre, versión, ícono, `types`, `resources`, `settings`, `catalogs`, `contentType` |
| `GET /search?q=` | búsqueda federada en paralelo (`tracks`, `albums`, `artists`, `playlists`) |
| `GET /stream/{trackId}` | URL de audio + `codec`, `container`, `manifest`, `sampleRate`, `bitDepth`, `expiresAt`, `chapters`, `video` |
| `GET /album/{id}` | vista de álbum con sus temas |
| `GET /artist/{id}` | top tracks + discografía + bio + géneros |
| `GET /playlist/{id}` | temas de la playlist |
| `GET /catalog/{id}?skip=` | filas de Inicio, paginadas de a 100 |
| `GET /resolve-isrc?isrc=` | fallback exacto por ISRC entre addons |
| `GET /resolve?isrc=&title=&artist=&durationMs=` | fallback por identidad |

**Settings del manifest.** Sonido construye la UI de cada setting
(`select`, `toggle`, `text`, `number`, con `help`, `placeholder`, `min/max/step`,
`maxLength`) y manda los valores **como query params en todas las peticiones**, con la
clave textual del addon:

```
GET /{token}/stream/12345?quality=MAX&exactQuality=false&videosEnabled=true&hideExplicit=false&region=AR
```

Los settings con `perNetwork: true` guardan **dos** valores (Wi-Fi y celular); la app
detecta la interfaz activa (o se fuerza con `SONIDO_NETWORK=cellular`) y manda el que
corresponde. Con tu addon Tido, por ejemplo, podés dejar `quality=MAX` en Wi-Fi y
`quality=LOW` en celular.

También se pueden agregar **headers HTTP** por addon (tokens, `Authorization`, etc.).

### Resolución de streams y cadena de fallback

Cuando apretás play:

1. Si el tema trae `streamURL` → se usa directo (cero round-trips).
2. Si no → `GET /stream/{id}` al addon de origen. Las URLs se cachean **4 min en
   memoria** (o hasta el 80 % de su `expiresAt`), nunca a disco.
3. Si eso falla, se recorre la cadena de addons por prioridad:
   `/resolve-isrc` → `/resolve` → `/search` + *scoring* (ISRC exacto gana; si no,
   similitud de título 0.55 + artista 0.45 + bonus/penalidad por duración). Solo se
   acepta un candidato con score ≥ **0.62**: nunca se reproduce "uno parecido".
4. Se elige el mejor candidato por codec/bit depth/sample rate/bitrate, con bonus si
   activaste *preferir Hi-Res* o *preferir Atmos*.

---

## Uso

### GUI

* **Inicio** — las filas de catálogo (`catalogs` del manifest) de todos los addons
  habilitados, cargadas en paralelo. *Ver más* pagina de a 100.
* **Buscar** — escribe y a los 220 ms sale el fan-out. Tabs de temas/álbumes/artistas/
  playlists. Podés buscar en **todos** los addons o en **uno** (selector del sidebar).
* **Addons** — instalar, activar, ordenar prioridad, configurar settings por red,
  headers, probar el manifest y ver qué declara cada uno.
* **Favoritos / Playlists / Cola** — biblioteca local (`~/.local/share/sonido/library.json`).
* **Ajustes** — gapless, anticipación del pre-carga, ReplayGain, dispositivo de audio,
  modo exclusivo bit-perfect, caché y TTLs.

Click en una fila = **agregar a la cola** (o reproducir si la cola está vacía).
Doble click = **reproducir la lista entera desde ese tema**.

### Atajos

| Tecla | Acción |
|---|---|
| `Espacio` | play / pausa |
| `⌘/Ctrl + →` `←` | tema siguiente / anterior |
| `Shift + →` `←` | ±10 s |
| `/` o `⌘K` | ir a buscar |
| `F` | favorito |
| `M` | silencio |
| `S` | aleatorio |
| `R` | repetir (no → toda la cola → uno) |
| `Q` | panel de cola |
| `1` `2` `3` | Inicio / Buscar / Addons |
| `Esc` | volver |

### CLI

```bash
sonido info                                   # rutas, caché, motor, addons
sonido add <url-manifest>                     # instalar
sonido list                                   # instalados + resources + settings
sonido set com.yourname.tidal.hifi quality MAX
sonido set com.yourname.tidal.hifi quality LOW --cellular
sonido search "litoral" --limit 10
sonido stream --addon com.yourname.tidal.hifi 12345     # muestra URL, codec, 24/192, Atmos…
sonido play   --addon com.yourname.tidal.hifi 12345     # suena por consola
sonido catalog com.yourname.tidal.hifi top --skip 0
sonido detail album com.yourname.tidal.hifi 98765
sonido refresh
```

### Addon de prueba (sin red, sin cuentas)

```bash
python3 tools/mock_addon.py
sonido add http://127.0.0.1:8787/manifest.json
```

Implementa el protocolo completo (search, stream, album, artist, playlist, catalog,
resolve-isrc, resolve, settings con `perNetwork`) y genera WAV locales con soporte de
`Range`, así podés verificar seek, gapless y la UI entera sin depender de nadie.

---

## Dónde guarda las cosas

| Qué | Dónde |
|---|---|
| Addons + preferencias | `~/.config/sonido/addons.json` |
| Favoritos, playlists, historial | `~/.local/share/sonido/library.json` |
| Caché HTTP + portadas | `~/.cache/sonido/http/` (sharded, se poda solo) |
| Fuente opcional | `~/.config/sonido/font.ttf` |

Escritura atómica (`.tmp` + `rename`), flush cada 3 s y al cerrar.

---

## Gapless: cómo está hecho

Con `libmpv` se usa la playlist interna de mpv, que ya hace gapless entre entradas
adyacentes:

```
t = duración - N          resolvemos /stream del siguiente tema (worker)
                          mpv: loadfile <url> append-play     ← queda pre-bufferizado
t = fin del tema A        mpv arranca B solo, sin reabrir el dispositivo de audio
                          evento END_FILE(EOF) → la app promueve B como actual
                          evento PLAYBACK_RESTART → medimos el gap real
```

El gap medido (EOF → primer frame del siguiente) se muestra como badge en la barra de
reproducción: verde ≤ 60 ms, ámbar ≤ 250 ms, rojo arriba. Con `gapless-audio=yes`,
`cache=yes`, `demuxer-readahead-secs=60` y `demuxer-max-bytes=512MiB`.

Sin `libmpv`, el backend Rust encadena dos fuentes en el mismo `Player` de `rodio`
(con `gapless=true` en symphonia, que recorta el padding del encoder). No es bit-perfect
pero no corta.

---

## Estructura del código

```
src/
  main.rs            CLI (clap) + arranque de la ventana (eframe)
  lib.rs
  models.rs          tipos del protocolo + deserialización tolerante
  addon.rs           cliente de un addon (endpoints, settings→query params)
  net.rs             registro de addons, fan-out, detección de red
  resolve.rs         caché de streams, scoring y cadena de fallback
  http.rs            agente ureq, caché a disco, métricas por host
  images.rs          ImageLoader de egui con caché en RAM + disco
  config.rs          store persistido (addons.json / library.json)
  util.rs            normalización de texto, matching, formateo
  player.rs          cola (shuffle/repeat), estado, trait Backend
  player/mpv.rs      FFI a libmpv (a mano, sin crates intermedias)
  player/fallback.rs backend rodio + HttpReader con Range/Seek
  headless.rs        modo consola
  ui/                theme, widgets, shell, browser, addons, library,
                     settings, playerbar, app
tools/mock_addon.py  addon de prueba
docs/ARCHITECTURE.md hilos, eventos, cachés y presupuestos de rendimiento
```

## Solución de problemas

| Síntoma | Causa probable / qué hacer |
|---|---|
| `403 — token/permiso rechazado` | El token del addon caducó. Regenerá la URL desde tu cuenta. |
| `404 — el addon no expone ese endpoint` | El addon no implementa `/album`, `/artist`, `/catalog`… Es opcional en el protocolo. |
| `timeout` | El addon tardó > 20 s. Probá `sonido stream …` para ver la latencia real. |
| Sin sonido | `sonido info` te dice qué motor se usó. Si dice `rodio`, instalá `libmpv` y recompilá. |
| Vulkan falla / pantalla negra | `sonido --renderer glow` (OpenGL). |
| Want Atmos | En el addon poné `quality=MAX` y en Ajustes → *Preferir Dolby Atmos*. El downmix a tu dispositivo lo hace mpv. |

## Limitaciones conocidas

* Sin MPRIS/D-Bus todavía (control desde el teclado multimedia): es el siguiente paso.
* El vídeo de los temas (`video.muxed`) se resuelve y se muestra, pero la reproducción de
  vídeo abre una ventana de mpv aparte (`play_video`), no va dentro de la UI.
* El backend `rodio` no reproduce HLS/DASH ni Atmos; para eso hace falta `libmpv`.
* Las letras (lyrics) no están en el protocolo de Eclipse; no se implementaron.

## Licencia

MIT.
