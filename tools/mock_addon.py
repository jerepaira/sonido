#!/usr/bin/env python3
"""
Addon de prueba que implementa el protocolo de Eclipse Music completo.

Sirve para probar Sonido sin depender de un addon real (tokens vencidos, red,
etc.). Genera audio local (WAV) con soporte de Range requests, así se pueden
probar: búsqueda, catálogos, detalle de álbum/artista/playlist, resolución de
streams, gapless, seek y los settings del manifest.

Uso:
    python3 tools/mock_addon.py                 # http://127.0.0.1:8787/manifest.json
    python3 tools/mock_addon.py --port 9000
    python3 tools/mock_addon.py --seconds 8     # temas más cortos

Después, en Sonido: Addons → instalar
    http://127.0.0.1:8787/manifest.json
O por consola:
    sonido add http://127.0.0.1:8787/manifest.json
    sonido search "luna"
    sonido play --addon com.sonido.mock t1
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import struct
import sys
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

HOST = "127.0.0.1"
PORT = 8787
SECONDS = 22
SR = 44100
CACHE_DIR = os.path.join(os.path.expanduser("~"), ".cache", "sonido-mock")

MANIFEST = {
    "id": "com.sonido.mock",
    "name": "Mock (local)",
    "version": "1.0.0",
    "description": "Addon de prueba: genera audio local para verificar búsqueda, catálogos, streams, settings y gapless sin depender de servicios externos.",
    "types": ["track", "album", "artist", "playlist"],
    "resources": ["search", "stream", "catalog", "settings", "isrc", "resolve"],
    "contentType": "music",
    "catalogs": [
        {"id": "top", "type": "track", "name": "Lo más escuchado"},
        {"id": "new", "type": "album", "name": "Novedades"},
        {"id": "curated", "type": "playlist", "name": "Selección"},
    ],
    "settings": [
        {
            "key": "quality",
            "type": "select",
            "label": "Audio quality",
            "help": "Cambia el bitrate declarado y el tono generado (para verificar que el setting llega al addon).",
            "default": "HIGH",
            "perNetwork": True,
            "options": [
                {"value": "LOSSLESS", "label": "Lossless (16-bit / 44.1 kHz WAV)"},
                {"value": "HIGH", "label": "High (320 kbps)"},
                {"value": "LOW", "label": "Low (96 kbps)"},
            ],
        },
        {
            "key": "videosEnabled",
            "type": "toggle",
            "label": "Enable music videos",
            "help": "Con ON, /stream devuelve además un objeto `video` (URL de ejemplo).",
            "default": False,
            "perNetwork": True,
        },
        {
            "key": "region",
            "type": "text",
            "label": "Region code",
            "placeholder": "AR",
            "default": "AR",
            "maxLength": 2,
        },
        {
            "key": "delayMs",
            "type": "number",
            "label": "Latencia artificial (ms)",
            "help": "Para probar el estado de carga de la UI.",
            "default": 0,
            "min": 0,
            "max": 5000,
            "step": 50,
        },
    ],
}

def fold(s: str) -> str:
    """minúsculas + sin acentos, para que 'neon' matchee 'Neón'."""
    import unicodedata
    return "".join(
        c for c in unicodedata.normalize("NFD", s.lower()) if not unicodedata.combining(c)
    )


ARTISTS = [
    ("a1", "Litoral Eléctrico", ["folktronica", "litoral"]),
    ("a2", "Nadia Sol", ["pop", "sintético"]),
    ("a3", "Cuatro Vientos", ["jazz", "acústico"]),
    ("a4", "Ruido Blanco", ["ambient", "drone"]),
]

ALBUMS = [
    ("al1", "Río de Plata", "a1", "2024", 5, "Trazos de electrónica sobre campo grabado."),
    ("al2", "Neón y Arena", "a2", "2023", 4, "Pop de sintetizadores cálidos."),
    ("al3", "Madrugada en Do", "a3", "2025", 4, "Trio de jazz grabado en una toma."),
    ("al4", "Silencio Útil", "a4", "2022", 3, "Piezas largas para trabajar."),
]

TRACKS = [
    ("t1", "Luna sobre el río", "a1", "al1", 1, 214, "ARMO1234500001", False, True),
    ("t2", "Camalote", "a1", "al1", 2, 187, "ARMO1234500002", False, False),
    ("t3", "Bajamar", "a1", "al1", 3, 232, "ARMO1234500003", False, True),
    ("t4", "Estuario", "a1", "al1", 4, 198, "ARMO1234500004", True, False),
    ("t5", "Sudestada", "a1", "al1", 5, 245, "ARMO1234500005", False, False),
    ("t6", "Neón", "a2", "al2", 1, 176, "ARMO1234500006", False, False),
    ("t7", "Arena y vidrio", "a2", "al2", 2, 203, "ARMO1234500007", False, True),
    ("t8", "Tarde azul", "a2", "al2", 3, 191, "ARMO1234500008", False, False),
    ("t9", "Parpadeo", "a2", "al2", 4, 168, "ARMO1234500009", True, False),
    ("t10", "Madrugada en Do", "a3", "al3", 1, 342, "ARMO1234500010", False, True),
    ("t11", "Cinco esquinas", "a3", "al3", 2, 288, "ARMO1234500011", False, False),
    ("t12", "Vals para nadie", "a3", "al3", 3, 254, "ARMO1234500012", False, False),
    ("t13", "Última copa", "a3", "al3", 4, 301, "ARMO1234500013", False, True),
    ("t14", "Silencio útil I", "a4", "al4", 1, 612, "ARMO1234500014", False, True),
    ("t15", "Ruido de fondo", "a4", "al4", 2, 540, "ARMO1234500015", False, False),
    ("t16", "Blanco", "a4", "al4", 3, 488, "ARMO1234500016", False, False),
]

PLAYLISTS = [
    ("p1", "Para concentrarse", "Mock", ["t14", "t15", "t10", "t3"], "Ambiente y jazz lento."),
    ("p2", "Ruta nocturna", "Mock", ["t6", "t7", "t1", "t13"], "Sintetizadores y ruta."),
]

ART = {
    "a1": None,
    "a2": None,
    "a3": None,
    "a4": None,
}


def artist_name(aid: str) -> str:
    for a in ARTISTS:
        if a[0] == aid:
            return a[1]
    return aid


def album_title(alid: str) -> str:
    for a in ALBUMS:
        if a[0] == alid:
            return a[1]
    return alid


def track(tid: str):
    for t in TRACKS:
        if t[0] == tid:
            return t
    return None


# ──────────────────────────── generación de audio ────────────────────────────

def wav_path(tid: str, quality: str) -> str:
    os.makedirs(CACHE_DIR, exist_ok=True)
    return os.path.join(CACHE_DIR, f"{tid}-{quality}-{SECONDS}s.wav")


def generate_wav(tid: str, quality: str) -> str:
    path = wav_path(tid, quality)
    if os.path.exists(path):
        return path
    t = track(tid) or ("t0", "Tema", "a1", "al1", 1, 200, None, False, False)
    idx = TRACKS.index(t) if t in TRACKS else 0
    base = 174.61 * (2 ** ((idx % 12) / 12.0))  # escala cromática desde F3
    n = int(SR * SECONDS)
    amp = 0.30 if quality != "LOW" else 0.22
    frames = bytearray()
    # dos voces + envolvente lenta para que se note el cambio de tema
    for i in range(n):
        x = i / SR
        env = 0.5 + 0.5 * math.sin(2 * math.pi * x / 8.0)
        v = math.sin(2 * math.pi * base * x) * 0.6
        v += math.sin(2 * math.pi * base * 1.5 * x) * 0.25
        v += math.sin(2 * math.pi * base * 2.0 * x) * 0.15 * env
        # pequeño silencio al inicio/final para probar el gapless
        if x < 0.05 or x > SECONDS - 0.05:
            v *= 0.0
        s = int(max(-1.0, min(1.0, v * amp)) * 32767)
        frames += struct.pack("<hh", s, s)
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(bytes(frames))
    return path


# ──────────────────────────── servidor ────────────────────────────

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "SonidoMock/1.0"

    def log_message(self, fmt, *args):  # menos ruido
        sys.stderr.write("  %s\n" % (fmt % args))

    # ── helpers ──
    def _json(self, obj, status=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _err(self, status, msg):
        self._json({"error": msg}, status)

    def _qs(self):
        return parse_qs(urlparse(self.path).query)

    def _q1(self, key, default=""):
        return self._qs().get(key, [default])[0]

    def _delay(self):
        try:
            ms = int(float(self._q1("delayMs", "0")))
        except ValueError:
            ms = 0
        if ms > 0:
            import time
            time.sleep(ms / 1000.0)

    def _audio_url(self, tid, quality):
        return f"http://{HOST}:{PORT}/audio/{tid}.wav?quality={quality}"

    def _track_obj(self, t, quality, with_stream=False):
        tid, title, aid, alid, num, dur, isrc, explicit, hires = t
        o = {
            "id": tid,
            "title": title,
            "artist": artist_name(aid),
            "album": album_title(alid),
            "duration": dur,
            "isrc": isrc,
            "format": "wav",
            "explicit": explicit,
            "hiRes": hires,
            "trackNumber": num,
        }
        if not with_stream:
            o["artworkURL"] = ""
        else:
            o["streamURL"] = self._audio_url(tid, quality)
        return o

    # ── rutas ──
    def do_GET(self):
        u = urlparse(self.path)
        path = u.path
        self._delay()
        try:
            if path in ("/manifest.json", "/manifest"):
                return self._json(MANIFEST)

            if path == "/search":
                return self.search()

            if path.startswith("/stream/"):
                return self.stream(path[len("/stream/"):])

            if path.startswith("/album/"):
                return self.album(path[len("/album/"):])

            if path.startswith("/artist/"):
                return self.artist(path[len("/artist/"):])

            if path.startswith("/playlist/"):
                return self.playlist(path[len("/playlist/"):])

            if path.startswith("/catalog/"):
                return self.catalog(path[len("/catalog/"):])

            if path == "/resolve-isrc":
                return self.resolve_isrc()

            if path == "/resolve":
                return self.resolve()

            if path.startswith("/audio/"):
                return self.audio(path[len("/audio/"):])

            return self._err(404, "Not found")
        except BrokenPipeError:
            pass
        except Exception as e:  # noqa: BLE001
            return self._err(500, f"{type(e).__name__}: {e}")

    def search(self):
        q = fold(self._q1("q").strip())
        quality = self._q1("quality", "HIGH")
        hide_explicit = self._q1("hideExplicit", "false").lower() == "true"
        if not q:
            return self._err(400, "falta q")

        def ok(t):
            return not (hide_explicit and t[7])

        tracks = [
            self._track_obj(t, quality)
            for t in TRACKS
            if ok(t)
            and (
                q in fold(t[1])
                or q in fold(artist_name(t[2]))
                or q in fold(album_title(t[3]))
            )
        ]
        albums = [
            {
                "id": a[0],
                "title": a[1],
                "artist": artist_name(a[2]),
                "year": a[3],
                "trackCount": a[4],
            }
            for a in ALBUMS
            if q in fold(a[1]) or q in fold(artist_name(a[2]))
        ]
        artists = [
            {"id": a[0], "name": a[1], "genres": a[2]}
            for a in ARTISTS
            if q in fold(a[1])
        ]
        playlists = [
            {
                "id": p[0],
                "title": p[1],
                "creator": p[2],
                "trackCount": len(p[3]),
                "description": p[4],
            }
            for p in PLAYLISTS
            if q in fold(p[1])
        ]
        # si no matcheó nada, devolvemos todo (útil para probar la UI)
        if not (tracks or albums or artists or playlists):
            tracks = [self._track_obj(t, quality) for t in TRACKS[:6] if ok(t)]
        self._json(
            {
                "tracks": tracks,
                "albums": albums,
                "artists": artists,
                "playlists": playlists,
            }
        )

    def stream(self, tid):
        t = track(tid)
        if not t:
            return self._err(404, "track inexistente")
        quality = self._q1("quality", "HIGH")
        videos = self._q1("videosEnabled", "false").lower() == "true"
        resp = {
            "url": self._audio_url(tid, quality),
            "format": "wav",
            "codec": "pcm_s16le",
            "container": "wav",
            "manifest": "none",
            "encrypted": False,
            "sampleRate": SR,
            "bitDepth": 16,
            "channels": 2,
            "quality": {"LOSSLESS": "lossless", "HIGH": "320kbps", "LOW": "96kbps"}.get(
                quality, quality
            ),
            "expiresAt": int(2_000_000_000),
            "chapters": [
                {"title": "Intro", "startTime": 0},
                {"title": "Desarrollo", "startTime": SECONDS // 3},
                {"title": "Cierre", "startTime": (SECONDS * 2) // 3},
            ],
        }
        if videos:
            resp["video"] = {
                "url": "https://example.invalid/video.mp4",
                "mimeType": "video/mp4",
                "muxed": True,
                "width": 1920,
                "height": 1080,
            }
        self._json(resp)

    def album(self, alid):
        a = next((x for x in ALBUMS if x[0] == alid), None)
        if not a:
            return self._err(404, "album inexistente")
        quality = self._q1("quality", "HIGH")
        tracks = [
            self._track_obj(t, quality, with_stream=True)
            for t in TRACKS
            if t[3] == alid
        ]
        self._json(
            {
                "id": a[0],
                "title": a[1],
                "artist": artist_name(a[2]),
                "year": a[3],
                "trackCount": len(tracks),
                "description": a[5],
                "tracks": tracks,
            }
        )

    def artist(self, aid):
        a = next((x for x in ARTISTS if x[0] == aid), None)
        if not a:
            return self._err(404, "artist inexistente")
        quality = self._q1("quality", "HIGH")
        top = [
            self._track_obj(t, quality, with_stream=True)
            for t in TRACKS
            if t[2] == aid
        ][:6]
        albums = [
            {
                "id": al[0],
                "title": al[1],
                "artist": a[1],
                "year": al[3],
                "trackCount": al[4],
            }
            for al in ALBUMS
            if al[2] == aid
        ]
        self._json(
            {
                "id": a[0],
                "name": a[1],
                "genres": a[2],
                "bio": f"{a[1]} es un proyecto de prueba generado por el mock addon de Sonido.",
                "topTracks": top,
                "albums": albums,
            }
        )

    def playlist(self, pid):
        p = next((x for x in PLAYLISTS if x[0] == pid), None)
        if not p:
            return self._err(404, "playlist inexistente")
        quality = self._q1("quality", "HIGH")
        tracks = []
        for tid in p[3]:
            t = track(tid)
            if t:
                tracks.append(self._track_obj(t, quality, with_stream=True))
        self._json(
            {
                "id": p[0],
                "title": p[1],
                "creator": p[2],
                "description": p[4],
                "trackCount": len(tracks),
                "tracks": tracks,
            }
        )

    def catalog(self, cid):
        skip = int(self._q1("skip", "0") or 0)
        quality = self._q1("quality", "HIGH")
        if cid == "top":
            items = [
                {
                    "id": t[0],
                    "type": "track",
                    "title": t[1],
                    "artist": artist_name(t[2]),
                    "album": album_title(t[3]),
                    "durationMs": t[5] * 1000,
                    "isrc": t[6],
                    "explicit": t[7],
                    "hiRes": t[8],
                }
                for t in TRACKS
            ]
        elif cid == "new":
            items = [
                {
                    "id": a[0],
                    "type": "album",
                    "title": a[1],
                    "artist": artist_name(a[2]),
                    "year": a[3],
                }
                for a in ALBUMS
            ]
        elif cid == "curated":
            items = [
                {"id": p[0], "type": "playlist", "title": p[1], "artist": p[2]}
                for p in PLAYLISTS
            ]
        else:
            return self._err(404, "catálogo inexistente")
        page = items[skip : skip + 100]
        self._json({"items": page})
        _ = quality

    def resolve_isrc(self):
        isrc = self._q1("isrc").upper()
        for t in TRACKS:
            if t[6] == isrc:
                return self._json({"trackId": t[0]})
        self._json({"trackId": None})

    def resolve(self):
        isrc = self._q1("isrc").upper()
        title = self._q1("title").lower()
        artist = self._q1("artist").lower()
        for t in TRACKS:
            if isrc and t[6] == isrc:
                return self._json({"item": {"id": t[0], "type": "track", "title": t[1], "artist": artist_name(t[2]), "isrc": t[6]}})
        for t in TRACKS:
            if title and title in fold(t[1]) and (not artist or artist in fold(artist_name(t[2]))):
                return self._json({"item": {"id": t[0], "type": "track", "title": t[1], "artist": artist_name(t[2]), "isrc": t[6]}})
        self._json({"item": None})

    def audio(self, name):
        m = re.match(r"^([a-z0-9]+)\.wav$", name)
        if not m:
            return self._err(404, "audio inexistente")
        tid = m.group(1)
        quality = self._q1("quality", "HIGH")
        path = generate_wav(tid, quality)
        size = os.path.getsize(path)
        rng = self.headers.get("Range")
        start, end = 0, size - 1
        partial = False
        if rng:
            m2 = re.match(r"bytes=(\d*)-(\d*)", rng)
            if m2:
                if m2.group(1):
                    start = int(m2.group(1))
                if m2.group(2):
                    end = min(int(m2.group(2)), size - 1)
                partial = True
        if start >= size or end < start:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{size}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        length = end - start + 1
        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(length))
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Cache-Control", "public, max-age=86400")
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(start)
            remaining = length
            while remaining > 0:
                chunk = f.read(min(65536, remaining))
                if not chunk:
                    break
                self.wfile.write(chunk)
                remaining -= len(chunk)


def main():
    global HOST, PORT, SECONDS
    ap = argparse.ArgumentParser(description="Addon de prueba para Sonido")
    ap.add_argument("--host", default=HOST)
    ap.add_argument("--port", type=int, default=PORT)
    ap.add_argument("--seconds", type=int, default=SECONDS, help="duración de cada tema")
    args = ap.parse_args()
    HOST, PORT, SECONDS = args.host, args.port, args.seconds

    srv = ThreadingHTTPServer((HOST, PORT), Handler)
    print(f"mock addon en http://{HOST}:{PORT}/manifest.json")
    print(f"  temas: {len(TRACKS)} · álbumes: {len(ALBUMS)} · artistas: {len(ARTISTS)} · playlists: {len(PLAYLISTS)}")
    print(f"  audio: WAV 16-bit/{SR} Hz de {SECONDS}s, cacheado en {CACHE_DIR}")
    print("Ctrl-C para terminar")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print("\nadiós")


if __name__ == "__main__":
    main()
